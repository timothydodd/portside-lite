//! Image registries: which host an image comes from, which hosts a pull
//! secret holds credentials for, and building a pull secret.

use base64::Engine as _;
use serde_json::{json, Value};

/// Secret types that hold registry credentials.
pub const DOCKER_CONFIG_JSON: &str = "kubernetes.io/dockerconfigjson";
pub const DOCKER_CFG: &str = "kubernetes.io/dockercfg";

const DOCKER_HUB: &str = "docker.io";

/// One spelling per registry: no scheme or path, Docker Hub's aliases
/// folded into `docker.io`, lowercase.
pub fn normalize_host(raw: &str) -> String {
    let s = raw.trim();
    let s = s.split_once("://").map_or(s, |(_, rest)| rest);
    let host = s.split('/').next().unwrap_or_default().to_ascii_lowercase();
    match host.as_str() {
        "index.docker.io" | "registry-1.docker.io" | "registry.hub.docker.com" | "docker.io" => DOCKER_HUB.into(),
        _ => host,
    }
}

/// Split an image reference into (registry host, rest), the way the
/// container runtime does: the first path segment is a host only if it has
/// a dot or a port, or is `localhost`; otherwise it's Docker Hub.
pub fn split_image(image: &str) -> (String, String) {
    let image = image.trim();
    if let Some((first, rest)) = image.split_once('/') {
        if first.contains('.') || first.contains(':') || first == "localhost" {
            return (normalize_host(first), rest.to_string());
        }
    }
    (DOCKER_HUB.into(), image.to_string())
}

/// Registry hosts in a pull secret's `.dockerconfigjson` (or `.dockercfg`)
/// payload, each with the username stored for it.
pub fn credential_hosts(secret_type: &str, payload: &[u8]) -> Vec<(String, Option<String>)> {
    let Ok(v) = serde_json::from_slice::<Value>(payload) else { return Vec::new() };
    let auths = match secret_type {
        DOCKER_CONFIG_JSON => v.get("auths").cloned(),
        DOCKER_CFG => Some(v),
        _ => None,
    };
    let Some(Value::Object(auths)) = auths else { return Vec::new() };
    let mut out: Vec<(String, Option<String>)> = auths
        .iter()
        .map(|(host, entry)| (normalize_host(host), username(entry)))
        .filter(|(h, _)| !h.is_empty())
        .collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn username(entry: &Value) -> Option<String> {
    if let Some(u) = entry.get("username").and_then(Value::as_str).filter(|u| !u.is_empty()) {
        return Some(u.to_string());
    }
    let auth = entry.get("auth").and_then(Value::as_str)?;
    let decoded = base64::engine::general_purpose::STANDARD.decode(auth.trim()).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.split_once(':').map(|(u, _)| u.to_string()).filter(|u| !u.is_empty())
}

/// Registry hosts a Secret (as JSON, `data` base64-encoded) holds
/// credentials for; empty for anything that isn't a pull secret.
pub fn secret_pull_hosts(secret: &Value) -> Vec<String> {
    let secret_type = secret.get("type").and_then(Value::as_str).unwrap_or_default();
    let key = match secret_type {
        DOCKER_CONFIG_JSON => ".dockerconfigjson",
        DOCKER_CFG => ".dockercfg",
        _ => return Vec::new(),
    };
    let Some(b64) = secret.pointer(&format!("/data/{key}")).and_then(Value::as_str) else { return Vec::new() };
    let Ok(payload) = base64::engine::general_purpose::STANDARD.decode(b64.trim()) else { return Vec::new() };
    credential_hosts(secret_type, &payload).into_iter().map(|(h, _)| h).collect()
}

/// What a new pull secret needs.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCredentialInput {
    pub namespace: String,
    pub name: String,
    pub server: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub email: Option<String>,
}

/// Why the input can't become a Secret, if it can't.
pub fn validate_credential(input: &RegistryCredentialInput) -> Result<(), String> {
    let dns_label = |s: &str| {
        !s.is_empty()
            && s.len() <= 253
            && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
            && s.starts_with(|c: char| c.is_ascii_alphanumeric())
            && s.ends_with(|c: char| c.is_ascii_alphanumeric())
    };
    if !dns_label(&input.name) {
        return Err("The name must be lowercase letters, digits, '-' or '.', starting and ending with a letter or digit.".into());
    }
    if !dns_label(&input.namespace) {
        return Err("Pick a namespace.".into());
    }
    if normalize_host(&input.server).is_empty() {
        return Err("Enter the registry server, e.g. ghcr.io.".into());
    }
    if input.username.is_empty() || input.password.is_empty() {
        return Err("A username and password (or token) are required.".into());
    }
    Ok(())
}

