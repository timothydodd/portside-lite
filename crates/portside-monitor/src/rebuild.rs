//! Rebuild a cluster from a backup on this PC (Backups → Rebuild a
//! cluster), onto any saved connection. Two steps, because k3s has to be
//! installed in between on a new machine:
//!
//! 1. [`Monitor::restore_node_files`] (SSH targets): the k3s config,
//!    registries and custom manifests go back on the node before k3s is
//!    installed (the installer reads them), images only the old node had go
//!    into `agent/images` (k3s imports them at start), and a running k3s is
//!    restarted to pick them up.
//! 2. [`Monitor::rebuild_cluster`]: the objects in phases: CRDs (waited on
//!    until established), then everything that doesn't run pods (Secrets
//!    from the sealed file among them), then each backed-up claim's newest
//!    archive from this PC, then the workloads, so nothing has a claim
//!    mounted while it's written. Objects k3s makes itself are left to it
//!    (`cluster_backup::rebuild_skip`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use portside_core::archive::safe_segment;
use portside_core::backup::ARCHIVE_SUFFIX;
use portside_core::cluster_backup::{self as cb, BackupIndex, RestorePhase, VolumeBackupRef};
use portside_core::manifest::{parse_sources, ManifestDoc, SourceFile};
use portside_core::Connection;
use portside_kube::manifests::{import_docs, ImportResult};
use portside_kube::node::Node;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::k3s::K3sInstallLog;
use crate::{blocking, Monitor};

const CRD_WAIT: Duration = Duration::from_secs(60);
/// The `kind` of a result row for a claim's files.
pub const VOLUME_FILES: &str = "Volume files";

/// The newest archive of one volume backup on this PC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeArchive {
    pub namespace: String,
    pub name: String,
    pub file: String,
    pub bytes: u64,
    pub modified_ms: Option<i64>,
}

