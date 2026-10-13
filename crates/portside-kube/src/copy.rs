//! Copy a workload and what belongs with it to another cluster or
//! namespace, and say what the target still has to provide.

use std::collections::HashMap;

use k8s_openapi::api::core::v1::{LocalObjectReference, Node, ServiceAccount};
use k8s_openapi::api::storage::v1::StorageClass;
use kube::api::{Api, ListParams, Patch, PatchParams, PostParams};
use kube::discovery::Scope;
use kube::Client;
use portside_core::manifest::{
    apply_order, clean_for_export, copy_needs, public_registry, retarget_binding_subjects, retarget_namespace, service_account_name, Need,
    ObjectRef, CLUSTER_SCOPED_NEEDS,
};
use portside_core::registry::secret_pull_hosts;
use serde::Serialize;
use serde_json::Value;

use crate::manifests::{api_for_kind_scoped, get_value, namespace_ready, pending_namespace_note, CopyResult, FIELD_MANAGER};
use crate::{KubeError, Result};

/// Secrets the token controller makes for a ServiceAccount; the target
/// makes its own, and applying one elsewhere is refused or misleading.
const SA_TOKEN_TYPE: &str = "kubernetes.io/service-account-token";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyOutcome {
    /// One row per object, in apply order; "Volume files" rows for copied
    /// claims; a last row for the target ServiceAccount when pull secrets
    /// were attached to it.
    pub results: Vec<CopyResult>,
    /// What the copy relies on that the target doesn't have: unticked
    /// config, missing classes, pull secrets, nodes.
    pub warnings: Vec<String>,
}

/// One object of the set, read and cleaned.
pub struct Fetched {
    pub item: ObjectRef,
    pub value: Value,
    pub scope: Scope,
}

/// Kinds that run pods. Applied last, after any volume data, so the pods
/// find everything in place and nothing has the claims mounted meanwhile.
pub const WORKLOAD_KINDS: &[&str] = &["Deployment", "StatefulSet", "DaemonSet", "ReplicaSet", "Job", "CronJob", "Pod"];

/// The set to copy, read from the source and checked against the target.
pub struct CopyPlan {
    pub workload: ObjectRef,
    pub fetched: Vec<Fetched>,
    /// Objects that couldn't be read; they're reported, not applied.
    pub unreadable: Vec<CopyResult>,
    pub warnings: Vec<String>,
}

impl CopyPlan {
    pub fn contains(&self, kind: &str, name: &str) -> bool {
        self.fetched.iter().any(|f| f.item.kind == kind && f.item.name == name)
    }
    /// The workload's own object, when it was readable.
    pub fn workload_value(&self) -> Option<&Value> {
        self.fetched.iter().find(|f| f.item == self.workload).map(|f| &f.value)
    }
}

/// Read `workload` plus `extras` from the source (everything first, so the
/// checks see the whole set and one unreadable object doesn't leave a
/// half-applied copy behind), clean them, and work out what the target
/// still has to provide.
pub async fn prepare(src: &Client, dst: &Client, source_namespace: &str, workload: &ObjectRef, extras: &[ObjectRef], target_namespace: &str) -> CopyPlan {
    let mut items: Vec<ObjectRef> = extras.iter().filter(|e| *e != workload).cloned().collect();
    items.push(workload.clone());
    items.sort_by_key(|r| apply_order(&r.kind));
    let mut unreadable = Vec::new();
    let mut fetched = Vec::with_capacity(items.len());
    for item in items {
        let got = async {
            let (_, _, scope) = api_for_kind_scoped(src, &item.kind, Some(source_namespace)).await?;
            let mut value = get_value(src, &item.kind, Some(source_namespace), &item.name).await?;
            clean_for_export(&mut value);
            Ok::<_, KubeError>((value, scope))
        }
        .await;
        match got {
            Ok((value, scope)) => fetched.push(Fetched { item, value, scope }),
            Err(e) => unreadable.push(row(&item, "error", Some(e.to_string()))),
        }
    }
    let warnings = checks(src, dst, source_namespace, target_namespace, &fetched).await;
    CopyPlan { workload: workload.clone(), fetched, unreadable, warnings }
}

