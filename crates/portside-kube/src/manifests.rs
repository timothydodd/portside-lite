//! Export, edit and copy manifests. Kinds are resolved through API discovery,
//! so plurals and scope are always right.

use kube::api::{Api, DynamicObject, ListParams, Patch, PatchParams, PostParams};
use kube::core::{GroupVersionKind, TypeMeta};
use kube::discovery::{self, ApiCapabilities, ApiResource, Scope};
use kube::Client;
use portside_core::manifest::{
    apply_order, clean_for_edit, clean_for_export, references, related_objects, retarget_namespace, service_account_name,
    ManifestDoc, NamespaceObjects, ObjectRef, RelatedRef,
};
pub use crate::copy::{copy_objects, CopyOutcome};
use std::collections::HashMap;
use serde::Serialize;
use serde_json::Value;

use crate::{KubeError, Result};

pub(crate) const FIELD_MANAGER: &str = "portside-lite";

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// (group, version) for the kinds the UI works with. A kind that isn't
/// listed is refused rather than assumed to be a core one.
fn group_version(kind: &str) -> Result<(&'static str, &'static str)> {
    Ok(match kind {
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" => ("apps", "v1"),
        "Job" | "CronJob" => ("batch", "v1"),
        "Ingress" | "IngressClass" | "NetworkPolicy" => ("networking.k8s.io", "v1"),
        "PodDisruptionBudget" => ("policy", "v1"),
        "PriorityClass" => ("scheduling.k8s.io", "v1"),
        "CustomResourceDefinition" => ("apiextensions.k8s.io", "v1"),
        "HelmChart" | "HelmChartConfig" => ("helm.cattle.io", "v1"),
        "HorizontalPodAutoscaler" => ("autoscaling", "v2"),
        "StorageClass" => ("storage.k8s.io", "v1"),
        "Role" | "RoleBinding" | "ClusterRole" | "ClusterRoleBinding" => ("rbac.authorization.k8s.io", "v1"),
        "Pod" | "Service" | "ConfigMap" | "Secret" | "PersistentVolumeClaim" | "PersistentVolume" | "ServiceAccount" | "Namespace"
        | "Node" | "Event" | "Endpoints" | "LimitRange" | "ResourceQuota" => ("", "v1"),
        "EndpointSlice" => ("discovery.k8s.io", "v1"),
        _ => return Err(other(format!("{kind} objects aren't handled here"))),
    })
}

async fn resolve(client: &Client, gvk: &GroupVersionKind) -> Result<(ApiResource, ApiCapabilities)> {
    discovery::pinned_kind(client, gvk).await.map_err(|e| match e {
        // The server answered: it doesn't serve this kind.
        kube::Error::Api(_) | kube::Error::Discovery(_) => other(format!("unknown kind {}/{}: {e}", gvk.api_version(), gvk.kind)),
        // It didn't answer; that says nothing about the kind.
        e => KubeError::from(e),
    })
}

pub(crate) async fn api_for_kind(client: &Client, kind: &str, namespace: Option<&str>) -> Result<(Api<DynamicObject>, ApiResource)> {
    let (api, ar, _) = api_for_kind_scoped(client, kind, namespace).await?;
    Ok((api, ar))
}

