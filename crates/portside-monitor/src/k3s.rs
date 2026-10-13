//! Probe an SSH host for k3s and install it there, with the installer's
//! output pushed to the UI as it arrives.

use portside_core::k3s::{HostProbe, K3sInstallOptions};
use portside_core::Connection;
use serde::Serialize;

use crate::Monitor;

/// A piece of installer output for the UI.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct K3sInstallLog {
    pub job_id: String,
    pub text: String,
}

fn ssh(connection: &Connection) -> Result<&portside_core::SshConnection, String> {
    match connection {
        Connection::Ssh(cfg) => Ok(cfg),
        Connection::Local { .. } => Err("Only SSH connections can be checked for k3s.".into()),
    }
}

impl Monitor {
    /// Saved secrets back into a connection the UI sends (it only ever sees
    /// placeholders).
    fn with_secrets(&self, mut connection: Connection, profile_id: Option<&str>) -> Connection {
        let saved = self.settings();
        connection.restore_secrets(saved.connections.iter().find(|p| Some(p.id.as_str()) == profile_id).map(|p| &p.connection));
        connection
    }

    /// What's on the host: k3s, its state, the OS, sudo.
    pub async fn probe_k3s(&self, connection: Connection, profile_id: Option<&str>) -> Result<HostProbe, String> {
        let connection = self.with_secrets(connection, profile_id);
        let cfg = ssh(&connection)?;
        let probe = portside_kube::k3s::probe(cfg).await.map_err(|e| e.to_string())?;
        self.pin_host_key(&connection, probe.host_key_fingerprint.as_deref()).await;
        Ok(probe)
    }

    /// Install k3s on the host, streaming output as `k3s:install` events
    /// tagged with `job_id`.
    pub async fn install_k3s(&self, connection: Connection, profile_id: Option<&str>, options: K3sInstallOptions, job_id: String) -> Result<HostProbe, String> {
        let connection = self.with_secrets(connection, profile_id);
        let cfg = ssh(&connection)?;
        let sink = std::sync::Arc::clone(&self.sink);
        let mut log = move |text: &str| sink.k3s_install_log(&K3sInstallLog { job_id: job_id.clone(), text: text.to_string() });
        let probe = portside_kube::k3s::install(cfg, &options, &mut log).await.map_err(|e| e.to_string())?;
        self.pin_host_key(&connection, probe.host_key_fingerprint.as_deref()).await;
        Ok(probe)
    }
}
