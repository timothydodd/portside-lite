//! Export, edit and copy manifests. Kinds are resolved through API discovery,
//! so plurals and scope are always right.

use kube::api::{Api, DynamicObject, ListParams, Patch, PatchParams, PostParams};
use kube::core::{GroupVersionKind, TypeMeta};
use kube::discovery::{self, ApiCapabilities, ApiResource, Scope};
use kube::Client;
use portside_core::manifest::{
    apply_order, clean_for_edit, clean_for_export, references, retarget_namespace, ManifestDoc, ObjectRef,
};
use std::collections::HashMap;
use serde::Serialize;
use serde_json::Value;

use crate::{KubeError, Result};

const FIELD_MANAGER: &str = "portside-lite";

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// (group, version) for the kinds the UI works with.
fn group_version(kind: &str) -> (&'static str, &'static str) {
    match kind {
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" => ("apps", "v1"),
        "Job" | "CronJob" => ("batch", "v1"),
        "Ingress" => ("networking.k8s.io", "v1"),
        _ => ("", "v1"), // Pod, Service, ConfigMap, Secret, PVC, ServiceAccount, Namespace, Node
    }
}

async fn resolve(client: &Client, gvk: &GroupVersionKind) -> Result<(ApiResource, ApiCapabilities)> {
    discovery::pinned_kind(client, gvk)
        .await
        .map_err(|e| other(format!("unknown kind {}/{}: {e}", gvk.api_version(), gvk.kind)))
}

pub(crate) async fn api_for_kind(client: &Client, kind: &str, namespace: Option<&str>) -> Result<(Api<DynamicObject>, ApiResource)> {
    let (g, v) = group_version(kind);
    let (ar, caps) = resolve(client, &GroupVersionKind::gvk(g, v, kind)).await?;
    let api = match (caps.scope, namespace) {
        (Scope::Namespaced, Some(ns)) => Api::namespaced_with(client.clone(), ns, &ar),
        _ => Api::all_with(client.clone(), &ar),
    };
    Ok((api, ar))
}

/// JSON form of one object, with apiVersion/kind filled in (list/get
/// responses for dynamic objects can omit them).
pub(crate) async fn get_value(client: &Client, kind: &str, namespace: Option<&str>, name: &str) -> Result<Value> {
    let (api, ar) = api_for_kind(client, kind, namespace).await?;
    let obj = api.get(name).await?;
    to_value(obj, &ar)
}

fn to_value(mut obj: DynamicObject, ar: &ApiResource) -> Result<Value> {
    if obj.types.is_none() {
        obj.types = Some(TypeMeta { api_version: ar.api_version.clone(), kind: ar.kind.clone() });
    }
    serde_json::to_value(&obj).map_err(|e| other(e.to_string()))
}

fn to_yaml(v: &Value) -> Result<String> {
    serde_yaml::to_string(v).map_err(|e| other(e.to_string()))
}

/// Clean, re-appliable YAML. `name: None` exports every object of `kind` in
/// the namespace (all namespaces when `None`) as one multi-document file.
pub async fn export_yaml(client: &Client, kind: &str, namespace: Option<&str>, name: Option<&str>) -> Result<String> {
    let values: Vec<Value> = match name {
        Some(n) => vec![get_value(client, kind, namespace, n).await?],
        None => {
            let (api, ar) = api_for_kind(client, kind, namespace).await?;
            let mut items = api.list(&ListParams::default()).await?.items;
            items.sort_by(|a, b| (&a.metadata.namespace, &a.metadata.name).cmp(&(&b.metadata.namespace, &b.metadata.name)));
            items.into_iter().map(|o| to_value(o, &ar)).collect::<Result<_>>()?
        }
    };
    let docs: Vec<String> = values
        .into_iter()
        .map(|mut v| {
            clean_for_export(&mut v);
            to_yaml(&v)
        })
        .collect::<Result<_>>()?;
    Ok(docs.join("---\n"))
}

/// YAML for the editor: current object minus status/managedFields, keeping
/// `resourceVersion` for optimistic concurrency.
pub async fn edit_yaml(client: &Client, kind: &str, namespace: Option<&str>, name: &str) -> Result<String> {
    let mut v = get_value(client, kind, namespace, name).await?;
    clean_for_edit(&mut v);
    to_yaml(&v)
}

