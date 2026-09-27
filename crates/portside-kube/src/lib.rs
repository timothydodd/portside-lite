//! Cluster access for Portside Lite. Both connection modes end in the same
//! `kube::Client`: local mode loads a kubeconfig from disk; SSH mode reads the
//! k3s kubeconfig over SSH and tunnels the API server port through the SSH
//! session, so every read, write and log call behaves identically.

pub mod actions;
pub mod config;
pub mod collect;
pub mod logs;
pub mod manifests;
pub mod portforward;
mod ssh;

use kube::config::{KubeConfigOptions, Kubeconfig};
use kube::{Client, Config};
use portside_core::Connection;

#[derive(Debug, thiserror::Error)]
pub enum KubeError {
    #[error("{0}")]
    Kube(#[from] kube::Error),
    #[error("kubeconfig: {0}")]
    Kubeconfig(#[from] kube::config::KubeconfigError),
    #[error("ssh: {0}")]
    Ssh(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, KubeError>;

/// A connected cluster. Keep it alive for as long as the client is in use —
/// in SSH mode dropping it closes the tunnel.
pub struct ClusterClient {
    pub client: Client,
    /// SSH mode: host key fingerprint presented by the server. The caller pins
    /// it into settings on first connect.
    pub host_key_fingerprint: Option<String>,
    _tunnel: Option<ssh::Tunnel>,
}

pub async fn connect(conn: &Connection) -> Result<ClusterClient> {
    match conn {
        Connection::Local { kubeconfig_path, context } => {
            let kc = match kubeconfig_path.as_deref().filter(|p| !p.trim().is_empty()) {
                Some(p) => Kubeconfig::read_from(p)?,
                None => Kubeconfig::read()?,
            };
            let opts = KubeConfigOptions {
                context: context.clone().filter(|c| !c.is_empty()),
                ..Default::default()
            };
            let config = Config::from_custom_kubeconfig(kc, &opts).await?;
            Ok(ClusterClient {
                client: Client::try_from(config)?,
                host_key_fingerprint: None,
                _tunnel: None,
            })
        }
        Connection::Ssh(cfg) => {
            let session = ssh::Session::connect(cfg).await?;
            let sudo_password = cfg.effective_sudo_password();
            let command = sudo_command(&cfg.kubeconfig_command, sudo_password.is_some());
            let yaml = session.exec(&command, sudo_password).await.map_err(|e| {
                let msg = e.to_string();
                if msg.contains("password is required") || msg.contains("incorrect password") || msg.contains("Sorry, try again") {
                    KubeError::Ssh(format!(
                        "sudo on {} needs a password{}. Enter it under Settings → this connection → Sudo password, \
                         or allow passwordless sudo for the kubeconfig command, or start k3s with --write-kubeconfig-mode 644.",
                        cfg.host,
                        if sudo_password.is_some() { " and the one provided was rejected" } else { "" }
                    ))
                } else {
                    e
                }
            })?;
            let kc = Kubeconfig::from_yaml(&yaml).map_err(|e| {
                KubeError::Other(format!(
                    "couldn't parse the kubeconfig printed by `{}`: {e}",
                    cfg.kubeconfig_command
                ))
            })?;
            let fingerprint = session.fingerprint.clone();
            let tunnel = session.tunnel(cfg.api_host.clone(), cfg.api_port).await?;
            let mut config = Config::from_custom_kubeconfig(kc, &KubeConfigOptions::default()).await?;
            // The k3s serving cert covers 127.0.0.1, and we reach the API server
            // through a local listener on 127.0.0.1.
            config.cluster_url = format!("https://127.0.0.1:{}", tunnel.local_port)
                .parse()
                .map_err(|e| KubeError::Other(format!("tunnel url: {e}")))?;
            config.connect_timeout = Some(std::time::Duration::from_secs(15));
            Ok(ClusterClient {
                client: Client::try_from(config)?,
                host_key_fingerprint: Some(fingerprint),
                _tunnel: Some(tunnel),
            })
        }
    }
}

/// When a sudo password is available, rewrite a leading `sudo [-n] …` to read
/// the password from stdin silently (`sudo -S -p '' …`). `-n` (never prompt)
/// is dropped because it would refuse the password. Other commands pass
/// through unchanged.
pub fn sudo_command(command: &str, has_password: bool) -> String {
    let trimmed = command.trim_start();
    let Some(rest) = trimmed.strip_prefix("sudo ") else { return command.to_string() };
    if !has_password {
        return command.to_string();
    }
    let args: Vec<&str> = rest.split_whitespace().filter(|a| *a != "-n" && *a != "-S").collect();
    format!("sudo -S -p '' {}", args.join(" "))
}

/// Contexts in a local kubeconfig and which one is current, for the settings UI.
pub fn list_contexts(kubeconfig_path: Option<&str>) -> Result<(Vec<String>, Option<String>)> {
    let kc = match kubeconfig_path.filter(|p| !p.trim().is_empty()) {
        Some(p) => Kubeconfig::read_from(p)?,
        None => Kubeconfig::read()?,
    };
    Ok((
        kc.contexts.iter().map(|c| c.name.clone()).collect(),
        kc.current_context,
    ))
}

/// Connect and make one cheap call, returning the server version.
pub async fn test_connection(conn: &Connection) -> Result<(String, Option<String>)> {
    let cc = connect(conn).await?;
    let info = cc.client.apiserver_version().await?;
    Ok((info.git_version, cc.host_key_fingerprint))
}

#[cfg(test)]
mod tests {
    use super::sudo_command;

    #[test]
    fn sudo_rewrite() {
        let default = "sudo -n cat /etc/rancher/k3s/k3s.yaml";
        assert_eq!(sudo_command(default, false), default, "no password → untouched");
        assert_eq!(sudo_command(default, true), "sudo -S -p '' cat /etc/rancher/k3s/k3s.yaml");
        assert_eq!(sudo_command("sudo k3s kubectl config view --raw", true), "sudo -S -p '' k3s kubectl config view --raw");
        assert_eq!(sudo_command("cat ~/k3s.yaml", true), "cat ~/k3s.yaml", "non-sudo commands pass through");
    }
}
