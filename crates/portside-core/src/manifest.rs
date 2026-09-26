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

/// What `kubectl neat` would leave: something you can apply anywhere.
/// Drops status and every server-assigned identity field.
pub fn clean_for_export(obj: &mut Value) {
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

/// Point a (cleaned) namespaced object at another namespace.
pub fn retarget_namespace(obj: &mut Value, namespace: &str) {
    if let Some(Value::Object(m)) = obj.get_mut("metadata") {
        m.insert("namespace".into(), Value::String(namespace.into()));
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

/// ConfigMaps, Secrets, PVCs and the ServiceAccount a workload references,
/// deduplicated and sorted.
pub fn references(obj: &Value) -> Vec<ObjectRef> {
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
                Ok(v) if str_at(&v, &["kind"]).is_some_and(|k| k.ends_with("List")) && v.get("items").is_some_and(Value::is_array) => {
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
}
