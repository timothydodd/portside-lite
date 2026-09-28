//! Archiving a workload: what goes in the archive, what else still uses its
//! objects (so they aren't deleted from under another workload), and which
//! stored log lines belong to it once its pods are gone.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::{ingress_backend_services, references, selector_matches, ObjectRef, RelatedRef};

/// Bumped when the on-disk layout of an archive changes.
pub const ARCHIVE_FORMAT: u32 = 1;

/// `archive.json`: what was archived, from where, and what happened to it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMeta {
    /// Folder path relative to the archive root, `/`-separated.
    pub id: String,
    pub format: u32,
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub cluster_id: String,
    /// Profile it was archived from (Restore's default target) and its name
    /// at the time, for display.
    #[serde(default)]
    pub profile_id: String,
    pub connection_name: String,
    pub archived_ms: i64,
    /// Replica count when archived (Deployments / StatefulSets).
    pub replicas: Option<i32>,
    pub images: Vec<String>,
    /// Everything in `manifest.yaml`, in apply order.
    pub objects: Vec<ArchivedObject>,
    /// Lines written to `logs.txt` (0 = no log file).
    pub log_lines: usize,
    pub restored_ms: Option<i64>,
    /// Profile name it was last restored to.
    pub restored_to: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivedObject {
    pub kind: String,
    pub name: String,
    /// Removed from the cluster when archived.
    pub removed: bool,
    /// Why removal failed, if it did.
    pub error: Option<String>,
}

/// A candidate for the archive dialog: a related object plus who else uses it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ArchivePlanItem {
    #[serde(flatten)]
    pub related: RelatedRef,
    /// Other workloads (or Services) that also use it, e.g. "Deployment/api".
    /// Non-empty means removing it would break them.
    pub used_by: Vec<String>,
}

/// One path segment safe on every filesystem (k8s names already are; the
/// cluster id has `:` and `@`).
fn safe_segment(s: &str) -> String {
    let out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '_' })
        .collect();
    match out.trim_matches('.') {
        "" => "_".into(),
        t => t.to_string(),
    }
}

/// `<cluster>/<namespace>/<kind>-<name>`: one folder per workload per
/// cluster, so re-archiving the same workload replaces its archive.
pub fn archive_id(cluster_id: &str, namespace: &str, kind: &str, name: &str) -> String {
    format!(
        "{}/{}/{}-{}",
        safe_segment(cluster_id),
        safe_segment(namespace),
        kind.to_ascii_lowercase(),
        safe_segment(name)
    )
}

/// An id handed back by the UI must stay inside the archive root.
pub fn valid_archive_id(id: &str) -> bool {
    let parts: Vec<&str> = id.split('/').collect();
    parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && *p == safe_segment(p) && *p != "." && *p != "..")
}