/// Like [`api_for_kind`], also saying whether the kind is namespaced (the
/// namespace is ignored for cluster-scoped kinds).
pub(crate) async fn api_for_kind_scoped(client: &Client, kind: &str, namespace: Option<&str>) -> Result<(Api<DynamicObject>, ApiResource, Scope)> {
    let (g, v) = group_version(kind)?;
    let (ar, caps) = resolve(client, &GroupVersionKind::gvk(g, v, kind)).await?;
    let scope = caps.scope;
    let api = match (&scope, namespace) {
        (Scope::Namespaced, Some(ns)) => Api::namespaced_with(client.clone(), ns, &ar),
        _ => Api::all_with(client.clone(), &ar),
    };
    Ok((api, ar, scope))
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
    // Cluster-scoped objects (PersistentVolume, StorageClass) come with an empty namespace.
    let expected_namespace = expected_namespace.filter(|n| !n.is_empty());
    let obj: DynamicObject = serde_yaml::from_str(yaml).map_err(|e| other(format!("Invalid YAML: {e}")))?;
    check_edit(&obj, expected_kind, expected_namespace, expected_name).map_err(other)?;
    let types = obj.types.clone().ok_or_else(|| other("apiVersion and kind are required"))?;
    let name = expected_name.to_string();
    let ns = obj.metadata.namespace.clone();

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

/// What an edit must keep: the identity it was opened with, and the
/// `resourceVersion` it was loaded at (without it, replace overwrites
/// whatever is on the cluster instead of failing with a conflict).
fn check_edit(obj: &DynamicObject, expected_kind: &str, expected_namespace: Option<&str>, expected_name: &str) -> std::result::Result<(), String> {
    let types = obj.types.as_ref().ok_or("apiVersion and kind are required")?;
    let name = obj.metadata.name.as_deref().ok_or("metadata.name is required")?;
    if types.kind != expected_kind {
        return Err(format!("kind changed from {expected_kind} to {} — that would be a different object", types.kind));
    }
    if name != expected_name {
        return Err(format!("metadata.name changed from {expected_name} to {name} — renaming isn't supported (export and copy instead)"));
    }
    if obj.metadata.namespace.as_deref() != expected_namespace {
        return Err("metadata.namespace can't be changed here — use Copy to cluster to move it".into());
    }
    if obj.metadata.resource_version.as_deref().is_none_or(str::is_empty) {
        return Err("metadata.resourceVersion is missing. It's how the save notices that someone else changed the object; reload to get it back, then reapply your edit.".into());
    }
    Ok(())
}

/// Every object in the namespace as JSON. Empty if the kind isn't served or
/// listing is forbidden; any other failure is an error, because callers take
/// "empty" to mean "nothing else uses this" when deciding what's safe to remove.
pub(crate) async fn list_values(client: &Client, kind: &str, namespace: &str) -> Result<Vec<Value>> {
    let settled = |e: &KubeError| matches!(e, KubeError::Other(_)) || matches!(e.api_status(), Some(403 | 404));
    let (api, ar) = match api_for_kind(client, kind, Some(namespace)).await {
        Ok(found) => found,
        Err(e) if settled(&e) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    match api.list(&ListParams::default()).await.map_err(KubeError::from) {
        Ok(list) => Ok(list.items.into_iter().filter_map(|o| to_value(o, &ar).ok()).collect()),
        Err(e) if settled(&e) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// What belongs with a workload (or a ServiceAccount) when exporting or
/// copying it: pod-spec references, what its ServiceAccount brings (pull
/// secrets, RoleBindings, Roles), Services selecting its pods, Ingresses
/// routing to them, HPAs.
pub async fn related(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<Vec<RelatedRef>> {
    let workload = get_value(client, kind, Some(namespace), name).await?;
    let ns = namespace_objects(client, namespace).await?;
    // Referenced by name, so they may not exist.
    let mut wanted = references(&workload);
    if let Some(account) = service_account_name(&workload) {
        let sa = ns.service_accounts.iter().find(|s| s.pointer("/metadata/name").and_then(Value::as_str) == Some(account.as_str()));
        wanted.extend(sa.map(references).unwrap_or_default());
    }
    let mut present = std::collections::HashSet::new();
    for r in wanted {
        if let Ok((api, _)) = api_for_kind(client, &r.kind, Some(namespace)).await {
            if api.get_opt(&r.name).await.ok().flatten().is_some() {
                present.insert((r.kind, r.name));
            }
        }
    }
    Ok(related_objects(&workload, &ns, |k, n| present.contains(&(k.to_string(), n.to_string()))))
}

/// The objects of a namespace that [`related_objects`] looks through.
pub(crate) async fn namespace_objects(client: &Client, namespace: &str) -> Result<NamespaceObjects> {
    let (services, ingresses, hpas, service_accounts, role_bindings, roles) = futures::try_join!(
        list_values(client, "Service", namespace),
        list_values(client, "Ingress", namespace),
        list_values(client, "HorizontalPodAutoscaler", namespace),
        list_values(client, "ServiceAccount", namespace),
        list_values(client, "RoleBinding", namespace),
        list_values(client, "Role", namespace),
    )?;
    let (pod_disruption_budgets, network_policies, cluster_roles, cluster_role_bindings) = futures::try_join!(
        list_values(client, "PodDisruptionBudget", namespace),
        list_values(client, "NetworkPolicy", namespace),
        list_values(client, "ClusterRole", namespace),
        list_values(client, "ClusterRoleBinding", namespace),
    )?;
    Ok(NamespaceObjects {
        services,
        ingresses,
        hpas,
        service_accounts,
        role_bindings,
        roles,
        pod_disruption_budgets,
        network_policies,
        cluster_roles,
        cluster_role_bindings,
    })
}

/// One workload plus the chosen related objects as a single multi-document
/// YAML file in apply order (dependencies first), cleaned for re-use anywhere.
pub async fn export_bundle(client: &Client, kind: &str, namespace: &str, name: &str, extras: &[ObjectRef]) -> Result<String> {
    let mut items: Vec<ObjectRef> = extras.to_vec();
    items.push(ObjectRef { kind: kind.into(), name: name.into() });
    items.sort_by_key(|r| apply_order(&r.kind));
    let mut docs = Vec::with_capacity(items.len());
    for r in &items {
        let mut v = get_value(client, &r.kind, Some(namespace), &r.name).await?;
        clean_for_export(&mut v);
        docs.push(to_yaml(&v)?);
    }
    Ok(docs.join("---\n"))
}

/// A cluster backup's contents from the API, as files relative to the backup's folder.
#[derive(Default)]
pub struct ManifestBackup {
    /// Plain files: the YAML, `secrets-needed.md`, `helm-releases.md`.
    pub files: Vec<(String, Vec<u8>)>,
    /// Files to seal with the passphrase (only asked for when there is one):
    /// Secret values and Helm release values.
    pub sealed: Vec<(String, Vec<u8>)>,
    pub objects: usize,
    pub custom_resources: usize,
    pub sealed_secrets: usize,
    pub helm_releases: usize,
    /// Kinds left out because the cluster doesn't serve them or this
    /// connection may not list them.
    pub skipped: Vec<String>,
    pub images_in_use: std::collections::BTreeMap<String, Vec<String>>,
    pub node_only_images: Vec<portside_core::cluster_backup::NodeImage>,
    pub volume_backups: Vec<portside_core::cluster_backup::VolumeBackupRef>,
    pub server_version: Option<String>,
}

type FileDocs = std::collections::BTreeMap<String, Vec<(String, String)>>;

fn add_doc(by_file: &mut FileDocs, kind_file: &str, v: &mut Value) -> Result<()> {
    let ns = v.pointer("/metadata/namespace").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string);
    let name = v.pointer("/metadata/name").and_then(Value::as_str).unwrap_or_default().to_string();
    clean_for_export(v);
    let path = match ns {
        Some(ns) => format!("namespaces/{}/{kind_file}", portside_core::archive::safe_segment(&ns)),
        None => format!("cluster/{kind_file}"),
    };
    by_file.entry(path).or_default().push((name, to_yaml(v)?));
    Ok(())
}

fn join_docs(mut docs: Vec<(String, String)>) -> Vec<u8> {
    docs.sort();
    docs.into_iter().map(|(_, y)| y).collect::<Vec<_>>().join("---\n").into_bytes()
}

async fn list_all(client: &Client, kind: &str) -> Result<Vec<Value>> {
    let (api, ar) = api_for_kind(client, kind, None).await?;
    api.list(&ListParams::default()).await?.items.into_iter().map(|o| to_value(o, &ar)).collect()
}

/// Everything a cluster backup reads from the API: `cluster/<Kind>.yaml`
/// for the cluster-wide settings, `namespaces/<ns>/<Kind>.yaml` for the
/// rest (each a multi-document file Import can apply), custom resources of
/// every CRD as `<Kind>.<group>.yaml`, `secrets-needed.md` (names and key
/// names) and `helm-releases.md`. With `seal`, Secret values and Helm
/// values go to [`ManifestBackup::sealed`]; without it they're never read
/// into the backup.
pub async fn manifest_backup(client: &Client, seal: bool) -> Result<ManifestBackup> {
    use portside_core::backup::{keep_in_manifest_backup, secret_line, CLUSTER_KINDS, MANIFEST_KINDS, OPTIONAL_KINDS};
    use portside_core::cluster_backup as cb;
    let mut out = ManifestBackup { server_version: client.apiserver_version().await.ok().map(|v| v.git_version), ..Default::default() };
    let mut by_file = FileDocs::new();
    let mut crds = Vec::new();
    let mut services = Vec::new();
    let mut cron_jobs = Vec::new();
    for kind in CLUSTER_KINDS.iter().chain(MANIFEST_KINDS) {
        let items = match list_all(client, kind).await {
            Ok(v) => v,
            // Not served (no k3s Helm controller), or not allowed: say so and go on.
            Err(e) if OPTIONAL_KINDS.contains(kind) || e.api_status() == Some(403) => {
                out.skipped.push(format!("{kind}: {e}"));
                continue;
            }
            Err(e) => return Err(other(format!("Couldn't list {kind}s: {e}"))),
        };
        for mut v in items {
            match *kind {
                "CustomResourceDefinition" => crds.push(v.clone()),
                "Service" => services.push(v.clone()),
                "CronJob" => cron_jobs.push(v.clone()),
                _ => {}
            }
            if keep_in_manifest_backup(kind, &v) {
                add_doc(&mut by_file, &format!("{kind}.yaml"), &mut v)?;
                out.objects += 1;
            }
        }
    }
    out.volume_backups = cb::volume_backup_refs(&cron_jobs);

    // Hand-made endpoints (Services without a selector that point outside the cluster).
    let selectorless = cb::selectorless_services(&services);
    for kind in ["Endpoints", "EndpointSlice"] {
        match list_all(client, kind).await {
            Ok(items) => {
                for mut v in items {
                    let keep = if kind == "Endpoints" { cb::keep_endpoints(&v, &selectorless) } else { cb::keep_endpoint_slice(&v) };
                    if keep {
                        add_doc(&mut by_file, &format!("{kind}.yaml"), &mut v)?;
                        out.objects += 1;
                    }
                }
            }
            Err(e) => out.skipped.push(format!("{kind}: {e}")),
        }
    }

    // Objects of every custom resource type (Traefik routes, certificates, …).
    for crd in &crds {
        let Some(r) = cb::crd_resource(crd) else { continue };
        let ar = ApiResource::from_gvk_with_plural(&GroupVersionKind::gvk(&r.group, &r.version, &r.kind), &r.plural);
        let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);
        match api.list(&ListParams::default()).await {
            Ok(list) => {
                for obj in list.items {
                    let mut v = to_value(obj, &ar)?;
                    if cb::keep_custom_resource(&v) {
                        add_doc(&mut by_file, &r.file_name(), &mut v)?;
                        out.objects += 1;
                        out.custom_resources += 1;
                    }
                }
            }
            Err(e) => out.skipped.push(format!("{}.{}: {e}", r.kind, r.group)),
        }
    }
    for (path, docs) in by_file {
        out.files.push((path, join_docs(docs)));
    }

    // Secrets: names always; values (and Helm's release records) only sealed.
    let mut secrets_md = String::from("# Secrets the workloads need\n\n");
    match Api::<k8s_openapi::api::core::v1::Secret>::all(client.clone()).list(&ListParams::default()).await {
        Ok(list) => {
            let all: Vec<Value> = list.items.iter().filter_map(|s| serde_json::to_value(s).ok()).collect();
            let latest = cb::latest_helm_secrets(&all);
            let mut releases: Vec<cb::HelmRelease> = all.iter().filter(|s| latest.contains(&ns_name(s))).filter_map(cb::helm_release).collect();
            releases.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
            out.helm_releases = releases.len();
            out.files.push(("helm-releases.md".into(), cb::helm_releases_md(&releases, seal).into_bytes()));
            let mut lines: Vec<String> = all.iter().filter(|v| keep_in_manifest_backup("Secret", v)).filter_map(secret_line).collect();
            lines.sort();
            secrets_md.push_str(if seal {
                "Their values are in `secrets.tar.gz.age`, sealed with your backup passphrase; a rebuild puts them back.\n\n"
            } else {
                "Not in this backup (no backup passphrase is set): recreate these (same name, type and keys) before importing the workloads that use them.\n\n"
            });
            secrets_md.push_str(&if lines.is_empty() { "None.".to_string() } else { lines.join("\n") });
            if seal {
                let mut docs = Vec::new();
                for v in all.iter().filter(|v| cb::seal_secret(v, &latest)) {
                    let mut v = v.clone();
                    if v.get("apiVersion").is_none() {
                        v["apiVersion"] = Value::from("v1");
                        v["kind"] = Value::from("Secret");
                    }
                    let (ns, name) = ns_name(&v);
                    clean_for_export(&mut v);
                    docs.push((format!("{ns}/{name}"), to_yaml(&v)?));
                }
                out.sealed_secrets = docs.len();
                out.sealed.push((cb::SEALED_SECRETS.into(), join_docs(docs)));
                for r in releases.iter().filter(|r| r.values.is_some()) {
                    let path = format!(
                        "{}/{}/{}.yaml",
                        cb::SEALED_HELM_VALUES,
                        portside_core::archive::safe_segment(&r.namespace),
                        portside_core::archive::safe_segment(&r.name)
                    );
                    out.sealed.push((path, r.values.clone().unwrap_or_default().into_bytes()));
                }
            }
        }
        Err(e) => {
            secrets_md.push_str(&format!("Couldn't list Secrets: {e}"));
            out.skipped.push(format!("Secrets: {e}"));
        }
    }
    secrets_md.push('\n');
    out.files.push(("secrets-needed.md".into(), secrets_md.into_bytes()));

    // Images: what runs, and what only the node has.
    match Api::<k8s_openapi::api::core::v1::Pod>::all(client.clone()).list(&ListParams::default()).await {
        Ok(list) => {
            let pods: Vec<Value> = list.items.iter().filter_map(|p| serde_json::to_value(p).ok()).collect();
            out.images_in_use = cb::images_in_use(&pods);
            out.node_only_images = cb::node_only_images(&pods);
        }
        Err(e) => out.skipped.push(format!("Pods (for images.md): {e}")),
    }
    Ok(out)
}

/// Wait until these CRDs are Established (their objects can be sent), up to
/// `timeout`. Returns the ones that still aren't.
pub async fn wait_established(client: &Client, names: &[String], timeout: std::time::Duration) -> Vec<String> {
    let deadline = tokio::time::Instant::now() + timeout;
    let mut waiting: Vec<String> = names.to_vec();
    loop {
        let mut still = Vec::new();
        for name in waiting {
            let ready = get_value(client, "CustomResourceDefinition", None, &name).await.is_ok_and(|v| {
                v.pointer("/status/conditions")
                    .and_then(Value::as_array)
                    .is_some_and(|cs| cs.iter().any(|c| c["type"] == "Established" && c["status"] == "True"))
            });
            if !ready {
                still.push(name);
            }
        }
        if still.is_empty() || tokio::time::Instant::now() >= deadline {
            return still;
        }
        waiting = still;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

fn ns_name(v: &Value) -> (String, String) {
    let get = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default().to_string();
    (get("/metadata/namespace"), get("/metadata/name"))
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

pub(crate) fn pending_namespace_note(namespace: &str) -> String {
    format!("namespace {namespace} will be created too; the server can't validate objects inside it until it exists")
}

/// Make sure `namespace` exists. Returns `true` when objects can be sent to
/// the server, `false` on a dry run where the namespace would first have to
/// be created (a dry run can't create it, so objects in it can't be
/// validated). Results are cached per call site in `seen`.
pub(crate) async fn namespace_ready(client: &Client, namespace: &str, dry_run: bool, seen: &mut HashMap<String, bool>) -> Result<bool> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_keep_identity_and_resource_version() {
        let obj = |yaml: &str| serde_yaml::from_str::<DynamicObject>(yaml).unwrap();
        let ok = "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: apps\n  resourceVersion: \"42\"\n";
        assert!(check_edit(&obj(ok), "Deployment", Some("apps"), "web").is_ok());
        let err = |yaml: &str, ns| check_edit(&obj(yaml), "Deployment", ns, "web").unwrap_err();
        assert!(err(&ok.replace("  resourceVersion: \"42\"\n", ""), Some("apps")).contains("resourceVersion"));
        assert!(err(&ok.replace("\"42\"", "\"\""), Some("apps")).contains("resourceVersion"));
        assert!(err(&ok.replace("name: web", "name: api"), Some("apps")).contains("metadata.name"));
        assert!(err(&ok.replace("kind: Deployment", "kind: StatefulSet"), Some("apps")).contains("kind changed"));
        assert!(err(&ok.replace("namespace: apps", "namespace: prod"), Some("apps")).contains("namespace"));
        assert!(err(ok, None).contains("namespace"), "a cluster-scoped target can't gain a namespace");
    }
}
