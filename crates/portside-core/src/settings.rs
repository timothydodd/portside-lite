//! User-configurable settings, persisted as JSON in the local store.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "camelCase")]
pub enum Connection {
    /// Use a kubeconfig on this machine (default `~/.kube/config` / `$KUBECONFIG`).
    #[serde(rename_all = "camelCase")]
    Local {
        kubeconfig_path: Option<String>,
        context: Option<String>,
        /// See [`SshConnection::partition`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partition: Option<String>,
    },
    /// SSH into a k3s server node, read its kubeconfig and tunnel the API
    /// server port over the SSH connection.
    #[serde(rename_all = "camelCase")]
    Ssh(SshConnection),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SshConnection {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: SshAuth,
    /// Command whose stdout is the cluster kubeconfig. k3s writes it as
    /// root-only, hence the non-interactive sudo default.
    #[serde(default = "default_kubeconfig_command")]
    pub kubeconfig_command: String,
    /// API server address as seen from the SSH host.
    #[serde(default = "default_api_host")]
    pub api_host: String,
    #[serde(default = "default_api_port")]
    pub api_port: u16,
    /// SHA-256 fingerprint of the host key, pinned on first connect (TOFU).
    #[serde(default)]
    pub host_key_fingerprint: Option<String>,
    /// Password for `sudo` in `kubeconfig_command`, sent on stdin. Falls back
    /// to the SSH login password when unset.
    #[serde(default)]
    pub sudo_password: Option<String>,
    /// Set by `Settings::normalize` when another profile would otherwise get
    /// the same cluster id for a different cluster; keeps their stored data
    /// apart. Never shown or edited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub partition: Option<String>,
}

impl SshConnection {
    /// The password to feed sudo: the explicit one, else the login password.
    pub fn effective_sudo_password(&self) -> Option<&str> {
        match (self.sudo_password.as_deref().filter(|p| !p.is_empty()), &self.auth) {
            (Some(p), _) => Some(p),
            (None, SshAuth::Password { password }) if !password.is_empty() => Some(password),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SshAuth {
    #[serde(rename_all = "camelCase")]
    Key {
        private_key_path: String,
        passphrase: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Password { password: String },
}

fn default_ssh_port() -> u16 {
    22
}
fn default_kubeconfig_command() -> String {
    "sudo -n cat /etc/rancher/k3s/k3s.yaml".into()
}
fn default_api_host() -> String {
    "127.0.0.1".into()
}
fn default_api_port() -> u16 {
    6443
}

impl Connection {
    /// Stable identifier used to partition stored samples/logs per cluster.
    pub fn cluster_id(&self) -> String {
        let base = match self {
            Connection::Local { context, .. } => {
                format!("local:{}", context.as_deref().unwrap_or("default"))
            }
            Connection::Ssh(s) => format!("ssh:{}@{}:{}", s.username, s.host, s.port),
        };
        match self.partition() {
            Some(p) => format!("{base}#{p}"),
            None => base,
        }
    }

    fn partition(&self) -> Option<&str> {
        match self {
            Connection::Local { partition, .. } => partition.as_deref(),
            Connection::Ssh(s) => s.partition.as_deref(),
        }
    }

    fn partition_mut(&mut self) -> &mut Option<String> {
        match self {
            Connection::Local { partition, .. } => partition,
            Connection::Ssh(s) => &mut s.partition,
        }
    }

    /// The parts of the target the cluster id leaves out: two connections with
    /// the same id that differ here are different clusters.
    fn id_blind_spot(&self) -> (Option<&str>, u16) {
        match self {
            Connection::Local { kubeconfig_path, .. } => (kubeconfig_path.as_deref().filter(|p| !p.is_empty()), 0),
            Connection::Ssh(s) => (Some(s.api_host.as_str()), s.api_port),
        }
    }

    /// Same cluster, reached the same way. A host key pinned after connecting
    /// doesn't make it a different connection.
    pub fn same_endpoint(&self, other: &Connection) -> bool {
        let unpinned = |c: &Connection| {
            let mut c = c.clone();
            if let Connection::Ssh(s) = &mut c {
                s.host_key_fingerprint = None;
            }
            c
        };
        unpinned(self) == unpinned(other)
    }
}

/// A saved, named connection. Only the active one is monitored.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionProfile {
    /// Stable identifier (generated by the UI).
    pub id: String,
    pub name: String,
    pub connection: Connection,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub connections: Vec<ConnectionProfile>,
    /// Which profile is monitored; `None` when there are no profiles.
    pub active_connection_id: Option<String>,
    /// Pre-profiles single connection. Read for migration only, never written.
    #[serde(skip_serializing)]
    pub connection: Option<Connection>,
    /// Seconds between cluster state + metrics polls.
    pub poll_interval_secs: u64,
    /// Pull container logs into the local store for search/analytics.
    pub collect_logs: bool,
    /// Seconds between log pulls.
    pub log_interval_secs: u64,
    /// How far back the first pull of a container's logs reaches.
    pub initial_log_lookback_hours: u32,
    /// Per-container cap on a single pull, so a noisy pod can't stall a cycle.
    pub max_log_bytes_per_pull: i64,
    /// Days of logs, metric samples and resolved issues to keep.
    pub retention_days: u32,
    /// Namespaces skipped by log collection (state is still monitored).
    pub excluded_namespaces: Vec<String>,
    /// Node/pod CPU or memory percentage that raises a warning.
    pub high_usage_percent: f64,
    /// Restart count at which a pod is flagged even if currently healthy.
    pub restart_warning_threshold: i32,
    /// Error-level log lines per hour that raise a log-spike issue.
    pub error_log_spike_per_hour: i64,
    /// Closing the window hides it to the system tray and keeps monitoring.
    pub close_to_tray: bool,
    /// Desktop notifications for new critical problems / unreachable clusters.
    pub notify_critical: bool,
    /// Also notify for new warning-level problems.
    pub notify_warnings: bool,
    /// Minutes between background health checks of the non-active connections.
    pub background_check_minutes: u32,
    /// All polling, log pulls and background checks stopped (tray toggle).
    pub monitoring_paused: bool,
    /// Where workload archives are kept; `None` = `archives` in the app's
    /// data folder.
    pub archive_dir: Option<String>,
    /// Image for the pod that mounts a volume for the file browser. Needs
    /// `sh`, `stat`, `head`, `tar` and `gzip` (busybox has them all).
    pub files_helper_image: String,
}

pub const DEFAULT_FILES_HELPER_IMAGE: &str = "busybox:1.37";

impl Default for Settings {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
            active_connection_id: None,
            connection: None,
            poll_interval_secs: 15,
            collect_logs: true,
            log_interval_secs: 60,
            initial_log_lookback_hours: 24,
            max_log_bytes_per_pull: 4 * 1024 * 1024,
            retention_days: 7,
            excluded_namespaces: Vec::new(),
            high_usage_percent: 90.0,
            restart_warning_threshold: 5,
            error_log_spike_per_hour: 50,
            close_to_tray: true,
            notify_critical: true,
            notify_warnings: false,
            background_check_minutes: 15,
            monitoring_paused: false,
            archive_dir: None,
            files_helper_image: DEFAULT_FILES_HELPER_IMAGE.into(),
        }
    }
}

impl Settings {
    pub fn active_profile(&self) -> Option<&ConnectionProfile> {
        let id = self.active_connection_id.as_deref()?;
        self.connections.iter().find(|p| p.id == id)
    }