/// Apply the plan's objects whose kind passes `take`, with server-side apply
/// (creates, or updates what exists). Namespaced objects are retargeted to
/// `target_namespace`; a (Cluster)RoleBinding's ServiceAccount subjects
/// follow. The namespace is created if missing.
pub async fn apply(
    dst: &Client,
    plan: &CopyPlan,
    source_namespace: &str,
    target_namespace: &str,
    dry_run: bool,
    take: impl Fn(&str) -> bool,
) -> Result<Vec<CopyResult>> {
    let mut namespaces = HashMap::new();
    let ns_ready = namespace_ready(dst, target_namespace, dry_run, &mut namespaces).await?;
    let mut pp = PatchParams::apply(FIELD_MANAGER).force();
    pp.dry_run = dry_run;

    let mut results = Vec::new();
    for Fetched { item, value, scope } in plan.fetched.iter().filter(|f| take(&f.item.kind)) {
        if !ns_ready {
            // Dry run into a namespace that doesn't exist yet: the server can't
            // validate objects inside it, so report the plan instead of failing.
            results.push(row(item, "created", Some(pending_namespace_note(target_namespace))));
            continue;
        }
        if item.kind == "Secret" && value.get("type").and_then(Value::as_str) == Some(SA_TOKEN_TYPE) {
            results.push(row(item, "error", Some("an auto-generated service account token; the target makes its own, so it isn't copied".into())));
            continue;
        }
        let mut value = value.clone();
        let result = async {
            match scope {
                Scope::Namespaced => retarget_namespace(&mut value, target_namespace),
                Scope::Cluster => retarget_binding_subjects(&mut value, source_namespace, target_namespace),
            }
            let (api, _, _) = api_for_kind_scoped(dst, &item.kind, Some(target_namespace)).await?;
            let existed = api.get_opt(&item.name).await?.is_some();
            api.patch(&item.name, &pp, &Patch::Apply(&value)).await?;
            Ok::<_, KubeError>(if existed { "updated" } else { "created" })
        }
        .await;
        results.push(match result {
            Ok(outcome) => row(item, outcome, None),
            Err(e) => row(item, "error", Some(e.to_string())),
        });
    }
    Ok(results)
}

/// Prepare, apply everything, attach pull secrets: the whole copy in one go
/// (no volume data).
pub async fn copy_objects(
    src: &Client,
    dst: &Client,
    source_namespace: &str,
    workload: &ObjectRef,
    extras: &[ObjectRef],
    target_namespace: &str,
    dry_run: bool,
) -> Result<CopyOutcome> {
    let plan = prepare(src, dst, source_namespace, workload, extras, target_namespace).await;
    let mut results = plan.unreadable.clone();
    results.extend(apply(dst, &plan, source_namespace, target_namespace, dry_run, |_| true).await?);
    if let Some(r) = attach_pull_secrets(src, dst, source_namespace, target_namespace, &plan, &results, dry_run).await {
        results.push(r);
    }
    Ok(CopyOutcome { results, warnings: plan.warnings })
}

fn row(item: &ObjectRef, outcome: &str, message: Option<String>) -> CopyResult {
    CopyResult { kind: item.kind.clone(), name: item.name.clone(), outcome: outcome.into(), message }
}