fn is_suffix(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// Whether a pod name has the shape its controller gives pods of `workload`.
/// Used for log lines stored before pod owners were recorded:
/// Deployment `<name>-<hash>-<5>`, StatefulSet `<name>-<ordinal>`,
/// DaemonSet / Job `<name>-<5>`, CronJob `<name>-<schedule ts>-<5>`.
pub fn pod_name_matches(kind: &str, workload: &str, pod: &str) -> bool {
    let Some(rest) = pod.strip_prefix(workload).and_then(|r| r.strip_prefix('-')) else { return false };
    let parts: Vec<&str> = rest.split('-').collect();
    match (kind, parts.as_slice()) {
        ("Deployment", [hash, id]) => is_suffix(hash) && hash.len() <= 10 && is_suffix(id) && id.len() == 5,
        ("StatefulSet", [ordinal]) => is_digits(ordinal),
        ("DaemonSet" | "Job", [id]) => is_suffix(id) && id.len() == 5,
        ("CronJob", [ts, id]) => is_digits(ts) && is_suffix(id) && id.len() == 5,
        _ => false,
    }
}

/// Whether a pod whose controller is `owner_kind/owner_name` (as the snapshot
/// reports it, ReplicaSets already folded into their Deployment) belongs to
/// `kind/name`. CronJob pods are owned by a Job named `<cronjob>-<ts>`.
pub fn owner_matches(kind: &str, name: &str, owner_kind: &str, owner_name: &str) -> bool {
    if kind == owner_kind && name == owner_name {
        return true;
    }
    kind == "CronJob"
        && owner_kind == "Job"
        && owner_name.strip_prefix(name).and_then(|r| r.strip_prefix('-')).is_some_and(is_digits)
}

fn pod_template_labels(workload: &Value) -> serde_json::Map<String, Value> {
    for p in ["/spec/template/metadata/labels", "/spec/jobTemplate/spec/template/metadata/labels"] {
        if let Some(l) = workload.pointer(p).and_then(Value::as_object).filter(|l| !l.is_empty()) {
            return l.clone();
        }
    }
    Default::default()
}

fn kind_name(v: &Value) -> (String, String) {
    let s = |p: &str| v.pointer(p).and_then(Value::as_str).unwrap_or_default().to_string();
    (s("/kind"), s("/metadata/name"))
}

/// For each related object, which *other* workloads in the namespace still
/// need it. `workloads` are the namespace's Deployments, StatefulSets,
/// DaemonSets, Jobs and CronJobs (the target among them is skipped, as are
/// objects with an owner, e.g. a CronJob's Jobs, whose owner speaks for
/// them). `services` and `ingresses` are the namespace's objects of those
/// kinds. An Ingress counts as shared when it also routes to Services that
/// aren't the target's.
pub fn plan_items(
    target: &Value,
    related: Vec<RelatedRef>,
    workloads: &[Value],
    services: &[Value],
    ingresses: &[Value],
) -> Vec<ArchivePlanItem> {
    let target_id = kind_name(target);
    let others: Vec<&Value> = workloads
        .iter()
        .filter(|w| kind_name(w) != target_id)
        .filter(|w| w.pointer("/metadata/ownerReferences").and_then(Value::as_array).is_none_or(|o| o.is_empty()))
        .collect();
    let label = |w: &Value| {
        let (k, n) = kind_name(w);
        format!("{k}/{n}")
    };
    let named = |list: &'_ [Value], name: &str| -> Option<Value> {
        list.iter().find(|v| v.pointer("/metadata/name").and_then(Value::as_str) == Some(name)).cloned()
    };
    let our_services: Vec<String> = related.iter().filter(|r| r.kind == "Service").map(|r| r.name.clone()).collect();
    let ours: Vec<&str> = our_services.iter().map(String::as_str).collect();

    related
        .into_iter()
        .map(|r| {
            let mut used_by: Vec<String> = match r.kind.as_str() {
                "Service" => {
                    let selector = named(services, &r.name)
                        .and_then(|s| s.pointer("/spec/selector").and_then(Value::as_object).cloned())
                        .unwrap_or_default();
                    others.iter().filter(|w| selector_matches(&selector, &pod_template_labels(w))).map(|w| label(w)).collect()
                }
                "Ingress" => named(ingresses, &r.name).map(|i| ingress_foreign_backends(&i, &ours)).unwrap_or_default(),
                "HorizontalPodAutoscaler" => Vec::new(),
                _ => {
                    let me = ObjectRef { kind: r.kind.clone(), name: r.name.clone() };
                    others.iter().filter(|w| references(w).contains(&me)).map(|w| label(w)).collect()
                }
            };
            used_by.sort();
            used_by.dedup();
            ArchivePlanItem { related: r, used_by }
        })
        .collect()
}