    pub fn active_connection(&self) -> Option<&Connection> {
        self.active_profile().map(|p| &p.connection)
    }

    /// A partition is assigned once and then stays, even if the UI sends the
    /// profile back without it (e.g. after switching its mode and back).
    pub fn keep_partitions(&mut self, prev: &Settings) {
        for p in &mut self.connections {
            if p.connection.partition().is_none() {
                if let Some(old) = prev.connections.iter().find(|o| o.id == p.id).and_then(|o| o.connection.partition()) {
                    *p.connection.partition_mut() = Some(old.to_string());
                }
            }
        }
    }

    /// Fold a legacy single `connection` into a profile, keep the active id
    /// pointing at an existing profile (first one if it dangles), and give a
    /// profile its own partition when its cluster id would collide with an
    /// earlier profile for a different cluster.
    pub fn normalize(&mut self) {
        if let Some(legacy) = self.connection.take() {
            if self.connections.is_empty() {
                let name = match &legacy {
                    Connection::Local { context, .. } => format!("Local ({})", context.as_deref().unwrap_or("current context")),
                    Connection::Ssh(s) => s.host.clone(),
                };
                self.connections.push(ConnectionProfile { id: "default".into(), name, connection: legacy });
            }
        }
        // Profiles are looked up by id, so a blank or repeated one would hide a profile.
        let mut seen = std::collections::HashSet::new();
        for i in 0..self.connections.len() {
            let wanted = match self.connections[i].id.trim() {
                "" => "profile".to_string(),
                id => id.to_string(),
            };
            let mut id = wanted.clone();
            let mut n = 2;
            while !seen.insert(id.clone()) {
                id = format!("{wanted}-{n}");
                n += 1;
            }
            self.connections[i].id = id;
        }
        // Below these the loops spin or the API rejects the request.
        self.poll_interval_secs = self.poll_interval_secs.max(5);
        self.log_interval_secs = self.log_interval_secs.max(10);
        self.initial_log_lookback_hours = self.initial_log_lookback_hours.max(1);
        self.retention_days = self.retention_days.max(1);
        let valid = self
            .active_connection_id
            .as_deref()
            .is_some_and(|id| self.connections.iter().any(|p| p.id == id));
        if !valid {
            self.active_connection_id = self.connections.first().map(|p| p.id.clone());
        }
        for i in 1..self.connections.len() {
            let (earlier, rest) = self.connections.split_at_mut(i);
            let p = &mut rest[0];
            let id = p.connection.cluster_id();
            let clash = earlier
                .iter()
                .any(|e| e.connection.cluster_id() == id && e.connection.id_blind_spot() != p.connection.id_blind_spot());
            if clash {
                *p.connection.partition_mut() = Some(p.id.clone());
            }
        }
        if self.files_helper_image.trim().is_empty() {
            self.files_helper_image = DEFAULT_FILES_HELPER_IMAGE.into();
        }
    }
}

/// Stands in for a saved password, passphrase or sudo password in settings
/// sent to the UI, which never sees the real value. Sent back unchanged, it
/// means "keep what's saved" (see [`Settings::restore_secrets`]).
pub const SAVED_SECRET: &str = "__portside_saved__";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretField {
    Password,
    Passphrase,
    SudoPassword,
}

impl SecretField {
    pub const ALL: [SecretField; 3] = [SecretField::Password, SecretField::Passphrase, SecretField::SudoPassword];
}

impl Connection {
    /// The secret slot, if this connection has one set (an unset optional
    /// field, or a field of the other auth kind, has none).
    pub fn secret_mut(&mut self, which: SecretField) -> Option<&mut String> {
        let Connection::Ssh(s) = self else { return None };
        match (which, &mut s.auth) {
            (SecretField::Password, SshAuth::Password { password }) => Some(password),
            (SecretField::Passphrase, SshAuth::Key { passphrase, .. }) => passphrase.as_mut(),
            (SecretField::SudoPassword, _) => s.sudo_password.as_mut(),
            _ => None,
        }
    }