/// Pull secrets the workload's pods get through their ServiceAccount are
/// only useful on the target if that account lists them too. When the
/// account itself is copied its manifest carries them; otherwise (the
/// `default` account, or one left unticked) add them to the target's account.
/// `results` are the rows applied so far; nothing happens on a dry run into
/// a namespace that doesn't exist yet.
pub async fn attach_pull_secrets(
    src: &Client,
    dst: &Client,
    source_namespace: &str,
    target_namespace: &str,
    plan: &CopyPlan,
    results: &[CopyResult],
    dry_run: bool,
) -> Option<CopyResult> {
    let applied = |kind: &str| -> Vec<String> {
        results
            .iter()
            .filter(|r| r.kind == kind && matches!(r.outcome.as_str(), "created" | "updated") && r.message.is_none())
            .map(|r| r.name.clone())
            .collect()
    };
    let (copied_secrets, copied_accounts) = (applied("Secret"), applied("ServiceAccount"));
    let account = service_account_name(plan.workload_value()?)?;
    if copied_accounts.contains(&account) || copied_secrets.is_empty() {
        return None;
    }
    let item = ObjectRef { kind: "ServiceAccount".into(), name: account.clone() };
    let source_sa = match Api::<ServiceAccount>::namespaced(src.clone(), source_namespace).get_opt(&account).await {
        Ok(sa) => sa?,
        Err(e) => return Some(row(&item, "error", Some(format!("couldn't read it on the source to see which pull secrets to attach: {e}")))),
    };
    let wanted: Vec<String> = source_sa
        .image_pull_secrets
        .into_iter()
        .flatten()
        .map(|r| r.name)
        .filter(|n| copied_secrets.contains(n))
        .collect();
    if wanted.is_empty() {
        return None;
    }
    let list = wanted.join(", ");
    if dry_run {
        return Some(row(&item, "updated", Some(format!("pull secret {list} added to its imagePullSecrets"))));
    }

    // A fresh namespace gets its default account from a controller, a moment later.
    let api = Api::<ServiceAccount>::namespaced(dst.clone(), target_namespace);
    let mut sa = None;
    for _ in 0..20 {
        match api.get_opt(&account).await {
            Ok(Some(s)) => {
                sa = Some(s);
                break;
            }
            Ok(None) if account == "default" => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
            Ok(None) => break,
            Err(e) => return Some(row(&item, "error", Some(format!("couldn't read it on the target: {e}")))),
        }
    }
    let Some(mut sa) = sa else {
        return Some(row(
            &item,
            "error",
            Some(format!("doesn't exist in {target_namespace}, so pull secret {list} couldn't be attached; the pods won't have it")),
        ));
    };
    let have = sa.image_pull_secrets.get_or_insert_with(Vec::new);
    let missing: Vec<String> = wanted.into_iter().filter(|w| !have.iter().any(|r| &r.name == w)).collect();
    if missing.is_empty() {
        return Some(row(&item, "unchanged", Some(format!("already lists pull secret {list}"))));
    }
    have.extend(missing.iter().map(|name| LocalObjectReference { name: name.clone() }));
    // A replace at the version just read: a concurrent edit fails instead of being overwritten.
    let pp = PostParams { dry_run: false, field_manager: Some(FIELD_MANAGER.into()) };
    match api.replace(&account, &pp, &sa).await {
        Ok(_) => Some(row(&item, "updated", Some(format!("pull secret {} added to its imagePullSecrets", missing.join(", "))))),
        Err(kube::Error::Api(s)) if s.code == 409 => Some(row(
            &item,
            "error",
            Some(format!("changed while pull secret {} was being added; add it to the account again", missing.join(", "))),
        )),
        Err(e) => Some(row(&item, "error", Some(format!("pull secret {} couldn't be added: {e}", missing.join(", "))))),
    }
}

/// What the target has to provide for the copied set to work: objects it
/// references that aren't in the set, classes, pull secrets for its images,
/// and nodes it's pinned to. Problems looking are reported, never fatal.
async fn checks(src: &Client, dst: &Client, source_namespace: &str, target_namespace: &str, fetched: &[Fetched]) -> Vec<String> {
    let set: Vec<Value> = fetched.iter().map(|f| f.value.clone()).collect();
    let needs = copy_needs(&set);
    let mut out = Vec::new();

    for n in &needs.objects {
        let present = if n.kind == "StorageClass" && n.name.is_empty() {
            has_default_storage_class(dst).await
        } else if CLUSTER_SCOPED_NEEDS.contains(&n.kind.as_str()) {
            exists(dst, &n.kind, None, &n.name).await
        } else {
            exists(dst, &n.kind, Some(target_namespace), &n.name).await
        };
        match present {
            Ok(true) => {}
            Ok(false) => out.push(missing_message(n, target_namespace)),
            Err(e) => out.push(format!("Couldn't check whether {} {} exists on the target: {e}", n.kind, n.name)),
        }
    }

    // Pull secrets: which hosts each covers, read from the set or the source.
    let mut secret_hosts: HashMap<String, Vec<String>> = HashMap::new();
    for f in fetched.iter().filter(|f| f.item.kind == "Secret") {
        secret_hosts.insert(f.item.name.clone(), secret_pull_hosts(&f.value));
    }
    let in_set = |kind: &str, name: &str| fetched.iter().any(|f| f.item.kind == kind && f.item.name == name);
    let mut account_secrets: HashMap<String, Vec<String>> = HashMap::new();
    for image in &needs.images {
        if !account_secrets.contains_key(&image.account) {
            let names = match fetched.iter().find(|f| f.item.kind == "ServiceAccount" && f.item.name == image.account) {
                Some(sa) => portside_core::manifest::references(&sa.value).into_iter().map(|r| r.name).collect(),
                None => match Api::<ServiceAccount>::namespaced(src.clone(), source_namespace).get_opt(&image.account).await {
                    Ok(sa) => sa.and_then(|s| s.image_pull_secrets).unwrap_or_default().into_iter().map(|r| r.name).collect(),
                    Err(_) => Vec::new(),
                },
            };
            account_secrets.insert(image.account.clone(), names);
        }
        let mut candidates = image.pull_secrets.clone();
        candidates.extend(account_secrets.get(&image.account).cloned().unwrap_or_default());
        candidates.dedup();
        let mut covering: Vec<(String, bool)> = Vec::new();
        for name in candidates {
            if !secret_hosts.contains_key(&name) {
                let hosts = get_value(src, "Secret", Some(source_namespace), &name).await.map(|v| secret_pull_hosts(&v)).unwrap_or_default();
                secret_hosts.insert(name.clone(), hosts);
            }
            if secret_hosts[&name].contains(&image.host) {
                covering.push((name.clone(), in_set("Secret", &name)));
            }
        }
        let message = if covering.iter().any(|(_, copied)| *copied) {
            None
        } else if let Some((name, _)) = covering.first() {
            let on_target = get_value(dst, "Secret", Some(target_namespace), name).await.map(|v| secret_pull_hosts(&v).contains(&image.host)).unwrap_or(false);
            (!on_target).then(|| {
                format!(
                    "{} pulls {} with pull secret {name}, which isn't being copied and isn't in {target_namespace}: tick it under Bring along, or the pods will fail to pull.",
                    image.by, image.image
                )
            })
        } else if !public_registry(&image.host) {
            Some(format!(
                "{} pulls {} without a pull secret. If the source nodes are set up for {} themselves (registries.yaml, a mirror), the target nodes need the same.",
                image.by, image.image, image.host
            ))
        } else {
            None
        };
        if let Some(m) = message {
            if !out.contains(&m) {
                out.push(m);
            }
        }
    }

    if !needs.node_names.is_empty() || !needs.node_selectors.is_empty() {
        match Api::<Node>::all(dst.clone()).list(&ListParams::default()).await {
            Ok(nodes) => {
                let names: Vec<String> = nodes.items.iter().filter_map(|n| n.metadata.name.clone()).collect();
                let labels: Vec<serde_json::Map<String, Value>> = nodes
                    .items
                    .iter()
                    .map(|n| n.metadata.labels.clone().unwrap_or_default().into_iter().map(|(k, v)| (k, Value::String(v))).collect())
                    .collect();
                for (node, by) in &needs.node_names {
                    if !names.contains(node) {
                        out.push(format!("{by} is pinned to node {node} (nodeName), and the target has no node by that name: its pods will never be scheduled."));
                    }
                }
                for (selector, by) in &needs.node_selectors {
                    if !labels.iter().any(|l| portside_core::manifest::selector_matches(selector, l)) {
                        let wanted: Vec<String> = selector.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default())).collect();
                        out.push(format!("{by} needs a node labelled {}; no node on the target matches, so its pods will stay Pending.", wanted.join(", ")));
                    }
                }
            }
            Err(e) => out.push(format!("Couldn't list the target's nodes to check node pinning: {e}")),
        }
    }
    out
}

