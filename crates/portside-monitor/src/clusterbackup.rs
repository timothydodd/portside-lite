//! One cluster backup on this PC (the scheduled "Cluster" job): the API
//! objects (`manifests::manifest_backup`), and for SSH connections the k3s
//! node's own files, images only the node has and, when asked for, a
//! datastore snapshot. Credentials are sealed with the backup passphrase
//! (`portside_secrets::sealed`) or left out. The layout is described in
//! `portside_core::cluster_backup`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use portside_core::archive::safe_segment;
use portside_core::cluster_backup::{self as cb, BackupIndex, ImageFile, NodeSummary};
use portside_core::{now_ms, Connection, ConnectionProfile, Settings};
use portside_kube::node::Node;
use portside_kube::ClusterClient;
use portside_secrets::sealed;

use crate::{blocking, Monitor};

/// Upper bound for reading the node's files over SSH.
const NODE_TIMEOUT: Duration = Duration::from_secs(120);
/// Upper bound for a datastore snapshot or one image export.
const STREAM_TIMEOUT: Duration = Duration::from_secs(60 * 60);

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A backup folder being written: a hidden `.partial-<stamp>` folder, renamed
/// to `<stamp>` once complete, so a half-written backup never looks like a
/// finished one (and is replaced by the next attempt).
pub(crate) struct Staging {
    dir: PathBuf,
    stamp: String,
    pub(crate) path: PathBuf,
}