/// A `kubernetes.io/dockerconfigjson` Secret, the same shape as
/// `kubectl create secret docker-registry`. Docker Hub is keyed by its
/// legacy index URL, which is what the runtime looks up.
pub fn pull_secret(input: &RegistryCredentialInput) -> Value {
    let host = normalize_host(&input.server);
    let key = if host == DOCKER_HUB { "https://index.docker.io/v1/".to_string() } else { host };
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let mut entry = json!({
        "username": input.username,
        "password": input.password,
        "auth": b64(&format!("{}:{}", input.username, input.password)),
    });
    if let Some(email) = input.email.as_deref().filter(|e| !e.is_empty()) {
        entry["email"] = json!(email);
    }
    let config = json!({ "auths": { key: entry } }).to_string();
    json!({
        "apiVersion": "v1",
        "kind": "Secret",
        "type": DOCKER_CONFIG_JSON,
        "metadata": { "name": input.name, "namespace": input.namespace },
        "data": { ".dockerconfigjson": b64(&config) },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_resolve_to_their_registry() {
        assert_eq!(split_image("nginx:1.27"), ("docker.io".into(), "nginx:1.27".into()));
        assert_eq!(split_image("bitnami/redis"), ("docker.io".into(), "bitnami/redis".into()));
        assert_eq!(split_image("ghcr.io/org/app:v1"), ("ghcr.io".into(), "org/app:v1".into()));
        assert_eq!(split_image("registry.local:5000/app"), ("registry.local:5000".into(), "app".into()));
        assert_eq!(split_image("localhost/app"), ("localhost".into(), "app".into()));
        assert_eq!(split_image("docker.io/library/nginx"), ("docker.io".into(), "library/nginx".into()));
        assert_eq!(split_image("index.docker.io/x/y"), ("docker.io".into(), "x/y".into()));
    }

    #[test]
    fn hosts_normalize() {
        assert_eq!(normalize_host("https://index.docker.io/v1/"), "docker.io");
        assert_eq!(normalize_host("https://GHCR.io"), "ghcr.io");
        assert_eq!(normalize_host("registry.local:5000"), "registry.local:5000");
    }

    #[test]
    fn reads_hosts_and_usernames_from_pull_secrets() {
        let auth = base64::engine::general_purpose::STANDARD.encode("bob:hunter2");
        let payload = json!({ "auths": {
            "https://index.docker.io/v1/": { "auth": auth },
            "ghcr.io": { "username": "alice", "password": "x" },
        } })
        .to_string();
        assert_eq!(
            credential_hosts(DOCKER_CONFIG_JSON, payload.as_bytes()),
            vec![("docker.io".into(), Some("bob".into())), ("ghcr.io".into(), Some("alice".into()))]
        );
        let legacy = json!({ "quay.io": { "auth": "" } }).to_string();
        assert_eq!(credential_hosts(DOCKER_CFG, legacy.as_bytes()), vec![("quay.io".into(), None)]);
        assert!(credential_hosts("Opaque", payload.as_bytes()).is_empty());
        assert!(credential_hosts(DOCKER_CONFIG_JSON, b"not json").is_empty());
    }

    #[test]
    fn pull_hosts_read_from_a_secret_object() {
        let input = RegistryCredentialInput { namespace: "a".into(), name: "s".into(), server: "ghcr.io".into(), username: "u".into(), password: "p".into(), email: None };
        assert_eq!(secret_pull_hosts(&pull_secret(&input)), vec!["ghcr.io".to_string()]);
        assert!(secret_pull_hosts(&json!({ "type": "Opaque", "data": { "x": "eQ==" } })).is_empty());
        assert!(secret_pull_hosts(&json!({ "type": DOCKER_CONFIG_JSON, "data": { ".dockerconfigjson": "!!" } })).is_empty());
    }

    #[test]
    fn built_pull_secret_reads_back() {
        let input = RegistryCredentialInput {
            namespace: "apps".into(),
            name: "ghcr-pull".into(),
            server: "https://ghcr.io".into(),
            username: "alice".into(),
            password: "tok".into(),
            email: None,
        };
        assert!(validate_credential(&input).is_ok());
        let s = pull_secret(&input);
        assert_eq!(s["type"], DOCKER_CONFIG_JSON);
        let raw = base64::engine::general_purpose::STANDARD.decode(s["data"][".dockerconfigjson"].as_str().unwrap()).unwrap();
        assert_eq!(credential_hosts(DOCKER_CONFIG_JSON, &raw), vec![("ghcr.io".into(), Some("alice".into()))]);

        let hub = pull_secret(&RegistryCredentialInput { server: "docker.io".into(), ..input.clone() });
        let raw = base64::engine::general_purpose::STANDARD.decode(hub["data"][".dockerconfigjson"].as_str().unwrap()).unwrap();
        assert!(String::from_utf8(raw).unwrap().contains("https://index.docker.io/v1/"));

        assert!(validate_credential(&RegistryCredentialInput { name: "Bad_Name".into(), ..input.clone() }).is_err());
        assert!(validate_credential(&RegistryCredentialInput { password: String::new(), ..input }).is_err());
    }
}