/// Save edited YAML back, like `kubectl edit`: a replace that fails with a
/// conflict if the object changed since it was loaded. The edit must target
/// the same kind/namespace/name it was opened from.
pub async fn apply_edit(
    client: &Client,
    yaml: &str,
    expected_kind: &str,
    expected_namespace: Option<&str>,
    expected_name: &str,
    dry_run: bool,
) -> Result<String> {
    if yaml.split("\n---").filter(|d| !d.trim().is_empty()).count() > 1 {
        return Err(other("The editor holds one object; remove the extra `---` documents."));
    }
    let obj: DynamicObject = serde_yaml::from_str(yaml).map_err(|e| other(format!("Invalid YAML: {e}")))?;
    let types = obj.types.clone().ok_or_else(|| other("apiVersion and kind are required"))?;
    let name = obj.metadata.name.clone().ok_or_else(|| other("metadata.name is required"))?;
    if types.kind != expected_kind {
        return Err(other(format!("kind changed from {expected_kind} to {} — that would be a different object", types.kind)));
    }
    if name != expected_name {
        return Err(other(format!("metadata.name changed from {expected_name} to {name} — renaming isn't supported (export and copy instead)")));
    }
    let ns = obj.metadata.namespace.clone();
    if expected_namespace.is_some() && ns.as_deref() != expected_namespace {
        return Err(other("metadata.namespace can't be changed here — use Copy to cluster to move it"));
    }

    let gvk = GroupVersionKind::try_from(&types).map_err(|e| other(e.to_string()))?;
    let (ar, caps) = resolve(client, &gvk).await?;
    let api: Api<DynamicObject> = match (caps.scope, ns.as_deref()) {
        (Scope::Namespaced, Some(n)) => Api::namespaced_with(client.clone(), n, &ar),
        _ => Api::all_with(client.clone(), &ar),
    };
    let pp = PostParams { dry_run, field_manager: Some(FIELD_MANAGER.into()) };
    api.replace(&name, &pp, &obj).await.map_err(|e| match &e {
        kube::Error::Api(s) if s.code == 409 => other(
            "Conflict: the object changed on the cluster since you opened it. Reload to get the latest version, then reapply your edit.",
        ),
        _ => KubeError::from(e),
    })?;
    Ok(format!(
        "{}/{} {}",
        ar.plural,
        name,
        if dry_run { "validated (dry run, nothing changed)" } else { "configured" }
    ))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefStatus {
    pub kind: String,
    pub name: String,
    /// Whether it exists in the source namespace.
    pub exists: bool,
    /// ConfigMap/Secret can be copied along; PVC/ServiceAccount are warnings.
    pub copyable: bool,
}

/// What a workload depends on, and whether each exists in its namespace.
pub async fn workload_references(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<Vec<RefStatus>> {
    let v = get_value(client, kind, Some(namespace), name).await?;
    let mut out = Vec::new();
    for ObjectRef { kind: rk, name: rn } in references(&v) {
        let exists = match api_for_kind(client, &rk, Some(namespace)).await {
            Ok((api, _)) => api.get_opt(&rn).await.ok().flatten().is_some(),
            Err(_) => false,
        };
        out.push(RefStatus { copyable: rk == "ConfigMap" || rk == "Secret", kind: rk, name: rn, exists });
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyResult {
    pub kind: String,
    pub name: String,
    /// created | updated | error
    pub outcome: String,
    pub message: Option<String>,
}

/// Copy objects from one cluster/namespace to another with server-side apply
/// (creates, or updates what exists). Each item is cleaned of server fields
/// and retargeted to `target_namespace`. The namespace is created if missing.
pub async fn copy_objects(
    src: &Client,
    dst: &Client,
    source_namespace: &str,
    items: &[ObjectRef],
    target_namespace: &str,
    dry_run: bool,
) -> Result<Vec<CopyResult>> {
    let mut namespaces = HashMap::new();
    let ns_ready = namespace_ready(dst, target_namespace, dry_run, &mut namespaces).await?;
    let mut pp = PatchParams::apply(FIELD_MANAGER).force();
    pp.dry_run = dry_run;

    let mut results = Vec::with_capacity(items.len());
    for item in items {
        if !ns_ready {
            // Dry run into a namespace that doesn't exist yet: the server can't
            // validate objects inside it, so report the plan instead of failing.
            results.push(CopyResult {
                kind: item.kind.clone(),
                name: item.name.clone(),
                outcome: "created".into(),
                message: Some(pending_namespace_note(target_namespace)),
            });
            continue;
        }
        let result = async {
            let mut v = get_value(src, &item.kind, Some(source_namespace), &item.name).await?;
            clean_for_export(&mut v);
            retarget_namespace(&mut v, target_namespace);
            let (api, _) = api_for_kind(dst, &item.kind, Some(target_namespace)).await?;
            let existed = api.get_opt(&item.name).await?.is_some();
            api.patch(&item.name, &pp, &Patch::Apply(&v)).await?;
            Ok::<_, KubeError>(if existed { "updated" } else { "created" })
        }
        .await;
        results.push(match result {
            Ok(outcome) => CopyResult { kind: item.kind.clone(), name: item.name.clone(), outcome: outcome.into(), message: None },
            Err(e) => CopyResult { kind: item.kind.clone(), name: item.name.clone(), outcome: "error".into(), message: Some(e.to_string()) },
        });
    }
    Ok(results)
}

fn pending_namespace_note(namespace: &str) -> String {
    format!("namespace {namespace} will be created too; the server can't validate objects inside it until it exists")
}

/// Make sure `namespace` exists. Returns `true` when objects can be sent to
/// the server, `false` on a dry run where the namespace would first have to
/// be created (a dry run can't create it, so objects in it can't be
/// validated). Results are cached per call site in `seen`.
async fn namespace_ready(client: &Client, namespace: &str, dry_run: bool, seen: &mut HashMap<String, bool>) -> Result<bool> {
    use k8s_openapi::api::core::v1::Namespace;
    if let Some(ready) = seen.get(namespace) {
        return Ok(*ready);
    }
    let api: Api<Namespace> = Api::all(client.clone());
    let ready = if api.get_opt(namespace).await?.is_some() {
        true
    } else if dry_run {
        false
    } else {
        let ns: Namespace = serde_json::from_value(serde_json::json!({ "metadata": { "name": namespace } }))
            .map_err(|e| other(e.to_string()))?;
        api.create(&PostParams { dry_run: false, field_manager: Some(FIELD_MANAGER.into()) }, &ns).await?;
        true
    };
    seen.insert(namespace.to_string(), ready);
    Ok(ready)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub index: usize,
    pub source: String,
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    /// created | updated | error
    pub outcome: String,
    pub message: Option<String>,
}

/// Apply parsed manifests with server-side apply, dependencies first.
/// Namespaced objects go to `namespace_override`, else their own namespace,
/// else `default`; missing namespaces are created. Every object gets a result
/// row; one failure doesn't stop the rest.
pub async fn import_docs(
    client: &Client,
    mut docs: Vec<(ManifestDoc, Value)>,
    namespace_override: Option<&str>,
    dry_run: bool,
) -> Vec<ImportResult> {
    docs.sort_by_key(|(d, _)| apply_order(d.kind.as_deref().unwrap_or_default()));
    let mut pp = PatchParams::apply(FIELD_MANAGER).force();
    pp.dry_run = dry_run;
    let mut namespaces: HashMap<String, bool> = HashMap::new();
    let mut results = Vec::with_capacity(docs.len());

    for (doc, mut value) in docs {
        let kind = doc.kind.clone().unwrap_or_default();
        let name = doc.name.clone().unwrap_or_default();
        let row = |namespace: Option<String>, outcome: &str, message: Option<String>| ImportResult {
            index: doc.index,
            source: doc.source.clone(),
            kind: kind.clone(),
            name: name.clone(),
            namespace,
            outcome: outcome.into(),
            message,
        };
        let outcome: Result<(Option<String>, &str, Option<String>)> = async {
            let types = TypeMeta { api_version: doc.api_version.clone().unwrap_or_default(), kind: kind.clone() };
            let gvk = GroupVersionKind::try_from(&types).map_err(|e| other(e.to_string()))?;
            let (ar, caps) = resolve(client, &gvk).await?;
            clean_for_export(&mut value);

            let (api, namespace): (Api<DynamicObject>, Option<String>) = if caps.scope == Scope::Namespaced {
                let ns = namespace_override
                    .filter(|n| !n.trim().is_empty())
                    .map(str::to_string)
                    .or_else(|| doc.namespace.clone())
                    .unwrap_or_else(|| "default".into());
                retarget_namespace(&mut value, &ns);
                if !namespace_ready(client, &ns, dry_run, &mut namespaces).await? {
                    return Ok((Some(ns.clone()), "created", Some(pending_namespace_note(&ns))));
                }
                (Api::namespaced_with(client.clone(), &ns, &ar), Some(ns))
            } else {
                (Api::all_with(client.clone(), &ar), None)
            };

            let existed = api.get_opt(&name).await?.is_some();
            api.patch(&name, &pp, &Patch::Apply(&value)).await?;
            if kind == "Namespace" {
                // Later objects can target it; on a dry run it still won't exist.
                namespaces.insert(name.clone(), !dry_run || existed);
            }
            Ok((namespace, if existed { "updated" } else { "created" }, None))
        }
        .await;
        results.push(match outcome {
            Ok((ns, o, msg)) => row(ns, o, msg),
            Err(e) => row(doc.namespace.clone(), "error", Some(e.to_string())),
        });
    }
    results
}