fn missing_message(n: &Need, target_namespace: &str) -> String {
    let (by, kind, name) = (&n.by, &n.kind, &n.name);
    match kind.as_str() {
        "StorageClass" if name.is_empty() => format!("{by} uses the default StorageClass, and the target has none: the claim will stay Pending."),
        "StorageClass" => format!("{by} uses StorageClass {name}, which the target doesn't have: the claim will stay Pending."),
        "IngressClass" => format!("{by} uses IngressClass {name}, which the target doesn't have: nothing will serve it."),
        "PriorityClass" => format!("{by} uses PriorityClass {name}, which the target doesn't have: its pods will be rejected."),
        "ClusterRole" => format!("{by} grants ClusterRole {name}, which the target doesn't have: the binding grants nothing."),
        "Role" => format!("{by} grants Role {name}, which isn't being copied and doesn't exist in {target_namespace}: the binding grants nothing."),
        "Service" => format!("{by} points at Service {name}, which isn't being copied and doesn't exist in {target_namespace}."),
        "ConfigMap" | "Secret" | "PersistentVolumeClaim" | "ServiceAccount" => {
            format!("{by} needs {kind} {name}, which isn't being copied and doesn't exist in {target_namespace}: its pods won't start. Tick it under Bring along.")
        }
        _ => format!("{by} needs {kind} {name}, which isn't being copied and doesn't exist in {target_namespace}."),
    }
}

async fn exists(client: &Client, kind: &str, namespace: Option<&str>, name: &str) -> Result<bool> {
    let (api, _, _) = api_for_kind_scoped(client, kind, namespace).await?;
    Ok(api.get_opt(name).await?.is_some())
}

async fn has_default_storage_class(client: &Client) -> Result<bool> {
    let list = Api::<StorageClass>::all(client.clone()).list(&ListParams::default()).await?;
    Ok(list.items.iter().any(|sc| {
        sc.metadata
            .annotations
            .as_ref()
            .is_some_and(|a| ["storageclass.kubernetes.io/is-default-class", "storageclass.beta.kubernetes.io/is-default-class"].iter().any(|k| a.get(*k).map(String::as_str) == Some("true")))
    }))
}