    fn secret(&self, which: SecretField) -> Option<&str> {
        let Connection::Ssh(s) = self else { return None };
        match (which, &s.auth) {
            (SecretField::Password, SshAuth::Password { password }) => Some(password),
            (SecretField::Passphrase, SshAuth::Key { passphrase, .. }) => passphrase.as_deref(),
            (SecretField::SudoPassword, _) => s.sudo_password.as_deref(),
            _ => None,
        }
    }

    /// Replace every non-empty secret with [`SAVED_SECRET`].
    pub fn redact(&mut self) {
        for f in SecretField::ALL {
            if let Some(v) = self.secret_mut(f).filter(|v| !v.is_empty()) {
                *v = SAVED_SECRET.into();
            }
        }
    }

    /// Put back the secrets the UI left as [`SAVED_SECRET`], from `prev` (this
    /// connection as saved). Without a saved value to take, the field is cleared.
    pub fn restore_secrets(&mut self, prev: Option<&Connection>) {
        for f in SecretField::ALL {
            if let Some(v) = self.secret_mut(f).filter(|v| *v == SAVED_SECRET) {
                *v = prev.and_then(|p| p.secret(f)).filter(|p| *p != SAVED_SECRET).unwrap_or_default().to_string();
            }
        }
        // An empty optional secret means none.
        if let Connection::Ssh(s) = self {
            if let SshAuth::Key { passphrase, .. } = &mut s.auth {
                if passphrase.as_deref() == Some("") {
                    *passphrase = None;
                }
            }
            if s.sudo_password.as_deref() == Some("") {
                s.sudo_password = None;
            }
        }
    }
}

impl Settings {
    /// A copy safe to hand to the UI: every saved secret is [`SAVED_SECRET`].
    pub fn redacted(&self) -> Settings {
        let mut s = self.clone();
        for p in &mut s.connections {
            p.connection.redact();
        }
        s
    }

    /// Settings back from the UI: fill in secrets it left as [`SAVED_SECRET`]
    /// from the same profile (by id) in `prev`.
    pub fn restore_secrets(&mut self, prev: &Settings) {
        for p in &mut self.connections {
            let old = prev.connections.iter().find(|o| o.id == p.id).map(|o| &o.connection);
            p.connection.restore_secrets(old);
        }
    }

