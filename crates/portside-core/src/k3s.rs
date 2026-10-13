//! Installing k3s on an SSH host: what a host probe reports, the install
//! options and the commands they turn into. Pure; the SSH side is in
//! `portside_kube::k3s`.

use serde::{Deserialize, Serialize};

/// Shell script run on the host (as the login user) to see what's there.
/// Every line is `key=value`; see [`parse_probe`].
pub const PROBE_SCRIPT: &str = r#"echo "k3s=$(command -v k3s 2>/dev/null || true)"
echo "version=$(k3s --version 2>/dev/null | head -n 1)"
echo "active=$(systemctl is-active k3s 2>/dev/null || true)"
if [ -r /etc/rancher/k3s/k3s.yaml ]; then echo "kubeconfig=readable"; elif [ -e /etc/rancher/k3s/k3s.yaml ]; then echo "kubeconfig=root-only"; else echo "kubeconfig=missing"; fi
echo "os=$( ( . /etc/os-release 2>/dev/null && printf '%s' "$PRETTY_NAME" ) )"
echo "arch=$(uname -m)"
if command -v apt-get >/dev/null 2>&1; then echo "pm=apt"; elif command -v dnf >/dev/null 2>&1; then echo "pm=dnf"; elif command -v yum >/dev/null 2>&1; then echo "pm=yum"; elif command -v zypper >/dev/null 2>&1; then echo "pm=zypper"; elif command -v apk >/dev/null 2>&1; then echo "pm=apk"; else echo "pm="; fi
echo "curl=$(command -v curl 2>/dev/null || true)"
echo "systemd=$(command -v systemctl 2>/dev/null || true)"
"#;

/// What the probe found on the host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostProbe {
    pub k3s_installed: bool,
    /// `k3s --version`'s first line, e.g. "k3s version v1.31.4+k3s1 (…)".
    pub k3s_version: Option<String>,
    /// systemd says the k3s service is active.
    pub k3s_running: bool,
    /// `readable` (no sudo needed), `root-only`, or `missing`.
    pub kubeconfig: String,
    pub os: String,
    pub arch: String,
    /// apt | dnf | yum | zypper | apk, or none recognised.
    pub package_manager: Option<String>,
    pub has_curl: bool,
    pub has_systemd: bool,
    /// `sudo` works for this login (passwordless, or with the saved password).
    pub sudo_ok: bool,
    pub host_key_fingerprint: Option<String>,
}

/// Turn the probe script's output into a [`HostProbe`] (`sudo_ok` and the
/// fingerprint are filled in by the caller).
pub fn parse_probe(output: &str) -> HostProbe {
    let get = |key: &str| {
        output
            .lines()
            .find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    HostProbe {
        k3s_installed: get("k3s").is_some(),
        k3s_version: get("version"),
        k3s_running: get("active").as_deref() == Some("active"),
        kubeconfig: get("kubeconfig").unwrap_or_else(|| "missing".into()),
        os: get("os").unwrap_or_else(|| "Linux".into()),
        arch: get("arch").unwrap_or_default(),
        package_manager: get("pm"),
        has_curl: get("curl").is_some(),
        has_systemd: get("systemd").is_some(),
        sudo_ok: false,
        host_key_fingerprint: None,
    }
}

/// What to install. Each field maps to one installer flag; the UI explains them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct K3sInstallOptions {
    /// Update the OS packages first.
    pub update_system: bool,
    /// `stable` or `latest` (INSTALL_K3S_CHANNEL), unless `version` is set.
    pub channel: String,
    /// A specific release, e.g. `v1.31.4+k3s1` (INSTALL_K3S_VERSION).
    #[serde(default)]
    pub version: Option<String>,
    /// `--write-kubeconfig-mode 644`: readable without sudo.
    pub kubeconfig_readable: bool,
    pub traefik: bool,
    pub servicelb: bool,
    pub local_storage: bool,
    pub metrics_server: bool,
    /// `--secrets-encryption`.
    pub secrets_encryption: bool,
    /// `--cluster-init`: embedded etcd instead of SQLite.
    pub cluster_init: bool,
    /// `--tls-san`: another name or address for the API certificate.
    #[serde(default)]
    pub tls_san: Option<String>,
    /// `--node-name`.
    #[serde(default)]
    pub node_name: Option<String>,
}

