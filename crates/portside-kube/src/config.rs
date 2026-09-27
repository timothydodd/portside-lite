//! Reading and saving ConfigMaps / Secrets as key/value entries for the
//! config editor. Secret values are decoded to text when they're valid UTF-8;
//! binary entries are shown read-only and preserved on save.

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{ConfigMap, Secret};
use k8s_openapi::ByteString;
use kube::api::{Api, PostParams};
use kube::Client;
use serde::{Deserialize, Serialize};

use crate::{KubeError, Result};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigEntry {
    pub key: String,
    /// Text value; `None` for binary entries.
    pub value: Option<String>,
    pub binary: bool,
    pub size: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigData {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    /// Sent back on save; the save is refused if the object changed meanwhile.
    pub resource_version: String,
    pub secret_type: Option<String>,
    pub immutable: bool,
    pub entries: Vec<ConfigEntry>,
}

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

fn entry_from_bytes(key: String, bytes: &[u8]) -> ConfigEntry {
    match std::str::from_utf8(bytes) {
        Ok(text) => ConfigEntry { key, value: Some(text.to_string()), binary: false, size: bytes.len() },
        Err(_) => ConfigEntry { key, value: None, binary: true, size: bytes.len() },
    }
}

pub async fn get_config(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<ConfigData> {
    match kind {
        "ConfigMap" => {
            let c = Api::<ConfigMap>::namespaced(client.clone(), namespace).get(name).await?;
            let mut entries: Vec<ConfigEntry> = c
                .data
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| ConfigEntry { size: v.len(), key: k, value: Some(v), binary: false })
                .collect();
            entries.extend(c.binary_data.unwrap_or_default().into_iter().map(|(k, v)| ConfigEntry {
                key: k,
                value: None,
                binary: true,
                size: v.0.len(),
            }));
            entries.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(ConfigData {
                kind: kind.into(),
                namespace: namespace.into(),
                name: name.into(),
                resource_version: c.metadata.resource_version.unwrap_or_default(),
                secret_type: None,
                immutable: c.immutable.unwrap_or(false),
                entries,
            })
        }
        "Secret" => {
            let s = Api::<Secret>::namespaced(client.clone(), namespace).get(name).await?;
            let mut entries: Vec<ConfigEntry> =
                s.data.unwrap_or_default().into_iter().map(|(k, v)| entry_from_bytes(k, &v.0)).collect();
            entries.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(ConfigData {
                kind: kind.into(),
                namespace: namespace.into(),
                name: name.into(),
                resource_version: s.metadata.resource_version.unwrap_or_default(),
                secret_type: s.type_,
                immutable: s.immutable.unwrap_or(false),
                entries,
            })
        }
        other_kind => Err(other(format!("{other_kind} isn't a ConfigMap or Secret"))),
    }
}

/// Replace the text entries with `text` (key → value). Binary entries listed
/// in `keep_binary` are preserved unchanged; any other binary entry is
/// removed. Fails with a conflict if the object changed since
/// `resource_version` was read.
pub async fn save_config(
    client: &Client,
    kind: &str,
    namespace: &str,
    name: &str,
    resource_version: &str,
    text: BTreeMap<String, String>,
    keep_binary: &[String],
) -> Result<ConfigData> {
    if let Some(bad) = text.keys().find(|k| !valid_key(k)) {
        return Err(other(format!(
            "\"{bad}\" isn't a valid key (letters, digits, '-', '_' and '.' only, max 253 chars)"
        )));
    }
    let pp = PostParams { dry_run: false, field_manager: Some("portside-lite".into()) };
    let conflict = |e: kube::Error| match &e {
        kube::Error::Api(s) if s.code == 409 => {
            other("Conflict: it changed on the cluster since you opened it. Reload, then reapply your edit.")
        }
        _ => KubeError::from(e),
    };
    match kind {
        "ConfigMap" => {
            let api = Api::<ConfigMap>::namespaced(client.clone(), namespace);
            let mut c = api.get(name).await?;
            if c.immutable == Some(true) {
                return Err(other("This ConfigMap is immutable; Kubernetes won't allow edits. Recreate it instead."));
            }
            c.metadata.resource_version = Some(resource_version.to_string());
            c.metadata.managed_fields = None;
            c.binary_data = c.binary_data.map(|b| b.into_iter().filter(|(k, _)| keep_binary.contains(k)).collect());
            c.data = Some(text);
            api.replace(name, &pp, &c).await.map_err(conflict)?;
        }
        "Secret" => {
            let api = Api::<Secret>::namespaced(client.clone(), namespace);
            let mut s = api.get(name).await?;
            if s.immutable == Some(true) {
                return Err(other("This Secret is immutable; Kubernetes won't allow edits. Recreate it instead."));
            }
            let mut data: BTreeMap<String, ByteString> = s
                .data
                .take()
                .unwrap_or_default()
                .into_iter()
                .filter(|(k, v)| keep_binary.contains(k) && std::str::from_utf8(&v.0).is_err())
                .collect();
            data.extend(text.into_iter().map(|(k, v)| (k, ByteString(v.into_bytes()))));
            s.metadata.resource_version = Some(resource_version.to_string());
            s.metadata.managed_fields = None;
            s.data = Some(data);
            s.string_data = None;
            api.replace(name, &pp, &s).await.map_err(conflict)?;
        }
        other_kind => return Err(other(format!("{other_kind} isn't a ConfigMap or Secret"))),
    }
    get_config(client, kind, namespace, name).await
}

/// ConfigMap/Secret key rule: [-._a-zA-Z0-9]+, at most 253 characters.
pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= 253 && key.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_binary_detection() {
        assert!(valid_key("app.properties"));
        assert!(valid_key("DB_URL"));
        assert!(!valid_key("has space"));
        assert!(!valid_key(""));
        assert!(!entry_from_bytes("t".into(), b"hello").binary);
        let bin = entry_from_bytes("b".into(), &[0xff, 0xfe, 0x00]);
        assert!(bin.binary && bin.value.is_none() && bin.size == 3);
    }
}