/// Services an Ingress routes to that aren't `ours`, as "Service/<name>".
pub fn ingress_foreign_backends(ingress: &Value, ours: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = ingress_backend_services(ingress)
        .into_iter()
        .filter(|s| !ours.contains(&s.as_str()))
        .map(|s| format!("Service/{s}"))
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ids_are_filesystem_safe_and_validated() {
        let id = archive_id("ssh:tim@k3s.lan:22", "apps", "Deployment", "web");
        assert_eq!(id, "ssh_tim_k3s.lan_22/apps/deployment-web");
        assert!(valid_archive_id(&id));
        for bad in ["../x/y", "a/b", "a/../c", "a/b/c/d", "a//c", "a/b/c d", "/a/b"] {
            assert!(!valid_archive_id(bad), "{bad} must be rejected");
        }
    }

    #[test]
    fn pod_names_match_their_controller() {
        assert!(pod_name_matches("Deployment", "web", "web-7d9f8b6c5-x2k4p"));
        assert!(!pod_name_matches("Deployment", "web", "web-api-7d9f8b6c5-x2k4p"), "a longer workload name isn't ours");
        assert!(!pod_name_matches("Deployment", "web", "web-x2k4p"));
        assert!(pod_name_matches("StatefulSet", "db", "db-0"));
        assert!(pod_name_matches("StatefulSet", "db", "db-12"));
        assert!(!pod_name_matches("StatefulSet", "db", "db-replica-0"));
        assert!(pod_name_matches("DaemonSet", "agent", "agent-q8z7w"));
        assert!(pod_name_matches("CronJob", "backup", "backup-29012345-abcde"));
        assert!(!pod_name_matches("CronJob", "backup", "backup-abcde"));
        assert!(!pod_name_matches("Deployment", "web", "webx-7d9f8b6c5-x2k4p"));
    }

    #[test]
    fn cronjob_owns_its_jobs_pods() {
        assert!(owner_matches("Deployment", "web", "Deployment", "web"));
        assert!(!owner_matches("Deployment", "web", "StatefulSet", "web"));
        assert!(owner_matches("CronJob", "backup", "Job", "backup-29012345"));
        assert!(!owner_matches("CronJob", "backup", "Job", "backup-manual"));
        assert!(!owner_matches("Job", "backup", "Job", "backup-29012345"));
    }

    fn rel(kind: &str, name: &str) -> RelatedRef {
        RelatedRef { kind: kind.into(), name: name.into(), reason: String::new(), exists: true, sensitive: false, default_selected: true }
    }

    fn workload(kind: &str, name: &str, app: &str, config: &str) -> Value {
        json!({
            "kind": kind, "metadata": { "name": name },
            "spec": { "template": {
                "metadata": { "labels": { "app": app } },
                "spec": { "containers": [{ "name": "c", "envFrom": [{ "configMapRef": { "name": config } }] }] }
            } }
        })
    }

    #[test]
    fn flags_objects_other_workloads_still_use() {
        let target = workload("Deployment", "web", "web", "shared-config");
        let workloads = vec![
            target.clone(),
            workload("Deployment", "worker", "worker", "shared-config"),
            workload("StatefulSet", "web-canary", "web", "own-config"),
            // A CronJob's Job: its owner speaks for it.
            json!({ "kind": "Job", "metadata": { "name": "j-1", "ownerReferences": [{ "kind": "CronJob", "name": "j" }] },
                    "spec": { "template": { "spec": { "containers": [{ "name": "c", "envFrom": [{ "configMapRef": { "name": "web-only" } }] }] } } } }),
        ];
        let services = vec![json!({ "metadata": { "name": "web" }, "spec": { "selector": { "app": "web" } } })];
        let ingresses = vec![
            json!({ "metadata": { "name": "web-ing" }, "spec": { "defaultBackend": { "service": { "name": "web" } } } }),
            json!({ "metadata": { "name": "site" }, "spec": { "rules": [{ "http": { "paths": [
                { "backend": { "service": { "name": "web" } } }, { "backend": { "service": { "name": "blog" } } }
            ] } }] } }),
        ];
        let related = vec![
            rel("ConfigMap", "shared-config"),
            rel("ConfigMap", "web-only"),
            rel("Service", "web"),
            rel("Ingress", "web-ing"),
            rel("Ingress", "site"),
            rel("HorizontalPodAutoscaler", "web"),
        ];
        let plan = plan_items(&target, related, &workloads, &services, &ingresses);
        let used: Vec<(String, Vec<String>)> = plan.into_iter().map(|p| (format!("{}/{}", p.related.kind, p.related.name), p.used_by)).collect();
        assert_eq!(
            used,
            vec![
                ("ConfigMap/shared-config".into(), vec!["Deployment/worker".to_string()]),
                ("ConfigMap/web-only".into(), vec![]),
                ("Service/web".into(), vec!["StatefulSet/web-canary".to_string()]),
                ("Ingress/web-ing".into(), vec![]),
                ("Ingress/site".into(), vec!["Service/blog".to_string()]),
                ("HorizontalPodAutoscaler/web".into(), vec![]),
            ]
        );
    }
}