impl Default for K3sInstallOptions {
    fn default() -> Self {
        Self {
            update_system: true,
            channel: "stable".into(),
            version: None,
            kubeconfig_readable: true,
            traefik: true,
            servicelb: true,
            local_storage: true,
            metrics_server: true,
            secrets_encryption: false,
            cluster_init: false,
            tls_san: None,
            node_name: None,
        }
    }
}

fn clean(v: Option<&str>) -> Option<String> {
    v.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
}

/// Only what can sit on a command line unquoted: letters, digits, `.-_+:`.
fn plain(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | ':'))
}

/// Why the options can't be turned into a command, if they can't.
pub fn validate(opts: &K3sInstallOptions) -> Result<(), String> {
    if !matches!(opts.channel.as_str(), "stable" | "latest") {
        return Err("The channel must be stable or latest.".into());
    }
    if let Some(v) = clean(opts.version.as_deref()) {
        if !v.starts_with('v') || !plain(&v) {
            return Err("A version looks like v1.31.4+k3s1.".into());
        }
    }
    if let Some(s) = clean(opts.tls_san.as_deref()) {
        if !plain(&s) {
            return Err("The extra TLS name must be a hostname or IP address.".into());
        }
    }
    if let Some(n) = clean(opts.node_name.as_deref()) {
        if !plain(&n) || n.contains(':') || n.contains('+') {
            return Err("The node name must be letters, digits, '-' or '.'.".into());
        }
    }
    Ok(())
}

/// The installer's server flags for these options.
pub fn server_args(opts: &K3sInstallOptions) -> Vec<String> {
    let mut args = vec!["server".to_string()];
    if opts.kubeconfig_readable {
        args.extend(["--write-kubeconfig-mode".into(), "644".into()]);
    }
    for (on, component) in [(opts.traefik, "traefik"), (opts.servicelb, "servicelb"), (opts.local_storage, "local-storage"), (opts.metrics_server, "metrics-server")] {
        if !on {
            args.extend(["--disable".into(), component.into()]);
        }
    }
    if opts.secrets_encryption {
        args.push("--secrets-encryption".into());
    }
    if opts.cluster_init {
        args.push("--cluster-init".into());
    }
    if let Some(s) = clean(opts.tls_san.as_deref()) {
        args.extend(["--tls-san".into(), s]);
    }
    if let Some(n) = clean(opts.node_name.as_deref()) {
        args.extend(["--node-name".into(), n]);
    }
    args
}

/// The install command, run with sudo: the official script piped to sh.
/// Call [`validate`] first.
pub fn install_command(opts: &K3sInstallOptions) -> String {
    let env = match clean(opts.version.as_deref()) {
        Some(v) => format!("INSTALL_K3S_VERSION={v}"),
        None => format!("INSTALL_K3S_CHANNEL={}", opts.channel),
    };
    format!("sudo -n sh -c 'curl -sfL https://get.k3s.io | {env} sh -s - {}'", server_args(opts).join(" "))
}

/// Bring the OS packages up to date and make sure curl is there, for the
/// host's package manager. `None` for one we don't know.
pub fn update_command(package_manager: &str) -> Option<String> {
    let body = match package_manager {
        "apt" => "export DEBIAN_FRONTEND=noninteractive; apt-get update -q && apt-get upgrade -y -q && apt-get install -y -q curl",
        "dnf" => "dnf upgrade -y && dnf install -y curl",
        "yum" => "yum update -y && yum install -y curl",
        "zypper" => "zypper --non-interactive refresh && zypper --non-interactive update && zypper --non-interactive install curl",
        "apk" => "apk update && apk upgrade && apk add curl",
        _ => return None,
    };
    Some(format!("sudo -n sh -c '{body}'"))
}

/// Just install curl (the installer needs it).
pub fn curl_command(package_manager: &str) -> Option<String> {
    let body = match package_manager {
        "apt" => "export DEBIAN_FRONTEND=noninteractive; apt-get update -q && apt-get install -y -q curl",
        "dnf" => "dnf install -y curl",
        "yum" => "yum install -y curl",
        "zypper" => "zypper --non-interactive install curl",
        "apk" => "apk add curl",
        _ => return None,
    };
    Some(format!("sudo -n sh -c '{body}'"))
}

/// Node readiness, as `kubectl get nodes --no-headers` prints it.
pub const READY_COMMAND: &str = "sudo -n k3s kubectl get nodes --no-headers 2>/dev/null";