    /// Every set secret, for encrypting before the settings are stored.
    pub fn secrets_mut(&mut self) -> Vec<&mut String> {
        let mut out = Vec::new();
        for p in &mut self.connections {
            let Connection::Ssh(s) = &mut p.connection else { continue };
            match &mut s.auth {
                SshAuth::Password { password } => out.push(password),
                SshAuth::Key { passphrase, .. } => out.extend(passphrase.as_mut()),
            }
            out.extend(s.sudo_password.as_mut());
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ssh(id: &str, auth: SshAuth, sudo: Option<&str>) -> ConnectionProfile {
        let json = r#"{"mode":"ssh","host":"h","username":"u","auth":{"kind":"password","password":""}}"#;
        let Connection::Ssh(mut c) = serde_json::from_str(json).unwrap() else { panic!() };
        c.auth = auth;
        c.sudo_password = sudo.map(Into::into);
        ConnectionProfile { id: id.into(), name: id.into(), connection: Connection::Ssh(c) }
    }

    fn pw(p: &str) -> SshAuth {
        SshAuth::Password { password: p.into() }
    }

    fn key(passphrase: Option<&str>) -> SshAuth {
        SshAuth::Key { private_key_path: "k".into(), passphrase: passphrase.map(Into::into) }
    }

    #[test]
    fn redacted_settings_carry_no_secrets() {
        let s = Settings {
            connections: vec![ssh("a", pw("login"), Some("sudo")), ssh("b", key(Some("phrase")), None), ssh("c", pw(""), Some(""))],
            ..Default::default()
        };
        let json = serde_json::to_string(&s.redacted()).unwrap();
        for secret in ["login", "sudo", "phrase"] {
            assert!(!json.contains(&format!("\"{secret}\"")), "{secret} leaked: {json}");
        }
        let r = s.redacted();
        assert_eq!(r.connections[0].connection, ssh("a", pw(SAVED_SECRET), Some(SAVED_SECRET)).connection);
        assert_eq!(r.connections[2].connection, s.connections[2].connection, "empty stays empty, so the UI shows 'none'");
    }

    #[test]
    fn restore_keeps_saved_secrets_and_takes_new_ones() {
        let prev = Settings {
            connections: vec![ssh("a", pw("login"), Some("sudo")), ssh("b", key(Some("phrase")), None)],
            ..Default::default()
        };
        let mut back = prev.redacted();
        // Profile a: new sudo password typed; profile b: untouched.
        if let Connection::Ssh(c) = &mut back.connections[0].connection {
            c.sudo_password = Some("new-sudo".into());
        }
        back.restore_secrets(&prev);
        assert_eq!(back.connections[0].connection, ssh("a", pw("login"), Some("new-sudo")).connection);
        assert_eq!(back.connections[1].connection, prev.connections[1].connection);
    }

    #[test]
    fn restore_never_moves_a_secret_to_another_profile_or_field() {
        let prev = Settings { connections: vec![ssh("a", pw("login"), None)], ..Default::default() };
        // A new profile can't borrow a's password, and switching a to key auth
        // doesn't turn its password into a passphrase.
        let mut back = Settings {
            connections: vec![ssh("a", key(Some(SAVED_SECRET)), Some(SAVED_SECRET)), ssh("new", pw(SAVED_SECRET), None)],
            ..Default::default()
        };
        back.restore_secrets(&prev);
        assert_eq!(back.connections[0].connection, ssh("a", key(None), None).connection);
        assert_eq!(back.connections[1].connection, ssh("new", pw(""), None).connection);
    }

    #[test]
    fn secrets_mut_finds_every_set_secret() {
        let mut s = Settings {
            connections: vec![ssh("a", pw("login"), Some("sudo")), ssh("b", key(Some("phrase")), None), ssh("c", key(None), None), local("l", None)],
            ..Default::default()
        };
        let mut found: Vec<String> = s.secrets_mut().into_iter().map(|v| v.clone()).collect();
        found.sort();
        assert_eq!(found, ["login", "phrase", "sudo"]);
    }

    #[test]
    fn ssh_defaults_fill_in() {
        let json = r#"{"mode":"ssh","host":"h","username":"u","auth":{"kind":"password","password":"p"}}"#;
        let c: Connection = serde_json::from_str(json).unwrap();
        let Connection::Ssh(s) = &c else { panic!() };
        assert_eq!(s.port, 22);
        assert_eq!(s.api_port, 6443);
        assert_eq!(c.cluster_id(), "ssh:u@h:22");
    }

    fn local(id: &str, path: Option<&str>) -> ConnectionProfile {
        ConnectionProfile {
            id: id.into(),
            name: id.into(),
            connection: Connection::Local { kubeconfig_path: path.map(Into::into), context: None, partition: None },
        }
    }

    #[test]
    fn colliding_profiles_get_their_own_partition() {
        let mut s = Settings::default();
        s.connections = vec![local("a", Some("/k/one.yaml")), local("b", Some("/k/two.yaml")), local("c", Some("/k/one.yaml"))];
        s.normalize();
        let ids: Vec<String> = s.connections.iter().map(|p| p.connection.cluster_id()).collect();
        assert_eq!(ids, ["local:default", "local:default#b", "local:default"], "first keeps its id; same file = same cluster");

        // Stable across saves, including one where the UI dropped the field.
        let mut next = s.clone();
        next.connections[1] = local("b", Some("/k/two.yaml"));
        next.connections.remove(0);
        next.keep_partitions(&s);
        next.normalize();
        assert_eq!(next.connections[0].connection.cluster_id(), "local:default#b");

        // Old settings without the field still load, and the field isn't written when unset.
        let json = serde_json::to_string(&local("a", None).connection).unwrap();
        assert!(!json.contains("partition"));
        assert_eq!(serde_json::from_str::<Connection>(&json).unwrap().cluster_id(), "local:default");
    }

    #[test]
    fn blank_and_repeated_profile_ids_are_repaired() {
        let mut s = Settings::default();
        s.connections = vec![local("a", None), local("a", None), local("", None)];
        s.poll_interval_secs = 0;
        s.normalize();
        let ids: Vec<&str> = s.connections.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "a-2", "profile"]);
        assert_eq!(s.poll_interval_secs, 5);
    }

