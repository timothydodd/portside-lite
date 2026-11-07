//! The k3s server node over SSH, for cluster backups and rebuilds: read its
//! config and custom manifests, snapshot its datastore, export images only
//! it has, and put files back. The scripts and rules are in
//! `portside_core::cluster_backup`.

use std::io::Read;
use std::path::Path;

use portside_core::cluster_backup::{self as cb, NodeInfo};
use portside_core::SshConnection;

use crate::ssh::Session;
use crate::{sudo_command, KubeError, Result};

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// One SSH login to the node, reused for every step.
pub struct Node {
    session: Session,
    password: Option<String>,
    pub info: NodeInfo,
    pub fingerprint: String,
}

/// What [`Node::read_files`] found: paths relative to `/`, k3s's default manifests dropped.
pub struct NodeFiles {
    pub files: Vec<(String, Vec<u8>)>,
    pub exec_start: Option<String>,
}

impl Node {
    pub async fn connect(cfg: &SshConnection) -> Result<Node> {
        let session = Session::connect(cfg).await?;
        let password = cfg.effective_sudo_password().map(str::to_string);
        let mut node = Node { fingerprint: session.fingerprint.clone(), session, password, info: NodeInfo::default() };
        let out = node.sudo_bytes(&cb::sudo_script(cb::NODE_INFO_SCRIPT)).await?;
        node.info = cb::parse_node_info(&String::from_utf8_lossy(&out));
        Ok(node)
    }

    fn sudo(&self, command: &str) -> String {
        sudo_command(command, self.password.is_some())
    }

    async fn sudo_bytes(&self, command: &str) -> Result<Vec<u8>> {
        self.session.exec_bytes(&self.sudo(command), self.password.as_deref()).await
    }

    async fn sudo_to(&self, command: &str, out: &mut (dyn FnMut(&[u8]) -> std::io::Result<()> + Send)) -> Result<()> {
        self.session.exec_to(&self.sudo(command), self.password.as_deref(), out).await
    }

    async fn sudo_text(&self, command: &str) -> Result<String> {
        self.sudo_bytes(command).await.map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// The k3s config, registries, custom manifests, token and service unit.
    pub async fn read_files(&self) -> Result<NodeFiles> {
        let tgz = self.sudo_bytes(&cb::sudo_script(&cb::node_files_script())).await?;
        let mut files = Vec::new();
        if !tgz.is_empty() {
            let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tgz.as_slice()));
            let bad = |e: std::io::Error| other(format!("couldn't read the node's files: {e}"));
            for entry in archive.entries().map_err(bad)? {
                let mut entry = entry.map_err(bad)?;
                if !entry.header().entry_type().is_file() {
                    continue;
                }
                let rel = entry.path().map_err(bad)?.to_string_lossy().trim_start_matches("./").to_string();
                if rel.split('/').any(|s| s == "..") || !cb::keep_node_file(&rel) {
                    continue;
                }
                let mut data = Vec::new();
                entry.read_to_end(&mut data).map_err(bad)?;
                files.push((rel, data));
            }
        }
        files.sort();
        let exec_start = files
            .iter()
            .find(|(p, _)| p == "etc/systemd/system/k3s.service" || p == "etc/init.d/k3s")
            .and_then(|(_, d)| cb::exec_start(&String::from_utf8_lossy(d)));
        Ok(NodeFiles { files, exec_start })
    }

    /// Stream a snapshot of the datastore (plus the token) as `.tar.gz` into `out`.
    pub async fn datastore_snapshot(&self, out: &mut (dyn FnMut(&[u8]) -> std::io::Result<()> + Send)) -> Result<()> {
        self.sudo_to(&cb::sudo_script(cb::DATASTORE_SCRIPT), out).await
    }

    /// Stream one image out of the node's containerd as a tar into `out`.
    pub async fn export_image(&self, image: &str, out: &mut (dyn FnMut(&[u8]) -> std::io::Result<()> + Send)) -> Result<()> {
        let cmd = cb::image_export_command(image, &self.info.arch).map_err(other)?;
        self.sudo_to(&cmd, out).await
    }

    /// Copy a local file to `dest` on the node (as root, with `mode`):
    /// first into a private temp file in the login's home, then moved into
    /// place with sudo.
    pub async fn put_file(&self, local: &Path, dest: &str, mode: &str) -> Result<()> {
        let home = self.session.exec("printf '%s' \"$HOME\"", None).await?;
        let home = home.trim();
        if home.is_empty() {
            return Err(other("the login has no home folder to upload through"));
        }
        let tmp = format!("{home}/.portside-upload-{}", portside_core::now_ms());
        let file = tokio::fs::File::open(local).await.map_err(|e| other(format!("couldn't open {}: {e}", local.display())))?;
        self.session.exec_with_input(&format!("umask 077 && cat > {}", cb::sh_quote(&tmp)), file).await?;
        let script = format!("install -D -m {mode} {src} {dest}; s=$?; rm -f {src}; exit $s", src = cb::sh_quote(&tmp), dest = cb::sh_quote(dest));
        let moved = self.sudo_bytes(&cb::sudo_script(&script)).await;
        if moved.is_err() {
            let _ = self.session.exec(&format!("rm -f {}", cb::sh_quote(&tmp)), None).await;
        }
        moved.map(|_| ())
    }

    /// Like [`put_file`](Self::put_file), from memory.
    pub async fn put_bytes(&self, data: &[u8], dest: &str, mode: &str) -> Result<()> {
        let tmp = std::env::temp_dir().join(format!("portside-node-{}-{}", std::process::id(), portside_core::now_ms()));
        tokio::fs::write(&tmp, data).await.map_err(|e| other(format!("couldn't write {}: {e}", tmp.display())))?;
        let result = self.put_file(&tmp, dest, mode).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        result
    }

    /// Have the running k3s import an image tarball now (it also imports
    /// everything in `agent/images` at every start).
    pub async fn import_image(&self, path: &str) -> Result<String> {
        self.sudo_text(&format!("sudo k3s ctr -n k8s.io images import {}", cb::sh_quote(path))).await
    }

    /// Restart k3s so it reads its config again.
    pub async fn restart_k3s(&self) -> Result<()> {
        self.sudo_text("sudo systemctl restart k3s").await.map(|_| ())
    }
}