/// One cluster backup on this PC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterBackupEntry {
    /// The cluster's folder under the backup folder.
    pub folder: String,
    /// The backup's folder under `manifests/`.
    pub stamp: String,
    /// `None` for backups made before `backup.json` existed (manifests only).
    pub index: Option<BackupIndex>,
    pub sealed: bool,
    pub taken_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterBackups {
    pub root: String,
    /// Newest first.
    pub backups: Vec<ClusterBackupEntry>,
    /// Per cluster folder: the newest archive of each volume backup.
    pub volumes: HashMap<String, Vec<VolumeArchive>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildRequest {
    pub folder: String,
    pub stamp: String,
    pub target_profile_id: String,
    /// Typed in the dialog; `None` = the saved one.
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Copy each backed-up claim's newest archive from this PC into it.
    pub volumes: bool,
    pub dry_run: bool,
    /// Tags progress events (`files:progress`) and lets the UI cancel a copy.
    #[serde(default)]
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RebuildOutcome {
    pub results: Vec<ImportResult>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRestoreRequest {
    pub folder: String,
    pub stamp: String,
    pub target_profile_id: String,
    #[serde(default)]
    pub passphrase: Option<String>,
    /// Restart k3s afterwards if it's running, so it reads the files.
    pub restart: bool,
    /// Tags the log lines (`k3s:install` events).
    pub job_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRestoreOutcome {
    pub files: Vec<String>,
    pub images: Vec<String>,
    pub restarted: bool,
    /// k3s is installed on the target (if not, install it next).
    pub k3s_installed: bool,
}

fn modified_ms(meta: &std::fs::Metadata) -> Option<i64> {
    meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64)
}

fn dirs(path: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with('.') && safe_segment(n) == *n)
        .collect();
    out.sort();
    out
}

/// The newest archive of each volume backup under `<cluster>/volumes`.
fn volume_archives(cluster: &Path) -> Vec<VolumeArchive> {
    let base = cluster.join("volumes");
    let mut out = Vec::new();
    for ns in dirs(&base) {
        for name in dirs(&base.join(&ns)) {
            let prefix = format!("{name}-");
            let newest = std::fs::read_dir(base.join(&ns).join(&name))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|e| {
                    let file = e.file_name().to_string_lossy().into_owned();
                    let meta = e.metadata().ok().filter(|m| m.is_file())?;
                    (file.starts_with(&prefix) && file.ends_with(ARCHIVE_SUFFIX)).then_some((file, meta))
                })
                .max_by(|a, b| a.0.cmp(&b.0));
            if let Some((file, meta)) = newest {
                out.push(VolumeArchive { namespace: ns.clone(), name: name.clone(), file, bytes: meta.len(), modified_ms: modified_ms(&meta) });
            }
        }
    }
    out
}

/// Every `.yaml` under the backup's `cluster/` and `namespaces/`, as import sources.
fn yaml_sources(dir: &Path) -> Result<Vec<SourceFile>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<SourceFile>) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("Couldn't read {}: {e}", dir.display()))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let path = e.path();
            if path.is_dir() {
                walk(base, &path, out)?;
            } else if path.extension().is_some_and(|x| x == "yaml") {
                let content = std::fs::read_to_string(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
                let name = path.strip_prefix(base).unwrap_or(&path).to_string_lossy().replace('\\', "/");
                out.push(SourceFile { name, content });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    for sub in ["cluster", "namespaces"] {
        let d = dir.join(sub);
        if d.is_dir() {
            walk(dir, &d, &mut out)?;
        }
    }
    Ok(out)
}

fn row(index: usize, source: &str, kind: &str, namespace: Option<String>, name: &str, outcome: &str, message: Option<String>) -> ImportResult {
    ImportResult { index, source: source.into(), kind: kind.into(), name: name.into(), namespace, outcome: outcome.into(), message }
}

impl Monitor {
    /// Every cluster backup under the backup folder, newest first.
    pub async fn cluster_backups(&self) -> ClusterBackups {
        let root = self.backup_root();
        let root_str = root.to_string_lossy().into_owned();
        blocking(move || {
            let mut backups = Vec::new();
            let mut volumes = HashMap::new();
            for folder in dirs(&root) {
                let cluster = root.join(&folder);
                let archives = volume_archives(&cluster);
                if !archives.is_empty() {
                    volumes.insert(folder.clone(), archives);
                }
                for stamp in dirs(&cluster.join("manifests")) {
                    let dir = cluster.join("manifests").join(&stamp);
                    let index: Option<BackupIndex> = std::fs::read(dir.join(cb::INDEX_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());
                    let taken_ms = index.as_ref().map(|i| i.taken_ms).or_else(|| std::fs::metadata(&dir).ok().as_ref().and_then(modified_ms));
                    backups.push(ClusterBackupEntry { sealed: dir.join(cb::SEALED_FILE).is_file(), folder: folder.clone(), stamp, index, taken_ms });
                }
            }
            backups.sort_by(|a, b| b.taken_ms.cmp(&a.taken_ms));
            ClusterBackups { root: root_str, backups, volumes }
        })
        .await
    }

    /// `<root>/<folder>` and `<root>/<folder>/manifests/<stamp>`, refusing
    /// names that could point anywhere else.
    fn backup_dirs(&self, folder: &str, stamp: &str) -> Result<(PathBuf, PathBuf), String> {
        let ok = |s: &str| !s.is_empty() && !s.starts_with('.') && safe_segment(s) == s;
        if !ok(folder) || !ok(stamp) {
            return Err("That isn't a backup in the backup folder.".into());
        }
        let cluster = self.backup_root().join(folder);
        let dir = cluster.join("manifests").join(stamp);
        if !dir.is_dir() {
            return Err(format!("{} doesn't exist any more.", dir.display()));
        }
        Ok((cluster, dir))
    }

    fn passphrase_for(&self, typed: Option<&str>) -> Option<String> {
        typed.filter(|p| !p.is_empty()).map(str::to_string).or_else(|| self.settings().passphrase().map(str::to_string))
    }

    /// The sealed file's contents, or an empty list when the backup has none.
    async fn open_sealed(&self, dir: &Path, typed: Option<&str>) -> Result<Vec<(String, Vec<u8>)>, String> {
        let path = dir.join(cb::SEALED_FILE);
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let Some(pass) = self.passphrase_for(typed) else {
            return Err("This backup's Secrets are sealed: enter the backup passphrase it was made with.".into());
        };
        blocking(move || {
            let data = std::fs::read(&path).map_err(|e| format!("Couldn't read {}: {e}", path.display()))?;
            portside_secrets::sealed::open_files(&data, &pass)
        })
        .await
    }

    fn log_to(&self, job_id: &str) -> impl Fn(&str) + Send + Sync + use<> {
        let sink = Arc::clone(&self.sink);
        let job_id = job_id.to_string();
        move |text: &str| sink.k3s_install_log(&K3sInstallLog { job_id: job_id.clone(), text: format!("{text}\n") })
    }

    /// Put the backup's node files (and node-only images) on the target's node.
    pub async fn restore_node_files(&self, req: NodeRestoreRequest) -> Result<NodeRestoreOutcome, String> {
        let (cluster, dir) = self.backup_dirs(&req.folder, &req.stamp)?;
        let settings = self.settings();
        let profile = settings.connections.iter().find(|p| p.id == req.target_profile_id).ok_or("That connection no longer exists.")?;
        let Connection::Ssh(cfg) = &profile.connection else {
            return Err("Node files can only be put back through an SSH connection to the node.".into());
        };
        let log = self.log_to(&req.job_id);
        let index: Option<BackupIndex> = std::fs::read(dir.join(cb::INDEX_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok());

        // What goes back: plain files under node/, plus sealed node files when the passphrase opens them.
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        let node_dir = dir.join(cb::NODE_DIR);
        if node_dir.is_dir() {
            let base = dir.clone();
            files = blocking(move || {
                let mut out = Vec::new();
                let mut stack = vec![node_dir];
                while let Some(d) = stack.pop() {
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        let p = e.path();
                        if p.is_dir() {
                            stack.push(p);
                        } else if let Ok(data) = std::fs::read(&p) {
                            out.push((p.strip_prefix(&base).unwrap_or(&p).to_string_lossy().replace('\\', "/"), data));
                        }
                    }
                }
                out
            })
            .await;
        }
        let sealed_has_node = index.as_ref().and_then(|i| i.node.as_ref()).is_some_and(|n| !n.sealed_files.is_empty());
        if sealed_has_node {
            match self.open_sealed(&dir, req.passphrase.as_deref()).await {
                Ok(sealed) => files.extend(sealed.into_iter().filter(|(p, _)| p.starts_with(&format!("{}/", cb::NODE_DIR)))),
                Err(e) => log(&format!("Sealed node files (registry logins, a config with a token) aren't restored: {e}")),
            }
        }
        let mut targets: Vec<(String, Vec<u8>)> = files.into_iter().filter_map(|(rel, data)| cb::node_restore_dest(&rel).map(|d| (d, data))).collect();
        targets.sort_by(|a, b| a.0.cmp(&b.0));

        log(&format!("$ Connecting to {}", cfg.host));
        let node = Node::connect(cfg).await.map_err(|e| e.to_string())?;
        let installed = node.info.k3s_version.is_some();
        let mut done = NodeRestoreOutcome { files: Vec::new(), images: Vec::new(), restarted: false, k3s_installed: installed };
        for (dest, data) in &targets {
            log(&format!("Writing {dest}"));
            node.put_bytes(data, dest, "0600").await.map_err(|e| format!("Couldn't write {dest}: {e}"))?;
            done.files.push(dest.clone());
        }
        for img in index.as_ref().map(|i| i.images.clone()).unwrap_or_default() {
            let local = cluster.join(cb::IMAGES_DIR).join(&img.file);
            if safe_segment(&img.file) != img.file || !local.is_file() {
                log(&format!("Image {} isn't on this PC any more; skipped.", img.image));
                continue;
            }
            let dest = format!("/var/lib/rancher/k3s/agent/images/{}", img.file);
            log(&format!("Copying image {} to {dest}", img.image));
            node.put_file(&local, &dest, "0644").await.map_err(|e| format!("Couldn't copy {}: {e}", img.image))?;
            if node.info.running {
                match node.import_image(&dest).await {
                    Ok(_) => log(&format!("Imported {}", img.image)),
                    Err(e) => log(&format!("Couldn't import {} now ({e}); k3s imports it at its next start.", img.image)),
                }
            }
            done.images.push(img.image);
        }
        if targets.is_empty() && done.images.is_empty() {
            log("This backup has no node files to put back.");
        }
        if req.restart && node.info.running && !targets.is_empty() {
            log("$ Restarting k3s");
            node.restart_k3s().await.map_err(|e| format!("The files are in place, but k3s didn't restart: {e}"))?;
            done.restarted = true;
        }
        log(if installed { "Done." } else { "Done. k3s isn't installed yet: install it next; it reads these files when it starts." });
        Ok(done)
    }

    /// Send the backup's objects to the target, restore volume data, start the workloads.
    pub async fn rebuild_cluster(self: &Arc<Self>, req: RebuildRequest) -> Result<RebuildOutcome, String> {
        let (cluster, dir) = self.backup_dirs(&req.folder, &req.stamp)?;
        let sealed = self.open_sealed(&dir, req.passphrase.as_deref()).await?;
        let mut sources = {
            let dir = dir.clone();
            blocking(move || yaml_sources(&dir)).await?
        };
        if let Some((_, data)) = sealed.iter().find(|(p, _)| p == cb::SEALED_SECRETS) {
            sources.push(SourceFile { name: "Secrets (sealed)".into(), content: String::from_utf8_lossy(data).into_owned() });
        }
        let parsed = parse_sources(&sources);
        let dst = self.connect_profile(&req.target_profile_id).await?;

        let mut results: Vec<ImportResult> = Vec::new();
        let mut warnings = Vec::new();
        let values: Vec<Value> = parsed.iter().filter_map(|(_, v)| v.clone()).collect();
        let charts = cb::helm_chart_names(&values);
        let mut phases: HashMap<RestorePhase, Vec<(ManifestDoc, Value)>> = HashMap::new();
        for (doc, value) in parsed {
            let Some(value) = value else {
                results.push(row(doc.index, &doc.source, doc.kind.as_deref().unwrap_or("?"), doc.namespace.clone(), doc.name.as_deref().unwrap_or("?"), "error", doc.error.clone()));
                continue;
            };
            if let Some(why) = cb::rebuild_skip(&value, &charts) {
                results.push(row(doc.index, &doc.source, doc.kind.as_deref().unwrap_or_default(), doc.namespace.clone(), doc.name.as_deref().unwrap_or_default(), "skipped", Some(why)));
                continue;
            }
            phases.entry(cb::restore_phase(doc.kind.as_deref().unwrap_or_default())).or_default().push((doc, value));
        }

        // 1. CRDs, then wait for them to be served.
        let crds = phases.remove(&RestorePhase::Definitions).unwrap_or_default();
        let crd_names: Vec<String> = crds.iter().filter_map(|(d, _)| d.name.clone()).collect();
        let crd_kinds: Vec<String> = crds.iter().filter_map(|(_, v)| v.pointer("/spec/names/kind").and_then(Value::as_str).map(str::to_string)).collect();
        results.extend(import_docs(&dst.client, crds, None, req.dry_run).await);
        if !req.dry_run && !crd_names.is_empty() {
            let late = portside_kube::manifests::wait_established(&dst.client, &crd_names, CRD_WAIT).await;
            if !late.is_empty() {
                warnings.push(format!("These CRDs weren't ready after {} s, so their objects may have failed: {}", CRD_WAIT.as_secs(), late.join(", ")));
            }
        }

        // 2. Everything that doesn't run pods.
        let mut objects = import_docs(&dst.client, phases.remove(&RestorePhase::Objects).unwrap_or_default(), None, req.dry_run).await;
        if req.dry_run {
            // Objects of a CRD the dry run couldn't create can't be checked yet.
            for r in objects.iter_mut().filter(|r| r.outcome == "error" && crd_kinds.contains(&r.kind)) {
                r.outcome = "created".into();
                r.message = Some("checked once its CRD exists".into());
            }
        }
        results.extend(objects);

        // 3. Volume data from this PC, before anything mounts the claims.
        if req.volumes {
            let refs: Vec<VolumeBackupRef> = cb::volume_backup_refs(&values);
            let archives = {
                let cluster = cluster.clone();
                blocking(move || volume_archives(&cluster)).await
            };
            for r in refs {
                let label = format!("{}/{}", r.namespace, r.claim);
                let Some(a) = archives.iter().find(|a| a.namespace == safe_segment(&r.namespace) && a.name == safe_segment(&r.name)) else {
                    results.push(row(results.len(), "volumes", VOLUME_FILES, Some(r.namespace.clone()), &r.claim, "error", Some("no archive of this backup on this PC (is copying volume backups on?)".into())));
                    continue;
                };
                let path = cluster.join("volumes").join(&a.namespace).join(&a.name).join(&a.file);
                let message = format!("{} ({})", a.file, crate::copydata::fmt_bytes(a.bytes));
                if req.dry_run {
                    results.push(row(results.len(), "volumes", VOLUME_FILES, Some(r.namespace.clone()), &r.claim, "copied", Some(format!("from {message}"))));
                    continue;
                }
                let claim_ok = results.iter().any(|x| x.kind == "PersistentVolumeClaim" && x.name == r.claim && x.namespace.as_deref() == Some(r.namespace.as_str()) && x.outcome != "error");
                if !claim_ok {
                    results.push(row(results.len(), "volumes", VOLUME_FILES, Some(r.namespace.clone()), &r.claim, "error", Some("the claim wasn't created, so its files can't be restored".into())));
                    continue;
                }
                let job = if req.job_id.is_empty() { format!("rebuild-{label}") } else { req.job_id.clone() };
                let outcome = self.write_volume_data(&dst, &r.namespace, &r.claim, &path, a.bytes, &job).await;
                results.push(match outcome {
                    Ok(()) => row(results.len(), "volumes", VOLUME_FILES, Some(r.namespace.clone()), &r.claim, "copied", Some(format!("from {message}"))),
                    Err(e) => row(results.len(), "volumes", VOLUME_FILES, Some(r.namespace.clone()), &r.claim, "error", Some(e)),
                });
            }
        }

        // 4. The workloads.
        results.extend(import_docs(&dst.client, phases.remove(&RestorePhase::Workloads).unwrap_or_default(), None, req.dry_run).await);
        if !req.dry_run && self.settings().active_connection_id.as_deref() == Some(req.target_profile_id.as_str()) {
            self.refresh_now();
        }
        Ok(RebuildOutcome { results, warnings })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_newest_archive_of_each_volume_backup() {
        let root = std::env::temp_dir().join(format!("portside-rebuild-test-{}", portside_core::now_ms()));
        let d = root.join("volumes/shop/uploads");
        std::fs::create_dir_all(&d).unwrap();
        for f in ["uploads-20261001T020000Z.tar.gz", "uploads-20261002T020000Z.tar.gz", "other-20261009T020000Z.tar.gz", "uploads-x.part"] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let found = volume_archives(&root);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].file, "uploads-20261002T020000Z.tar.gz");

        std::fs::create_dir_all(root.join("m/cluster")).unwrap();
        std::fs::create_dir_all(root.join("m/namespaces/shop")).unwrap();
        std::fs::write(root.join("m/cluster/Namespace.yaml"), "a").unwrap();
        std::fs::write(root.join("m/namespaces/shop/Deployment.yaml"), "b").unwrap();
        std::fs::write(root.join("m/README.md"), "c").unwrap();
        let names: Vec<String> = yaml_sources(&root.join("m")).unwrap().into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["cluster/Namespace.yaml", "namespaces/shop/Deployment.yaml"]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