    #[test]
    fn a_pinned_host_key_is_the_same_endpoint() {
        let json = r#"{"mode":"ssh","host":"h","username":"u","auth":{"kind":"password","password":"p"}}"#;
        let a: Connection = serde_json::from_str(json).unwrap();
        let mut b = a.clone();
        if let Connection::Ssh(s) = &mut b {
            s.host_key_fingerprint = Some("SHA256:x".into());
        }
        assert!(a.same_endpoint(&b));
        if let Connection::Ssh(s) = &mut b {
            s.api_port = 7443;
        }
        assert!(!a.same_endpoint(&b));
    }

    #[test]
    fn settings_tolerate_missing_fields() {
        let s: Settings = serde_json::from_str(r#"{"pollIntervalSecs":5}"#).unwrap();
        assert_eq!(s.poll_interval_secs, 5);
        assert_eq!(s.retention_days, 7);
    }

    #[test]
    fn legacy_connection_migrates_to_active_profile() {
        let json = r#"{"connection":{"mode":"ssh","host":"k3s","username":"u","auth":{"kind":"password","password":"p"}}}"#;
        let mut s: Settings = serde_json::from_str(json).unwrap();
        s.normalize();
        assert_eq!(s.connections.len(), 1);
        assert_eq!(s.connections[0].name, "k3s");
        assert_eq!(s.active_connection_id.as_deref(), Some("default"));
        assert!(matches!(s.active_connection(), Some(Connection::Ssh(_))));
        // legacy field is never written back
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("connection").is_none());
    }

    #[test]
    fn dangling_active_id_falls_back_to_first() {
        let mut s = Settings::default();
        s.connections.push(ConnectionProfile {
            id: "a".into(),
            name: "A".into(),
            connection: Connection::Local { kubeconfig_path: None, context: None, partition: None },
        });
        s.active_connection_id = Some("deleted".into());
        s.normalize();
        assert_eq!(s.active_connection_id.as_deref(), Some("a"));
        s.connections.clear();
        s.normalize();
        assert_eq!(s.active_connection_id, None);
    }

    #[test]
    fn sudo_password_falls_back_to_login_password() {
        let json = r#"{"host":"h","username":"u","auth":{"kind":"password","password":"login"}}"#;
        let mut c: SshConnection = serde_json::from_str(json).unwrap();
        assert_eq!(c.effective_sudo_password(), Some("login"));
        c.sudo_password = Some("sudo-pw".into());
        assert_eq!(c.effective_sudo_password(), Some("sudo-pw"));
        c.sudo_password = Some(String::new());
        c.auth = SshAuth::Key { private_key_path: "k".into(), passphrase: None };
        assert_eq!(c.effective_sudo_password(), None, "key auth + blank → passwordless sudo");
    }
}