/// A node is Ready in `kubectl get nodes --no-headers` output.
pub fn node_ready(output: &str) -> bool {
    output.lines().any(|l| l.split_whitespace().nth(1).is_some_and(|s| s == "Ready" || s.starts_with("Ready,")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_output_parses() {
        let out = "k3s=/usr/local/bin/k3s\nversion=k3s version v1.31.4+k3s1 (abc)\nactive=active\nkubeconfig=root-only\nos=Ubuntu 24.04.1 LTS\narch=x86_64\npm=apt\ncurl=/usr/bin/curl\nsystemd=/usr/bin/systemctl\n";
        let p = parse_probe(out);
        assert!(p.k3s_installed && p.k3s_running);
        assert_eq!(p.k3s_version.as_deref(), Some("k3s version v1.31.4+k3s1 (abc)"));
        assert_eq!((p.kubeconfig.as_str(), p.os.as_str(), p.arch.as_str()), ("root-only", "Ubuntu 24.04.1 LTS", "x86_64"));
        assert_eq!(p.package_manager.as_deref(), Some("apt"));
        assert!(p.has_curl && p.has_systemd);

        let bare = parse_probe("k3s=\nversion=\nactive=\nkubeconfig=missing\nos=\narch=aarch64\npm=\ncurl=\nsystemd=\n");
        assert!(!bare.k3s_installed && !bare.k3s_running && !bare.has_curl);
        assert_eq!(bare.os, "Linux");
        assert!(bare.package_manager.is_none());
    }

    #[test]
    fn options_become_flags() {
        let d = K3sInstallOptions::default();
        assert_eq!(install_command(&d), "sudo -n sh -c 'curl -sfL https://get.k3s.io | INSTALL_K3S_CHANNEL=stable sh -s - server --write-kubeconfig-mode 644'");
        let custom = K3sInstallOptions {
            version: Some("v1.31.4+k3s1".into()),
            kubeconfig_readable: false,
            traefik: false,
            servicelb: false,
            secrets_encryption: true,
            cluster_init: true,
            tls_san: Some(" k3s.example.com ".into()),
            node_name: Some("node-a".into()),
            ..d
        };
        assert!(validate(&custom).is_ok());
        assert_eq!(
            install_command(&custom),
            "sudo -n sh -c 'curl -sfL https://get.k3s.io | INSTALL_K3S_VERSION=v1.31.4+k3s1 sh -s - server --disable traefik --disable servicelb --secrets-encryption --cluster-init --tls-san k3s.example.com --node-name node-a'"
        );
    }

    #[test]
    fn options_are_checked_before_they_reach_a_shell() {
        let d = K3sInstallOptions::default();
        assert!(validate(&K3sInstallOptions { channel: "beta".into(), ..d.clone() }).is_err());
        assert!(validate(&K3sInstallOptions { version: Some("1.31".into()), ..d.clone() }).is_err());
        assert!(validate(&K3sInstallOptions { tls_san: Some("a;rm -rf /".into()), ..d.clone() }).is_err());
        assert!(validate(&K3sInstallOptions { tls_san: Some("$(x)".into()), ..d.clone() }).is_err());
        assert!(validate(&K3sInstallOptions { node_name: Some("bad name".into()), ..d.clone() }).is_err());
        assert!(validate(&K3sInstallOptions { tls_san: Some("".into()), version: Some("  ".into()), ..d }).is_ok(), "blank means unset");
    }

    #[test]
    fn update_commands_per_package_manager() {
        assert!(update_command("apt").unwrap().contains("apt-get upgrade -y"));
        assert!(update_command("apt").unwrap().starts_with("sudo -n sh -c '"));
        assert!(update_command("apk").unwrap().contains("apk upgrade"));
        assert!(update_command("pacman").is_none());
        assert!(curl_command("dnf").unwrap().contains("dnf install -y curl"));
    }

    #[test]
    fn node_readiness() {
        assert!(node_ready("k3s-1   Ready    control-plane,master   2m   v1.31.4+k3s1\n"));
        assert!(node_ready("k3s-1   Ready,SchedulingDisabled   control-plane   2m   v1\n"));
        assert!(!node_ready("k3s-1   NotReady   control-plane,master   2m   v1.31.4+k3s1\n"));
        assert!(!node_ready(""));
    }
}