impl Staging {
    pub(crate) fn new(dir: &Path, stamp: &str) -> Result<Staging, String> {
        let path = dir.join(format!(".partial-{stamp}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).map_err(|e| format!("Couldn't create {}: {e}", path.display()))?;
        Ok(Staging { dir: dir.to_path_buf(), stamp: stamp.to_string(), path })
    }

    /// The path for `rel`, which must be made of safe segments; parents are created.
    pub(crate) fn file(&self, rel: &str) -> Result<PathBuf, String> {
        if rel.split('/').any(|seg| seg.is_empty() || seg == "." || seg == ".." || safe_segment(seg) != seg) {
            return Err(format!("Refusing to write {rel}"));
        }
        let path = self.path.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
        }
        Ok(path)
    }

    pub(crate) fn write(&self, rel: &str, contents: &[u8]) -> Result<(), String> {
        let path = self.file(rel)?;
        std::fs::write(&path, contents).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
    }

    /// Rename into place as `<stamp>` (or `<stamp>-2`, … if that's taken).
    pub(crate) fn finish(self) -> Result<PathBuf, String> {
        let mut target = self.dir.join(&self.stamp);
        let mut n = 2;
        while target.exists() {
            target = self.dir.join(format!("{}-{n}", self.stamp));
            n += 1;
        }
        std::fs::rename(&self.path, &target).map_err(|e| format!("Couldn't save {}: {e}", target.display()))?;
        Ok(target)
    }

    pub(crate) fn abandon(self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Node files under `node/`, split into plain and sealed; ones holding
/// credentials with nothing to seal them with are left out.
fn split_node_files(files: Vec<(String, Vec<u8>)>, sealing: bool, summary: &mut NodeSummary) -> (Vec<(String, Vec<u8>)>, Vec<(String, Vec<u8>)>) {
    let (mut plain, mut sealed) = (Vec::new(), Vec::new());
    for (rel, data) in files {
        let path = format!("{}/{rel}", cb::NODE_DIR);
        if !cb::node_file_sensitive(&rel, &data) {
            summary.files.push(format!("/{rel}"));
            plain.push((path, data));
        } else if sealing {
            summary.sealed_files.push(format!("/{rel}"));
            sealed.push((path, data));
        } else {
            summary.left_out.push(format!("/{rel}"));
        }
    }
    (plain, sealed)
}

impl Monitor {
    /// Back up one connection's cluster into `<cluster folder>/manifests/<stamp>`.
    /// Returns a summary, plus warnings for parts that couldn't be saved
    /// (the rest is saved anyway).
    pub(crate) async fn cluster_backup_once(&self, cc: &ClusterClient, profile: &ConnectionProfile, cluster_dir: &Path, settings: &Settings) -> Result<(String, Vec<String>), String> {
        let passphrase = settings.passphrase().map(str::to_string);
        let backup = portside_kube::manifests::manifest_backup(&cc.client, passphrase.is_some()).await.map_err(err)?;
        let taken = now_ms();
        let mut index = BackupIndex {
            version: cb::INDEX_VERSION,
            cluster: profile.name.clone(),
            cluster_id: profile.connection.cluster_id(),
            taken_ms: taken,
            server_version: backup.server_version.clone(),
            objects: backup.objects,
            sealed_secrets: backup.sealed_secrets,
            helm_releases: backup.helm_releases,
            custom_resources: backup.custom_resources,
            volume_backups: backup.volume_backups.clone(),
            skipped: backup.skipped.clone(),
            ..Default::default()
        };
        let mut warnings = Vec::new();
        let mut sealed_files = backup.sealed.clone();
        let mut plain_files = backup.files.clone();

        let stamp = chrono::Local::now().format("%Y-%m-%d_%H%M").to_string();
        let manifests_dir = cluster_dir.join("manifests");
        let staging = {
            let dir = manifests_dir.clone();
            let stamp = stamp.clone();
            blocking(move || Staging::new(&dir, &stamp)).await?
        };

        // The node: only reachable through an SSH connection.
        let ssh = match &profile.connection {
            Connection::Ssh(cfg) => Some(cfg.clone()),
            Connection::Local { .. } => None,
        };
        let mut export_note = None;
        let node = match &ssh {
            Some(cfg) => {
                self.set_backup_activity(format!("Reading {}'s node files", profile.name));
                match tokio::time::timeout(NODE_TIMEOUT, Node::connect(cfg)).await {
                    Ok(Ok(node)) => Some(node),
                    Ok(Err(e)) => {
                        warnings.push(format!("Couldn't log in to the node for its files: {e}"));
                        None
                    }
                    Err(_) => {
                        warnings.push(format!("The node didn't answer within {} s; its files weren't saved.", NODE_TIMEOUT.as_secs()));
                        None
                    }
                }
            }
            None => None,
        };
        if let Some(node) = &node {
            match tokio::time::timeout(NODE_TIMEOUT, node.read_files()).await {
                Ok(Ok(files)) => {
                    let mut summary = NodeSummary {
                        hostname: node.info.hostname.clone(),
                        k3s_version: node.info.k3s_version.clone(),
                        datastore: node.info.datastore.clone(),
                        arch: node.info.arch.clone(),
                        exec_start: files.exec_start.clone(),
                        agent_images: node.info.agent_images.clone(),
                        ..Default::default()
                    };
                    let (plain, sealed) = split_node_files(files.files, passphrase.is_some(), &mut summary);
                    plain_files.extend(plain);
                    sealed_files.extend(sealed);
                    index.node = Some(summary);
                }
                Ok(Err(e)) => warnings.push(format!("Couldn't read the node's files: {e}")),
                Err(_) => warnings.push("Reading the node's files timed out.".into()),
            }
        }

        // Images only the node has: exported once each, next to the dated backups.
        if !backup.node_only_images.is_empty() {
            match (&node, settings.backup_images) {
                (_, false) => export_note = Some("Exporting them is off (Settings → Backups to this PC).".to_string()),
                (None, true) => {
                    export_note = Some(match ssh {
                        Some(_) => "They weren't exported: the node couldn't be reached.".to_string(),
                        None => "They can only be exported through an SSH connection to the node.".to_string(),
                    })
                }
                (Some(node), true) => {
                    let images_dir = cluster_dir.join(cb::IMAGES_DIR);
                    let (files, errors) = self.export_images(node, &backup.node_only_images, &images_dir, &profile.name).await;
                    warnings.extend(errors);
                    index.images = files;
                }
            }
        }

        // The datastore: sealed, streamed straight to disk.
        if settings.backup_datastore {
            match (&node, &passphrase) {
                (_, None) => warnings.push("The datastore snapshot needs a backup passphrase (it holds every Secret).".into()),
                (None, _) => warnings.push(if ssh.is_some() { "The datastore snapshot was skipped: the node couldn't be reached." } else { "A datastore snapshot needs an SSH connection to the node." }.into()),
                (Some(node), Some(pass)) => {
                    self.set_backup_activity(format!("Snapshotting {}'s datastore", profile.name));
                    let path = staging.file(cb::DATASTORE_FILE)?;
                    match self.snapshot_datastore(node, &path, pass).await {
                        Ok(()) => index.datastore = node.info.datastore.clone(),
                        Err(e) => {
                            let _ = std::fs::remove_file(&path);
                            warnings.push(format!("The datastore snapshot failed: {e}"));
                        }
                    }
                }
            }
        }

        index.warnings = warnings.clone();
        index.sealed = passphrase.is_some() && !sealed_files.is_empty();
        let when = portside_core::jiff::Timestamp::from_millisecond(taken).map(|t| t.to_string()).unwrap_or_default();
        plain_files.push(("images.md".into(), cb::images_md(&backup.images_in_use, &backup.node_only_images, &index.images, export_note.as_deref()).into_bytes()));
        plain_files.push(("README.md".into(), cb::readme(&index, &when).into_bytes()));
        plain_files.push((cb::INDEX_FILE.into(), serde_json::to_vec_pretty(&index).map_err(err)?));

        let keep = settings.manifest_backup_keep;
        let saved = blocking(move || -> Result<(), String> {
            let result = (|| {
                for (rel, data) in &plain_files {
                    staging.write(rel, data)?;
                }
                if let (Some(pass), false) = (&passphrase, sealed_files.is_empty()) {
                    let data = sealed::seal_files(&sealed_files, pass).map_err(|e| format!("Couldn't seal the secrets: {e}"))?;
                    staging.write(cb::SEALED_FILE, &data)?;
                }
                Ok::<_, String>(())
            })();
            match result {
                Ok(()) => {
                    staging.finish()?;
                    crate::backups::prune(&manifests_dir, keep, |e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'));
                    Ok(())
                }
                Err(e) => {
                    staging.abandon();
                    Err(e)
                }
            }
        })
        .await;
        saved?;

        let mut parts = vec![format!("{} objects", index.objects)];
        if index.sealed_secrets > 0 {
            parts.push(format!("{} secrets sealed", index.sealed_secrets));
        }
        if index.node.is_some() {
            parts.push("node files".into());
        }
        if !index.images.is_empty() {
            parts.push(format!("{} node-only images", index.images.len()));
        }
        if index.datastore.is_some() {
            parts.push("datastore".into());
        }
        let mut summary = parts.join(", ");
        if !index.skipped.is_empty() {
            summary.push_str(&format!("; {} kinds left out (see README.md)", index.skipped.len()));
        }
        Ok((summary, warnings))
    }

    async fn snapshot_datastore(&self, node: &Node, path: &Path, passphrase: &str) -> Result<(), String> {
        let file = std::fs::File::create(path).map_err(|e| format!("Couldn't create {}: {e}", path.display()))?;
        let mut writer = sealed::seal_writer(std::io::BufWriter::new(file), passphrase).map_err(err)?;
        let streamed = tokio::time::timeout(STREAM_TIMEOUT, node.datastore_snapshot(&mut |d| writer.write_all(d))).await;
        match streamed {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(err(e)),
            Err(_) => return Err("it took over an hour".into()),
        }
        writer.finish().and_then(|mut w| w.flush()).map_err(err)
    }

    /// Export each image not already saved; drop saved ones nothing uses any more.
    async fn export_images(&self, node: &Node, images: &[cb::NodeImage], dir: &Path, profile: &str) -> (Vec<ImageFile>, Vec<String>) {
        let (mut files, mut errors) = (Vec::new(), Vec::new());
        if let Err(e) = std::fs::create_dir_all(dir) {
            return (files, vec![format!("Couldn't create {}: {e}", dir.display())]);
        }
        for img in images {
            let name = cb::image_file_name(&img.image, &img.image_id);
            let dest = dir.join(&name);
            if dest.exists() {
                files.push(ImageFile { image: img.image.clone(), file: name });
                continue;
            }
            self.set_backup_activity(format!("Exporting {} from {profile}'s node", img.image));
            let part = dir.join(format!(".{name}.part"));
            let result = async {
                let mut out = std::io::BufWriter::new(std::fs::File::create(&part).map_err(err)?);
                match tokio::time::timeout(STREAM_TIMEOUT, node.export_image(&img.image, &mut |d| out.write_all(d))).await {
                    Ok(r) => r.map_err(err)?,
                    Err(_) => return Err("it took over an hour".to_string()),
                }
                out.flush().map_err(err)?;
                drop(out);
                std::fs::rename(&part, &dest).map_err(err)
            }
            .await;
            match result {
                Ok(()) => files.push(ImageFile { image: img.image.clone(), file: name }),
                Err(e) => {
                    let _ = std::fs::remove_file(&part);
                    errors.push(format!("Couldn't export {}: {e}", img.image));
                }
            }
        }
        // Images no longer in use (or rebuilt under the same tag) go.
        let keep: Vec<&str> = files.iter().map(|f| f.file.as_str()).collect();
        if errors.is_empty() {
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.ends_with(".tar") && !keep.contains(&n.as_str()) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        (files, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_files_split_by_what_they_hold() {
        let files = vec![
            ("etc/rancher/k3s/config.yaml".to_string(), b"disable: [traefik]\n".to_vec()),
            ("etc/rancher/k3s/registries.yaml".to_string(), b"configs: {}\n".to_vec()),
            ("var/lib/rancher/k3s/server/token".to_string(), b"K10abc".to_vec()),
        ];
        let mut s = NodeSummary::default();
        let (plain, sealed) = split_node_files(files.clone(), true, &mut s);
        assert_eq!(plain.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(), ["node/etc/rancher/k3s/config.yaml"]);
        assert_eq!(sealed.len(), 2);
        let mut s = NodeSummary::default();
        let (_, sealed) = split_node_files(files, false, &mut s);
        assert!(sealed.is_empty());
        assert_eq!(s.left_out, ["/etc/rancher/k3s/registries.yaml", "/var/lib/rancher/k3s/server/token"]);
    }

    #[test]
    fn staging_writes_whole_folders() {
        let dir = std::env::temp_dir().join(format!("portside-staging-test-{}", now_ms()));
        for _ in 0..2 {
            let st = Staging::new(&dir, "2026-10-03_0300").unwrap();
            st.write("namespaces/shop/Deployment.yaml", b"b").unwrap();
            assert!(st.write("../evil", b"").is_err());
            st.finish().unwrap();
        }
        assert!(dir.join("2026-10-03_0300/namespaces/shop/Deployment.yaml").exists());
        assert!(dir.join("2026-10-03_0300-2/namespaces/shop/Deployment.yaml").exists(), "same minute twice: kept apart");
        let st = Staging::new(&dir, "x").unwrap();
        st.abandon();
        assert!(!dir.join(".partial-x").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
