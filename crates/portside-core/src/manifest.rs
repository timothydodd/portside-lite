//! Manifest shaping for export, editing and copying between clusters.
//! Operates on the JSON form of any object, so it works for every kind.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Annotations the server or kubectl maintain; meaningless in a copy.
const NOISY_ANNOTATIONS: &[&str] = &[
    "kubectl.kubernetes.io/last-applied-configuration",
    "deployment.kubernetes.io/revision",
];

fn strip_meta(meta: &mut Value, keep_resource_version: bool) {
    let Some(m) = meta.as_object_mut() else { return };
    m.remove("managedFields");
    m.remove("selfLink");
    if !keep_resource_version {
        for k in ["uid", "resourceVersion", "generation", "creationTimestamp", "ownerReferences"] {
            m.remove(k);
        }
        if let Some(Value::Object(ann)) = m.get_mut("annotations") {
            for a in NOISY_ANNOTATIONS {
                ann.remove(*a);
            }
        }
        if m.get("annotations").and_then(Value::as_object).is_some_and(|a| a.is_empty()) {
            m.remove("annotations");
        }
    }
}

/// Annotations the PV controller adds when binding a claim.
const PVC_BINDING_ANNOTATIONS: &[&str] = &[
    "pv.kubernetes.io/bind-completed",
    "pv.kubernetes.io/bound-by-controller",
    "volume.beta.kubernetes.io/storage-provisioner",
    "volume.kubernetes.io/storage-provisioner",
    "volume.kubernetes.io/selected-node",
];

/// Fields the cluster assigns per kind, which would conflict or pin the
/// object to this cluster if applied elsewhere.
fn strip_cluster_assigned(obj: &mut Value) {
    let kind = obj.get("kind").and_then(Value::as_str).unwrap_or_default().to_string();
    match kind.as_str() {
        "Service" => {
            let service_type = obj
                .pointer("/spec/type")
                .and_then(Value::as_str)
                .unwrap_or("ClusterIP")
                .to_string();
            if let Some(Value::Object(spec)) = obj.get_mut("spec") {
                for k in ["clusterIP", "clusterIPs", "healthCheckNodePort"] {
                    spec.remove(k);
                }
                // NodePort services usually pin their node ports on purpose; for
                // the other types they're auto-assigned and may clash elsewhere.
                if service_type != "NodePort" {
                    if let Some(Value::Array(ports)) = spec.get_mut("ports") {
                        for p in ports.iter_mut().filter_map(Value::as_object_mut) {
                            p.remove("nodePort");
                        }
                    }
                }
            }
        }
        "PersistentVolumeClaim" => {
            if let Some(Value::Object(spec)) = obj.get_mut("spec") {
                spec.remove("volumeName"); // bind to a fresh volume on the target
            }
            if let Some(Value::Object(ann)) = obj.pointer_mut("/metadata/annotations") {
                for a in PVC_BINDING_ANNOTATIONS {
                    ann.remove(*a);
                }
            }
        }
        "ServiceAccount" => {
            if let Some(o) = obj.as_object_mut() {
                o.remove("secrets"); // auto-generated token references
            }
        }
        "Job" => {
            // The server generates the selector and its matching labels from
            // the Job's uid; applying them back is rejected unless the Job
            // manages its own selector.
            if obj.pointer("/spec/manualSelector").and_then(Value::as_bool) != Some(true) {
                if let Some(Value::Object(spec)) = obj.get_mut("spec") {
                    spec.remove("selector");
                }
                for labels in ["/metadata/labels", "/spec/template/metadata/labels"] {
                    if let Some(Value::Object(l)) = obj.pointer_mut(labels) {
                        for k in ["controller-uid", "batch.kubernetes.io/controller-uid", "job-name", "batch.kubernetes.io/job-name"] {
                            l.remove(k);
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// What `kubectl neat` would leave: something you can apply anywhere.
/// Drops status, every server-assigned identity field, and per-kind
/// cluster-assigned values (Service cluster IPs, PVC volume bindings…).
pub fn clean_for_export(obj: &mut Value) {
    strip_cluster_assigned(obj);
    if let Some(o) = obj.as_object_mut() {
        o.remove("status");
        if let Some(meta) = o.get_mut("metadata") {
            strip_meta(meta, false);
        }
        // Pod templates carry a null creationTimestamp that only adds noise.
        if let Some(Value::Object(tm)) = o
            .get_mut("spec")
            .and_then(|s| s.get_mut("template"))
            .and_then(|t| t.get_mut("metadata"))
        {
            tm.remove("creationTimestamp");
        }
        if let Some(Value::Object(tm)) = o
            .get_mut("spec")
            .and_then(|s| s.get_mut("jobTemplate"))
            .and_then(|t| t.get_mut("spec"))
            .and_then(|s| s.get_mut("template"))
            .and_then(|t| t.get_mut("metadata"))
        {
            tm.remove("creationTimestamp");
        }
    }
}

/// Editing view: like `kubectl edit`, minus status and managedFields. Keeps
/// `resourceVersion` so the save is rejected if the object changed meanwhile.
pub fn clean_for_edit(obj: &mut Value) {
    if let Some(o) = obj.as_object_mut() {
        o.remove("status");
        if let Some(meta) = o.get_mut("metadata") {
            strip_meta(meta, true);
        }
    }
}

/// Point a (cleaned) namespaced object at another namespace. A RoleBinding's
/// ServiceAccount subjects in the old namespace move with it, so the copy
/// grants the copied account rather than the original.
pub fn retarget_namespace(obj: &mut Value, namespace: &str) {
    let old = obj.pointer("/metadata/namespace").and_then(Value::as_str).map(str::to_string);
    if let Some(Value::Object(m)) = obj.get_mut("metadata") {
        m.insert("namespace".into(), Value::String(namespace.into()));
    }
    if obj.get("kind").and_then(Value::as_str) != Some("RoleBinding") {
        return;
    }
    let Some(old) = old.filter(|o| o != namespace) else { return };
    for s in obj.get_mut("subjects").and_then(Value::as_array_mut).into_iter().flatten() {
        if s.get("kind").and_then(Value::as_str) == Some("ServiceAccount") && s.get("namespace").and_then(Value::as_str) == Some(old.as_str()) {
            s["namespace"] = Value::String(namespace.into());
        }
    }
}

/// Something a workload's pod spec depends on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct ObjectRef {
    pub kind: String,
    pub name: String,
}

/// The pod spec inside any workload kind (or a bare Pod).
fn pod_spec(obj: &Value) -> Option<&Value> {
    let spec = obj.get("spec")?;
    if let Some(ps) = spec.get("template").and_then(|t| t.get("spec")) {
        return Some(ps); // Deployment, StatefulSet, DaemonSet, Job, ReplicaSet
    }
    if let Some(ps) = spec
        .get("jobTemplate")
        .and_then(|j| j.get("spec"))
        .and_then(|s| s.get("template"))
        .and_then(|t| t.get("spec"))
    {
        return Some(ps); // CronJob
    }
    spec.get("containers").map(|_| spec) // Pod
}

/// The ServiceAccount a workload's pods run as (`default` when unset).
pub fn service_account_name(obj: &Value) -> Option<String> {
    let ps = pod_spec(obj)?;
    Some(ps.get("serviceAccountName").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or("default").to_string())
}

/// Pull secrets a ServiceAccount hands to its pods.
fn account_pull_secrets(sa: &Value) -> Vec<String> {
    sa.get("imagePullSecrets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| r.get("name").and_then(Value::as_str))
        .filter(|n| !n.is_empty())
        .map(str::to_string)
        .collect()
}

/// ConfigMaps, Secrets, PVCs and the ServiceAccount a workload references,
/// deduplicated and sorted. For a ServiceAccount: its pull secrets.
pub fn references(obj: &Value) -> Vec<ObjectRef> {
    if obj.get("kind").and_then(Value::as_str) == Some("ServiceAccount") {
        let mut out: Vec<ObjectRef> = account_pull_secrets(obj).into_iter().map(|name| ObjectRef { kind: "Secret".into(), name }).collect();
        out.sort();
        out.dedup();
        return out;
    }
    let mut out = std::collections::BTreeSet::new();
    let Some(ps) = pod_spec(obj) else { return Vec::new() };
    let mut add = |kind: &str, name: Option<&str>| {
        if let Some(n) = name.filter(|n| !n.is_empty()) {
            out.insert(ObjectRef { kind: kind.into(), name: n.into() });
        }
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);

    if let Some(sa) = s(ps, "serviceAccountName").filter(|n| n != "default") {
        add("ServiceAccount", Some(&sa));
    }
    for ips in ps.get("imagePullSecrets").and_then(Value::as_array).into_iter().flatten() {
        add("Secret", s(ips, "name").as_deref());
    }
    for vol in ps.get("volumes").and_then(Value::as_array).into_iter().flatten() {
        add("ConfigMap", vol.get("configMap").and_then(|c| s(c, "name")).as_deref());
        add("Secret", vol.get("secret").and_then(|c| s(c, "secretName")).as_deref());
        add("PersistentVolumeClaim", vol.get("persistentVolumeClaim").and_then(|c| s(c, "claimName")).as_deref());
        for src in vol.get("projected").and_then(|p| p.get("sources")).and_then(Value::as_array).into_iter().flatten() {
            add("ConfigMap", src.get("configMap").and_then(|c| s(c, "name")).as_deref());
            add("Secret", src.get("secret").and_then(|c| s(c, "name")).as_deref());
        }
    }
    let containers = ["containers", "initContainers"]
        .iter()
        .filter_map(|k| ps.get(*k).and_then(Value::as_array))
        .flatten();
    for c in containers {
        for ef in c.get("envFrom").and_then(Value::as_array).into_iter().flatten() {
            add("ConfigMap", ef.get("configMapRef").and_then(|r| s(r, "name")).as_deref());
            add("Secret", ef.get("secretRef").and_then(|r| s(r, "name")).as_deref());
        }
        for e in c.get("env").and_then(Value::as_array).into_iter().flatten() {
            let vf = e.get("valueFrom");
            add("ConfigMap", vf.and_then(|v| v.get("configMapKeyRef")).and_then(|r| s(r, "name")).as_deref());
            add("Secret", vf.and_then(|v| v.get("secretKeyRef")).and_then(|r| s(r, "name")).as_deref());
        }
    }
    out.into_iter().collect()
}

/// An object that belongs with a workload when exporting or copying it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RelatedRef {
    pub kind: String,
    pub name: String,
    /// Why it's related, e.g. "selects its pods", "routes to Service web".
    pub reason: String,
    /// Exists in the workload's namespace (references can dangle).
    pub exists: bool,
    /// Holds credentials; never included unless asked for.
    pub sensitive: bool,
    /// Pre-ticked in the export/copy dialogs.
    pub default_selected: bool,
    /// Related through the ServiceAccount the pods run as ("ServiceAccount/x"),
    /// so every workload running as it needs it too.
    #[serde(default)]
    pub via: Option<String>,
}

/// The objects of a namespace that [`related_objects`] looks through.
#[derive(Debug, Default, Clone)]
pub struct NamespaceObjects {
    pub services: Vec<Value>,
    pub ingresses: Vec<Value>,
    pub hpas: Vec<Value>,
    pub service_accounts: Vec<Value>,
    pub role_bindings: Vec<Value>,
    pub roles: Vec<Value>,
}

fn name_at(v: &Value) -> Option<&str> {
    v.pointer("/metadata/name").and_then(Value::as_str)
}

/// A RoleBinding grants `account` (in `namespace`) its role. Subjects
/// without a namespace mean the binding's own.
pub fn binds_account(binding: &Value, namespace: &str, account: &str) -> bool {
    binding_accounts(binding).iter().any(|(ns, n)| ns.as_deref().unwrap_or(namespace) == namespace && n == account)
}

/// ServiceAccount subjects of a binding, as (namespace if set, name).
pub fn binding_accounts(binding: &Value) -> Vec<(Option<String>, String)> {
    binding
        .get("subjects")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|s| s.get("kind").and_then(Value::as_str) == Some("ServiceAccount"))
        .filter_map(|s| {
            let name = s.get("name").and_then(Value::as_str)?.to_string();
            let ns = s.get("namespace").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string);
            Some((ns, name))
        })
        .collect()
}

fn labels_at(v: &Value, pointer: &str) -> serde_json::Map<String, Value> {
    v.pointer(pointer).and_then(Value::as_object).cloned().unwrap_or_default()
}

/// Kubernetes equality-based selector: every selector label must match. An
/// empty selector matches nothing here (Services without selectors are
/// managed by hand).
pub fn selector_matches(selector: &serde_json::Map<String, Value>, labels: &serde_json::Map<String, Value>) -> bool {
    !selector.is_empty() && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
}

/// Labels the workload's pods get.
fn pod_template_labels(workload: &Value) -> serde_json::Map<String, Value> {
    for p in ["/spec/template/metadata/labels", "/spec/jobTemplate/spec/template/metadata/labels", "/metadata/labels"] {
        let l = labels_at(workload, p);
        if !l.is_empty() {
            return l;
        }
    }
    Default::default()
}

/// Everything that belongs with a workload: pod-spec references (ConfigMaps,
/// Secrets, ServiceAccount, PVCs), what its ServiceAccount brings (pull
/// secrets, RoleBindings to it and the Roles they grant), Services selecting
/// its pods, Ingresses routing to those Services, and HPAs scaling it. A
/// ServiceAccount itself gets its pull secrets, bindings and Roles. `ns` holds
/// the objects of the workload's namespace; `exists` answers for pod-spec
/// references and pull secrets.
pub fn related_objects(workload: &Value, ns: &NamespaceObjects, exists: impl Fn(&str, &str) -> bool) -> Vec<RelatedRef> {
    let mut out: Vec<RelatedRef> = Vec::new();
    let is_account = workload.get("kind").and_then(Value::as_str) == Some("ServiceAccount");
    for ObjectRef { kind, name } in references(workload) {
        let (reason, sensitive, default_selected) = match kind.as_str() {
            "Secret" if is_account => ("pull secret it gives its pods (contains credentials)", true, false),
            "Secret" => ("used by its pods (contains credentials)", true, false),
            "PersistentVolumeClaim" => ("mounted by its pods; data isn't included, only the claim", false, false),
            "ServiceAccount" => ("its pods run as this account", false, true),
            _ => ("used by its pods", false, true),
        };
        out.push(RelatedRef { exists: exists(&kind, &name), kind, name, reason: reason.into(), sensitive, default_selected, via: None });
    }

    // What the account brings. For a workload that's whatever its pods run as.
    let account = if is_account { name_at(workload).map(str::to_string) } else { service_account_name(workload) };
    if let Some(account) = account {
        let namespace = workload.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or_default();
        let via = (!is_account).then(|| format!("ServiceAccount/{account}"));
        // The default account isn't exported with a workload; its grants are
        // offered but left unticked, since the target has its own default.
        let pick = is_account || account != "default";
        let have = |out: &[RelatedRef], kind: &str, name: &str| out.iter().any(|r| r.kind == kind && r.name == name);
        if !is_account {
            let sa = ns.service_accounts.iter().find(|s| name_at(s) == Some(account.as_str()));
            for secret in sa.map(account_pull_secrets).unwrap_or_default() {
                if !have(&out, "Secret", &secret) {
                    out.push(RelatedRef {
                        exists: exists("Secret", &secret),
                        kind: "Secret".into(),
                        reason: format!("pull secret of ServiceAccount {account} (contains credentials)"),
                        name: secret,
                        sensitive: true,
                        default_selected: false,
                        via: via.clone(),
                    });
                }
            }
        }
        let mut roles: Vec<(String, String)> = Vec::new();
        for rb in ns.role_bindings.iter().filter(|rb| binds_account(rb, namespace, &account)) {
            let Some(rb_name) = name_at(rb) else { continue };
            out.push(RelatedRef {
                kind: "RoleBinding".into(),
                name: rb_name.into(),
                reason: format!("grants ServiceAccount {account} its permissions"),
                exists: true,
                sensitive: false,
                default_selected: pick,
                via: via.clone(),
            });
            if rb.pointer("/roleRef/kind").and_then(Value::as_str) == Some("Role") {
                if let Some(role) = rb.pointer("/roleRef/name").and_then(Value::as_str) {
                    roles.push((role.into(), rb_name.into()));
                }
            }
        }
        for (role, rb_name) in roles {
            if !have(&out, "Role", &role) {
                out.push(RelatedRef {
                    exists: ns.roles.iter().any(|r| name_at(r) == Some(role.as_str())),
                    kind: "Role".into(),
                    name: role,
                    reason: format!("granted by RoleBinding {rb_name}"),
                    sensitive: false,
                    default_selected: pick,
                    via: via.clone(),
                });
            }
        }
    }
    if is_account {
        return out;
    }

    let labels = pod_template_labels(workload);
    let mut service_names = Vec::new();
    for svc in &ns.services {
        let selector = labels_at(svc, "/spec/selector");
        if selector_matches(&selector, &labels) {
            if let Some(n) = name_at(svc) {
                service_names.push(n.to_string());
                out.push(RelatedRef {
                    kind: "Service".into(),
                    name: n.into(),
                    reason: "selects its pods".into(),
                    exists: true,
                    sensitive: false,
                    default_selected: true,
                    via: None,
                });
            }
        }
    }

    for ing in &ns.ingresses {
        let backends = ingress_backend_services(ing);
        if let Some(hit) = backends.iter().find(|b| service_names.contains(b)) {
            if let Some(n) = name_at(ing) {
                out.push(RelatedRef {
                    kind: "Ingress".into(),
                    name: n.into(),
                    reason: format!("routes to Service {hit}"),
                    exists: true,
                    sensitive: false,
                    default_selected: true,
                    via: None,
                });
            }
        }
    }

    let (wk, wn) = (workload.get("kind").and_then(Value::as_str).unwrap_or_default(), name_at(workload).unwrap_or_default());
    for hpa in &ns.hpas {
        let target_kind = hpa.pointer("/spec/scaleTargetRef/kind").and_then(Value::as_str);
        let target_name = hpa.pointer("/spec/scaleTargetRef/name").and_then(Value::as_str);
        if target_kind == Some(wk) && target_name == Some(wn) {
            if let Some(n) = name_at(hpa) {
                out.push(RelatedRef {
                    kind: "HorizontalPodAutoscaler".into(),
                    name: n.into(),
                    reason: "scales it".into(),
                    exists: true,
                    sensitive: false,
                    default_selected: true,
                    via: None,
                });
            }
        }
    }
    out
}

/// Service names an Ingress sends traffic to (rules and default backend).
pub fn ingress_backend_services(ingress: &Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(n) = ingress.pointer("/spec/defaultBackend/service/name").and_then(Value::as_str) {
        out.push(n.to_string());
    }
    for rule in ingress.pointer("/spec/rules").and_then(Value::as_array).into_iter().flatten() {
        for path in rule.pointer("/http/paths").and_then(Value::as_array).into_iter().flatten() {
            if let Some(n) = path.pointer("/backend/service/name").and_then(Value::as_str) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// A file (or pasted text) handed to import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    pub name: String,
    pub content: String,
}

/// One object found while parsing import sources, or why a document failed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestDoc {
    /// Position across all sources, stable for a given input (used to pick docs).
    pub index: usize,
    /// "file.yaml", or "file.yaml #2" for later documents in a multi-doc file.
    pub source: String,
    pub api_version: Option<String>,
    pub kind: Option<String>,
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub error: Option<String>,
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    path.iter().try_fold(v, |acc, k| acc.get(*k))?.as_str()
}

/// Split every source into objects: multi-document YAML is separated and
/// `kind: *List` wrappers (e.g. `kubectl get -o yaml`) are expanded. Bad
/// documents come back with `error` set instead of failing the whole import.
pub fn parse_sources(sources: &[SourceFile]) -> Vec<(ManifestDoc, Option<Value>)> {
    use serde::Deserialize as _;
    let mut out = Vec::new();
    for src in sources {
        let mut doc_no = 0usize;
        let mut push = |out: &mut Vec<(ManifestDoc, Option<Value>)>, v: Result<Value, String>| {
            doc_no += 1;
            let source = if doc_no == 1 { src.name.clone() } else { format!("{} #{doc_no}", src.name) };
            let index = out.len();
            let doc = match &v {
                Ok(val) => {
                    let api_version = str_at(val, &["apiVersion"]).map(str::to_string);
                    let kind = str_at(val, &["kind"]).map(str::to_string);
                    let name = str_at(val, &["metadata", "name"]).map(str::to_string);
                    let missing: Vec<&str> = [("apiVersion", api_version.is_none()), ("kind", kind.is_none()), ("metadata.name", name.is_none())]
                        .iter()
                        .filter(|(_, m)| *m)
                        .map(|(f, _)| *f)
                        .collect();
                    ManifestDoc {
                        index,
                        source,
                        namespace: str_at(val, &["metadata", "namespace"]).map(str::to_string),
                        error: (!missing.is_empty()).then(|| format!("missing {}", missing.join(", "))),
                        api_version,
                        kind,
                        name,
                    }
                }
                Err(e) => ManifestDoc { index, source, api_version: None, kind: None, name: None, namespace: None, error: Some(e.clone()) },
            };
            let value = if doc.error.is_none() { v.ok() } else { None };
            out.push((doc, value));
        };
        for de in serde_yaml::Deserializer::from_str(&src.content) {
            match Value::deserialize(de) {
                Ok(Value::Null) => {} // empty document (e.g. a trailing `---`)
                // A list wrapper (`List`, `DeploymentList`, …) has no name of
                // its own; a custom resource that happens to be called
                // `…List` and has an `items` field does.
                Ok(v) if str_at(&v, &["kind"]).is_some_and(|k| k.ends_with("List"))
                    && v.get("items").is_some_and(Value::is_array)
                    && str_at(&v, &["metadata", "name"]).is_none() =>
                {
                    for item in v["items"].as_array().cloned().unwrap_or_default() {
                        push(&mut out, Ok(item));
                    }
                }
                Ok(v) => push(&mut out, Ok(v)),
                Err(e) => {
                    push(&mut out, Err(format!("invalid YAML: {e}")));
                    break; // the parser can't resync after a syntax error
                }
            }
        }
    }
    out
}

/// Apply order so dependencies exist before the things that use them.
pub fn apply_order(kind: &str) -> u8 {
    match kind {
        "Namespace" => 0,
        "CustomResourceDefinition" => 1,
        "ServiceAccount" | "Role" | "ClusterRole" | "RoleBinding" | "ClusterRoleBinding" => 2,
        "ConfigMap" | "Secret" | "PersistentVolume" | "PersistentVolumeClaim" | "StorageClass" => 3,
        "Service" => 4,
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" | "CronJob" | "Pod" => 5,
        _ => 6, // Ingress, HPA, NetworkPolicy, CRs…
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn exported_job_can_be_applied_again() {
        let job = |manual: bool| {
            json!({
                "apiVersion": "batch/v1", "kind": "Job",
                "metadata": { "name": "migrate", "namespace": "apps", "labels": { "app": "migrate", "controller-uid": "u1" } },
                "spec": {
                    "manualSelector": manual,
                    "selector": { "matchLabels": { "batch.kubernetes.io/controller-uid": "u1" } },
                    "template": {
                        "metadata": { "labels": { "app": "migrate", "batch.kubernetes.io/controller-uid": "u1", "job-name": "migrate" } },
                        "spec": { "containers": [{ "name": "c", "image": "x" }], "restartPolicy": "Never" }
                    }
                }
            })
        };
        let mut generated = job(false);
        clean_for_export(&mut generated);
        assert!(generated.pointer("/spec/selector").is_none());
        assert_eq!(generated.pointer("/spec/template/metadata/labels"), Some(&json!({ "app": "migrate" })));
        assert_eq!(generated.pointer("/metadata/labels"), Some(&json!({ "app": "migrate" })));

        let mut manual = job(true);
        clean_for_export(&mut manual);
        assert!(manual.pointer("/spec/selector").is_some(), "a hand-written selector is the user's");
    }

    fn deployment() -> Value {
        json!({
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {
                "name": "web", "namespace": "apps", "uid": "u-1", "resourceVersion": "42",
                "generation": 3, "creationTimestamp": "2026-01-01T00:00:00Z",
                "managedFields": [{}],
                "annotations": { "deployment.kubernetes.io/revision": "3" },
                "labels": { "app": "web" }
            },
            "spec": {
                "replicas": 2,
                "template": {
                    "metadata": { "creationTimestamp": null, "labels": { "app": "web" } },
                    "spec": {
                        "serviceAccountName": "web-sa",
                        "imagePullSecrets": [{ "name": "ghcr" }],
                        "volumes": [
                            { "name": "cfg", "configMap": { "name": "web-config" } },
                            { "name": "data", "persistentVolumeClaim": { "claimName": "web-data" } }
                        ],
                        "containers": [{
                            "name": "app", "image": "web:1",
                            "envFrom": [{ "secretRef": { "name": "web-secrets" } }],
                            "env": [{ "name": "X", "valueFrom": { "configMapKeyRef": { "name": "web-config", "key": "x" } } }]
                        }]
                    }
                }
            },
            "status": { "readyReplicas": 2 }
        })
    }

    #[test]
    fn export_strips_server_fields() {
        let mut d = deployment();
        clean_for_export(&mut d);
        let m = &d["metadata"];
        assert!(d.get("status").is_none());
        for k in ["uid", "resourceVersion", "generation", "creationTimestamp", "managedFields", "annotations"] {
            assert!(m.get(k).is_none(), "{k} should be gone");
        }
        assert_eq!(m["labels"]["app"], "web", "labels survive");
        assert!(d["spec"]["template"]["metadata"].get("creationTimestamp").is_none());
        assert_eq!(d["spec"]["replicas"], 2);
    }

    #[test]
    fn edit_keeps_resource_version() {
        let mut d = deployment();
        clean_for_edit(&mut d);
        assert_eq!(d["metadata"]["resourceVersion"], "42");
        assert!(d["metadata"].get("managedFields").is_none());
        assert!(d.get("status").is_none());
    }

    #[test]
    fn finds_references() {
        let refs = references(&deployment());
        let names: Vec<String> = refs.iter().map(|r| format!("{}/{}", r.kind, r.name)).collect();
        assert_eq!(
            names,
            vec![
                "ConfigMap/web-config",
                "PersistentVolumeClaim/web-data",
                "Secret/ghcr",
                "Secret/web-secrets",
                "ServiceAccount/web-sa",
            ]
        );
    }

    #[test]
    fn retarget() {
        let mut d = deployment();
        clean_for_export(&mut d);
        retarget_namespace(&mut d, "staging");
        assert_eq!(d["metadata"]["namespace"], "staging");
    }

    fn src(name: &str, content: &str) -> SourceFile {
        SourceFile { name: name.into(), content: content.into() }
    }

    #[test]
    fn parses_multi_doc_lists_and_errors() {
        let multi = "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cfg\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\n  namespace: apps\n---\n";
        let list = "apiVersion: v1\nkind: List\nitems:\n- apiVersion: v1\n  kind: Service\n  metadata: {name: web}\n- apiVersion: v1\n  kind: Secret\n  metadata: {name: s}\n";
        let bad = "apiVersion: v1\nkind: ConfigMap\nmetadata: {}\n";
        let broken = "kind: [unclosed\n";
        let docs = parse_sources(&[src("a.yaml", multi), src("list.yaml", list), src("bad.yaml", bad), src("broken.yaml", broken)]);
        let summary: Vec<String> = docs
            .iter()
            .map(|(d, v)| format!("{}|{}|{}|{}", d.index, d.source, d.kind.clone().unwrap_or_default(), d.error.is_some() || v.is_none()))
            .collect();
        assert_eq!(
            summary,
            vec![
                "0|a.yaml|ConfigMap|false",
                "1|a.yaml #2|Deployment|false",
                "2|list.yaml|Service|false",
                "3|list.yaml #2|Secret|false",
                "4|bad.yaml|ConfigMap|true",
                "5|broken.yaml||true",
            ]
        );
        assert_eq!(docs[1].0.namespace.as_deref(), Some("apps"));
        assert_eq!(docs[4].0.error.as_deref(), Some("missing metadata.name"));
        assert!(docs[5].0.error.as_deref().unwrap().starts_with("invalid YAML"));
    }

    #[test]
    fn dependencies_apply_first() {
        let mut kinds = vec!["Deployment", "Ingress", "ConfigMap", "Namespace", "Service", "ServiceAccount"];
        kinds.sort_by_key(|k| apply_order(k));
        assert_eq!(kinds, vec!["Namespace", "ServiceAccount", "ConfigMap", "Service", "Deployment", "Ingress"]);
    }

    #[test]
    fn export_strips_per_kind_cluster_values() {
        let mut svc = json!({
            "apiVersion": "v1", "kind": "Service",
            "metadata": { "name": "web" },
            "spec": { "type": "LoadBalancer", "clusterIP": "10.43.0.9", "clusterIPs": ["10.43.0.9"],
                      "ports": [{ "port": 80, "nodePort": 31234 }], "selector": { "app": "web" } }
        });
        clean_for_export(&mut svc);
        assert!(svc["spec"].get("clusterIP").is_none() && svc["spec"].get("clusterIPs").is_none());
        assert!(svc["spec"]["ports"][0].get("nodePort").is_none(), "auto-assigned node port dropped");

        let mut np = json!({ "apiVersion": "v1", "kind": "Service", "metadata": { "name": "np" },
                             "spec": { "type": "NodePort", "ports": [{ "port": 80, "nodePort": 30080 }] } });
        clean_for_export(&mut np);
        assert_eq!(np["spec"]["ports"][0]["nodePort"], 30080, "NodePort services keep their pinned port");

        let mut pvc = json!({
            "apiVersion": "v1", "kind": "PersistentVolumeClaim",
            "metadata": { "name": "data", "annotations": { "pv.kubernetes.io/bind-completed": "yes", "team": "x" } },
            "spec": { "volumeName": "pvc-123", "resources": { "requests": { "storage": "1Gi" } } }
        });
        clean_for_export(&mut pvc);
        assert!(pvc["spec"].get("volumeName").is_none());
        assert_eq!(pvc["metadata"]["annotations"], json!({ "team": "x" }));
    }

    #[test]
    fn finds_related_services_ingresses_and_hpas() {
        let services = vec![
            json!({ "metadata": { "name": "web" }, "spec": { "selector": { "app": "web" } } }),
            json!({ "metadata": { "name": "other" }, "spec": { "selector": { "app": "other" } } }),
            json!({ "metadata": { "name": "manual" }, "spec": {} }),
        ];
        let ingresses = vec![
            json!({ "metadata": { "name": "web-ing" }, "spec": { "rules": [{ "http": { "paths": [{ "backend": { "service": { "name": "web" } } }] } }] } }),
            json!({ "metadata": { "name": "other-ing" }, "spec": { "defaultBackend": { "service": { "name": "other" } } } }),
        ];
        let hpas = vec![
            json!({ "metadata": { "name": "web-hpa" }, "spec": { "scaleTargetRef": { "kind": "Deployment", "name": "web" } } }),
            json!({ "metadata": { "name": "x-hpa" }, "spec": { "scaleTargetRef": { "kind": "Deployment", "name": "x" } } }),
        ];
        let ns = NamespaceObjects { services, ingresses, hpas, ..Default::default() };
        let rel = related_objects(&deployment(), &ns, |kind, _| kind != "PersistentVolumeClaim");
        let summary: Vec<String> = rel.iter().map(|r| format!("{}/{}:{}:{}", r.kind, r.name, r.default_selected, r.exists)).collect();
        assert_eq!(
            summary,
            vec![
                "ConfigMap/web-config:true:true",
                "PersistentVolumeClaim/web-data:false:false",
                "Secret/ghcr:false:true",
                "Secret/web-secrets:false:true",
                "ServiceAccount/web-sa:true:true",
                "Service/web:true:true",
                "Ingress/web-ing:true:true",
                "HorizontalPodAutoscaler/web-hpa:true:true",
            ]
        );
        assert!(rel.iter().filter(|r| r.kind == "Secret").all(|r| r.sensitive));
    }

    #[test]
    fn service_account_brings_pull_secrets_and_permissions() {
        let ns = NamespaceObjects {
            service_accounts: vec![json!({ "kind": "ServiceAccount", "metadata": { "name": "web-sa" }, "imagePullSecrets": [{ "name": "ghcr" }, { "name": "quay" }] })],
            role_bindings: vec![
                json!({ "metadata": { "name": "web-rb" }, "roleRef": { "kind": "Role", "name": "reader" },
                        "subjects": [{ "kind": "ServiceAccount", "name": "web-sa" }] }),
                json!({ "metadata": { "name": "web-view" }, "roleRef": { "kind": "ClusterRole", "name": "view" },
                        "subjects": [{ "kind": "ServiceAccount", "name": "web-sa", "namespace": "apps" }] }),
                json!({ "metadata": { "name": "elsewhere" }, "roleRef": { "kind": "Role", "name": "x" },
                        "subjects": [{ "kind": "ServiceAccount", "name": "web-sa", "namespace": "other" }] }),
                json!({ "metadata": { "name": "other-rb" }, "roleRef": { "kind": "Role", "name": "y" },
                        "subjects": [{ "kind": "User", "name": "web-sa" }] }),
            ],
            roles: vec![json!({ "metadata": { "name": "reader" } })],
            ..Default::default()
        };
        let rel = related_objects(&deployment(), &ns, |_, _| true);
        let via: Vec<String> = rel
            .iter()
            .filter(|r| r.via.is_some())
            .map(|r| format!("{}/{}:{}:{}", r.kind, r.name, r.default_selected, r.exists))
            .collect();
        // ghcr is already a pod-spec reference, so only quay comes through the account.
        assert_eq!(via, vec!["Secret/quay:false:true", "RoleBinding/web-rb:true:true", "RoleBinding/web-view:true:true", "Role/reader:true:true"]);
        assert!(rel.iter().all(|r| r.via.as_deref().is_none_or(|v| v == "ServiceAccount/web-sa")));

        let mut sa = ns.service_accounts[0].clone();
        sa["metadata"]["namespace"] = json!("apps");
        let own: Vec<String> = related_objects(&sa, &ns, |_, _| true).iter().map(|r| format!("{}/{}:{}", r.kind, r.name, r.via.is_some())).collect();
        assert_eq!(own, vec!["Secret/ghcr:false", "Secret/quay:false", "RoleBinding/web-rb:false", "RoleBinding/web-view:false", "Role/reader:false"]);
    }

    #[test]
    fn retargeted_role_binding_follows_its_accounts() {
        let mut rb = json!({ "kind": "RoleBinding", "metadata": { "name": "rb", "namespace": "apps" },
            "subjects": [
                { "kind": "ServiceAccount", "name": "web-sa", "namespace": "apps" },
                { "kind": "ServiceAccount", "name": "ci", "namespace": "tools" },
                { "kind": "ServiceAccount", "name": "local" },
            ] });
        retarget_namespace(&mut rb, "staging");
        assert_eq!(rb["metadata"]["namespace"], "staging");
        assert_eq!(rb["subjects"][0]["namespace"], "staging");
        assert_eq!(rb["subjects"][1]["namespace"], "tools", "accounts elsewhere stay put");
        assert!(rb["subjects"][2].get("namespace").is_none());
    }
}
