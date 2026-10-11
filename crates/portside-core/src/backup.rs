//! Volume backups. Each backup is a CronJob that copies one
//! PersistentVolumeClaim into a dated `.tar.gz` on its own backup claim and
//! keeps the newest N, so backups keep happening while Portside isn't
//! running. Taken offline (the default), the job scales the workloads using
//! the volume to 0 first and back up straight after the copy, so nothing is
//! writing while it reads. This module builds those objects and reads them
//! back; the cluster calls live in `portside-kube/backups.rs`.

use k8s_openapi::api::batch::v1::CronJob;
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, ServiceAccount};
use k8s_openapi::api::rbac::v1::{Role, RoleBinding};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};
use serde_json::json;


/// On every object a backup creates and on its runs' pods; the value is the backup's name.
pub const BACKUP_LABEL: &str = "portside-lite/backup";
/// The [`BackupSpec`] as JSON, on the CronJob, so Change can load it back.
pub const SPEC_ANNOTATION: &str = "portside-lite/backup-spec";
/// Set (to the backup's name) on a workload a run scaled to 0, next to
/// [`crate::models::DISABLED_REPLICAS_ANNOTATION`] with its replica count; both are removed
/// when the run scales it back. Still there after the run = it was left offline.
pub const OFFLINE_ANNOTATION: &str = "portside-lite/backup-offline";
/// Every object a backup creates is named `portside-backup-<name>`.
pub const OBJECT_PREFIX: &str = "portside-backup-";
/// A CronJob name can be at most 52 characters (its Jobs add 11).
pub const MAX_NAME_LEN: usize = 52 - OBJECT_PREFIX.len();
/// Has `sh`, `tar`, `gzip` (busybox) and `curl` for the API calls that stop
/// and start the workloads.
pub const DEFAULT_IMAGE: &str = "curlimages/curl:8.16.0";
pub const ARCHIVE_SUFFIX: &str = ".tar.gz";
/// Where the volume being backed up (read-only) and the backup claim are mounted.
pub const DATA_MOUNT: &str = "/data";
pub const BACKUP_MOUNT: &str = "/backup";
/// Workload kinds a run can stop: they have a replica count.
pub const STOPPABLE: &[&str] = &["Deployment", "StatefulSet"];
/// A run that takes longer than this is stopped and counts as failed (the
/// workloads are still started again: the script catches the stop).
const RUN_DEADLINE_SECS: i64 = 6 * 60 * 60;
/// How long a run waits for the stopped workloads' pods to go.
const STOP_WAIT_SECS: u32 = 600;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupSpec {
    pub namespace: String,
    /// Names the objects (`portside-backup-<name>`) and starts every archive's file name.
    pub name: String,
    /// The PersistentVolumeClaim being backed up.
    pub claim: String,
    /// Cron expression, in `time_zone`.
    pub schedule: String,
    /// IANA name ("Europe/London"); `None` = the cluster's (UTC on k3s).
    #[serde(default)]
    pub time_zone: Option<String>,
    /// Archives kept on the backup claim; older ones are deleted after each run.
    pub keep: u32,
    /// Raise a problem when there's been no successful run for this long. 0 = never.
    #[serde(default)]
    pub max_age_hours: u32,
    /// Size of the backup claim. Only used when the claim is created.
    pub claim_size: String,
    #[serde(default)]
    pub storage_class: Option<String>,
    /// Scale `workloads` to 0 for the copy and back up afterwards.
    pub offline: bool,
    /// "Deployment/web": the workloads stopped while `offline`.
    #[serde(default)]
    pub workloads: Vec<String>,
    #[serde(default = "default_image")]
    pub image: String,
}

fn default_image() -> String {
    DEFAULT_IMAGE.into()
}

pub fn object_name(name: &str) -> String {
    format!("{OBJECT_PREFIX}{name}")
}

/// The backup name from one of its objects' names.
pub fn backup_name(object: &str) -> Option<&str> {
    object.strip_prefix(OBJECT_PREFIX).filter(|n| !n.is_empty())
}

pub fn is_backup(meta: &ObjectMeta) -> bool {
    meta.labels.as_ref().is_some_and(|l| l.contains_key(BACKUP_LABEL))
}

/// The spec a backup CronJob was made from.
pub fn spec_of(cron: &CronJob) -> Option<BackupSpec> {
    let raw = cron.metadata.annotations.as_ref()?.get(SPEC_ANNOTATION)?;
    serde_json::from_str(raw).ok()
}

// --- validation ---------------------------------------------------------------------

fn dns_label(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 63
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !s.starts_with('-')
        && !s.ends_with('-')
}

fn dns_subdomain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.split('.').all(dns_label)
}

const CRON_MACROS: &[&str] = &["@yearly", "@annually", "@monthly", "@weekly", "@daily", "@midnight", "@hourly"];

fn valid_schedule(s: &str) -> bool {
    let s = s.trim();
    if CRON_MACROS.contains(&s) {
        return true;
    }
    let fields: Vec<&str> = s.split_whitespace().collect();
    fields.len() == 5 && fields.iter().all(|f| f.bytes().all(|b| b.is_ascii_alphanumeric() || b"*/,-?".contains(&b)))
}

/// Everything the cluster would reject or the script relies on, with a
/// message that says which field is wrong.
pub fn validate(spec: &BackupSpec) -> Result<(), String> {
    if !dns_label(&spec.namespace) {
        return Err("Pick a namespace.".into());
    }
    if !dns_label(&spec.name) || spec.name.len() > MAX_NAME_LEN {
        return Err(format!(
            "The backup name must be lowercase letters, digits and dashes, at most {MAX_NAME_LEN} characters, starting and ending with a letter or digit."
        ));
    }
    if !dns_subdomain(&spec.claim) {
        return Err("Pick the volume to back up.".into());
    }
    if backup_name(&spec.claim).is_some() {
        return Err(format!("{} holds backups already; back up the volume it was made from.", spec.claim));
    }
    if !valid_schedule(&spec.schedule) {
        return Err(format!("\"{}\" isn't a cron schedule (five fields: minute hour day month weekday).", spec.schedule));
    }
    if let Some(tz) = spec.time_zone.as_deref() {
        let ok = !tz.is_empty() && !tz.contains("..") && tz.bytes().all(|b| b.is_ascii_alphanumeric() || b"_+-/".contains(&b));
        if !ok {
            return Err(format!("\"{tz}\" isn't a time zone name like Europe/London."));
        }
    }
    if !(1..=1000).contains(&spec.keep) {
        return Err("Keep between 1 and 1000 backups.".into());
    }
    if crate::quantity::parse_memory(&spec.claim_size) <= 0.0 {
        return Err(format!("\"{}\" isn't a size like 10Gi or 500Mi.", spec.claim_size));
    }
    if let Some(sc) = spec.storage_class.as_deref() {
        if !dns_subdomain(sc) {
            return Err(format!("\"{sc}\" isn't a storage class name."));
        }
    }
    if spec.image.trim().is_empty() || spec.image.chars().any(char::is_whitespace) {
        return Err("Enter the image to run the backup with.".into());
    }
    for w in &spec.workloads {
        let ok = w.split_once('/').is_some_and(|(kind, name)| STOPPABLE.contains(&kind) && dns_subdomain(name));
        if !ok {
            return Err(format!("\"{w}\" can't be stopped for a backup: only Deployments and StatefulSets can."));
        }
    }
    if spec.offline && spec.workloads.is_empty() {
        return Err("Nothing to take offline: no Deployment or StatefulSet uses this volume. Turn off \"Take offline\".".into());
    }
    Ok(())
}

// --- the script -----------------------------------------------------------------------

/// Stop the workloads given as arguments (`Kind/name`), copy `/data` to a
/// dated archive, start them again, check the archive, keep the newest `KEEP`.
///
/// - Stopping = one merge patch setting replicas to 0 and recording the old
///   count (the same annotation "Scale to 0" uses, so the UI can restore it)
///   plus [`OFFLINE_ANNOTATION`]. A workload already at 0 is left alone,
///   unless an earlier run left it offline; then its recorded count is used.
/// - Starting again happens right after the copy, before the checks, and in
///   an EXIT trap too, so a failure or a TERM (deadline, deletion) still
///   brings the app back.
/// - The archive is written to `<file>.part` and renamed once it's verified,
///   so a reader never sees a partial file.
/// - Arguments and env vars only; nothing is spliced into the shell text.
const SCRIPT: &str = r#"set -eu
umask 077
SA=/var/run/secrets/kubernetes.io/serviceaccount
API=https://kubernetes.default.svc
api() { curl -sSf --max-time 30 --cacert "$SA/ca.crt" -H "Authorization: Bearer $(cat "$SA/token")" "$@"; }
patch() { api -X PATCH -H 'Content-Type: application/merge-patch+json' --data "$2" "$1" > /dev/null; }
url() {
  case "$1" in
    Deployment/*) echo "$API/apis/apps/v1/namespaces/$NAMESPACE/deployments/${1#*/}" ;;
    StatefulSet/*) echo "$API/apis/apps/v1/namespaces/$NAMESPACE/statefulsets/${1#*/}" ;;
    *) echo "Can't stop $1: only Deployments and StatefulSets can be stopped." >&2; return 1 ;;
  esac
}

stopped=""
start_again() {
  for entry in $stopped; do
    w=${entry%=*}
    n=${entry##*=}
    if patch "$(url "$w")" "{\"spec\":{\"replicas\":$n},\"metadata\":{\"annotations\":{\"portside-lite/disabled-replicas\":null,\"portside-lite/backup-offline\":null}}}"; then
      echo "Started $w again ($n replicas)"
    else
      echo "Couldn't start $w again. Scale it back to $n replicas by hand." >&2
    fi
  done
  stopped=""
}
trap start_again EXIT
trap 'exit 143' TERM INT

selectors=""
for w in "$@"; do
  u=$(url "$w")
  obj=$(api "$u")
  scale=$(api "$u/scale")
  current=$(printf '%s' "$scale" | sed -n 's/.*"spec":{"replicas":\([0-9][0-9]*\)}.*/\1/p')
  current=${current:-0}
  if [ "$current" -gt 0 ]; then
    patch "$u" "{\"spec\":{\"replicas\":0},\"metadata\":{\"annotations\":{\"portside-lite/disabled-replicas\":\"$current\",\"portside-lite/backup-offline\":\"$BACKUP_NAME\"}}}"
    stopped="$stopped $w=$current"
    echo "Stopped $w (was $current replicas)"
  elif printf '%s' "$obj" | grep -q '"portside-lite/backup-offline"'; then
    current=$(printf '%s' "$obj" | sed -n 's|.*"portside-lite/disabled-replicas":"\([0-9][0-9]*\)".*|\1|p')
    if [ -n "$current" ]; then
      stopped="$stopped $w=$current"
      echo "$w was left stopped by an earlier backup; it will be started again ($current replicas)"
    fi
  else
    echo "$w is already at 0 replicas; leaving it as it is"
  fi
  sel=$(printf '%s' "$scale" | sed -n 's/.*"selector":"\([^"]*\)".*/\1/p')
  if [ -n "$sel" ]; then selectors="$selectors
$sel"; fi
done

if [ -n "$selectors" ]; then
  echo "Waiting for their pods to stop"
  waited=0
  printf '%s\n' "$selectors" | while read -r sel; do
    [ -n "$sel" ] || continue
    while ! api -G --data-urlencode "labelSelector=$sel" "$API/api/v1/namespaces/$NAMESPACE/pods" | grep -q '"items":\[\]'; do
      waited=$((waited + 3))
      if [ "$waited" -gt "$STOP_WAIT" ]; then echo "Their pods didn't stop within $STOP_WAIT s." >&2; exit 1; fi
      sleep 3
    done
  done
fi

cd /backup
rm -f -- ./*.part
out="$BACKUP_NAME-$(date -u +%Y%m%d-%H%M%S).tar.gz"
echo "Copying $(du -sk /data | cut -f1) KiB ($(df -k /backup | awk 'NR==2 {print $4}') KiB free for backups)"
if ! tar -czf "$out.part" -C /data .; then
  echo "The copy failed. Nothing was saved." >&2
  rm -f -- "$out.part"
  exit 1
fi
start_again
if ! gzip -t "$out.part"; then
  echo "The archive is damaged. Nothing was saved." >&2
  rm -f -- "$out.part"
  exit 1
fi
mv -- "$out.part" "$out"
echo "Saved $out ($(wc -c < "$out") bytes)"
ls -1 -- "$BACKUP_NAME"-*.tar.gz | sort -r | tail -n +"$((KEEP + 1))" | while read -r old; do
  rm -f -- "$old"
  echo "Removed $old (keeping the newest $KEEP)"
done
"#;

// --- objects -----------------------------------------------------------------------

fn labels(spec: &BackupSpec) -> serde_json::Value {
    json!({ BACKUP_LABEL: spec.name, "app.kubernetes.io/managed-by": "portside-lite" })
}

fn meta(spec: &BackupSpec) -> serde_json::Value {
    json!({ "name": object_name(&spec.name), "namespace": spec.namespace, "labels": labels(spec) })
}

/// The claim the archives are written to.
pub fn backup_claim(spec: &BackupSpec) -> PersistentVolumeClaim {
    let mut pvc = json!({
        "apiVersion": "v1",
        "kind": "PersistentVolumeClaim",
        "metadata": meta(spec),
        "spec": {
            "accessModes": ["ReadWriteOnce"],
            "resources": { "requests": { "storage": spec.claim_size } }
        }
    });
    if let Some(sc) = &spec.storage_class {
        pvc["spec"]["storageClassName"] = json!(sc);
    }
    serde_json::from_value(pvc).expect("backup claim is valid")
}

/// Offline backups only: the account the job stops and starts the workloads
/// as. Its Role can read and patch exactly those workloads, and list pods.
pub fn service_account(spec: &BackupSpec) -> ServiceAccount {
    serde_json::from_value(json!({ "apiVersion": "v1", "kind": "ServiceAccount", "metadata": meta(spec) })).expect("service account is valid")
}

pub fn role(spec: &BackupSpec) -> Role {
    let names = |kind: &str| -> Vec<String> {
        spec.workloads.iter().filter_map(|w| w.strip_prefix(kind)?.strip_prefix('/').map(str::to_string)).collect()
    };
    let mut rules = vec![json!({ "apiGroups": [""], "resources": ["pods"], "verbs": ["list"] })];
    for (kind, resource) in [("Deployment", "deployments"), ("StatefulSet", "statefulsets")] {
        let n = names(kind);
        if !n.is_empty() {
            rules.push(json!({ "apiGroups": ["apps"], "resources": [resource], "resourceNames": n, "verbs": ["get", "patch"] }));
            rules.push(json!({ "apiGroups": ["apps"], "resources": [format!("{resource}/scale")], "resourceNames": n, "verbs": ["get"] }));
        }
    }
    serde_json::from_value(json!({ "apiVersion": "rbac.authorization.k8s.io/v1", "kind": "Role", "metadata": meta(spec), "rules": rules }))
        .expect("role is valid")
}

pub fn role_binding(spec: &BackupSpec) -> RoleBinding {
    let name = object_name(&spec.name);
    serde_json::from_value(json!({
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": meta(spec),
        "roleRef": { "apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": name },
        "subjects": [{ "kind": "ServiceAccount", "name": name, "namespace": spec.namespace }]
    }))
    .expect("role binding is valid")
}

/// The CronJob that makes the backups.
pub fn backup_cronjob(spec: &BackupSpec) -> CronJob {
    let stop: &[String] = if spec.offline { &spec.workloads } else { &[] };
    let mut command = vec!["sh".to_string(), "-c".into(), SCRIPT.into(), "sh".into()];
    command.extend(stop.iter().cloned());
    let mut pod = json!({
        "restartPolicy": "Never",
        "automountServiceAccountToken": spec.offline,
        "enableServiceLinks": false,
        // The volume may live on a tainted node; go wherever it is.
        "tolerations": [{ "operator": "Exists" }],
        "containers": [{
            "name": "backup",
            "image": spec.image,
            "command": command,
            "env": [
                { "name": "BACKUP_NAME", "value": spec.name },
                { "name": "NAMESPACE", "value": spec.namespace },
                { "name": "KEEP", "value": spec.keep.to_string() },
                { "name": "STOP_WAIT", "value": STOP_WAIT_SECS.to_string() }
            ],
            // Root, to read every file whatever its owner (the image's default user isn't).
            "securityContext": { "runAsUser": 0, "runAsGroup": 0 },
            "volumeMounts": [
                { "name": "data", "mountPath": DATA_MOUNT, "readOnly": true },
                { "name": "backup", "mountPath": BACKUP_MOUNT }
            ],
            "resources": {
                "requests": { "cpu": "50m", "memory": "32Mi" },
                "limits": { "memory": "256Mi" }
            }
        }],
        "volumes": [
            { "name": "data", "persistentVolumeClaim": { "claimName": spec.claim, "readOnly": true } },
            { "name": "backup", "persistentVolumeClaim": { "claimName": object_name(&spec.name) } }
        ]
    });
    if spec.offline {
        pod["serviceAccountName"] = json!(object_name(&spec.name));
    }
    let mut cron = json!({
        "apiVersion": "batch/v1",
        "kind": "CronJob",
        "metadata": {
            "name": object_name(&spec.name),
            "namespace": spec.namespace,
            "labels": labels(spec),
            "annotations": { SPEC_ANNOTATION: serde_json::to_string(spec).expect("spec serializes") }
        },
        "spec": {
            "schedule": spec.schedule.trim(),
            // One run at a time; a missed run (cluster down at the time) still
            // happens if the cluster is back within the hour.
            "concurrencyPolicy": "Forbid",
            "startingDeadlineSeconds": 3600,
            "successfulJobsHistoryLimit": 3,
            "failedJobsHistoryLimit": 3,
            "jobTemplate": {
                "metadata": { "labels": labels(spec) },
                "spec": {
                    // No retry: a second try would stop the app again.
                    "backoffLimit": 0,
                    "activeDeadlineSeconds": RUN_DEADLINE_SECS,
                    "template": { "metadata": { "labels": labels(spec) }, "spec": pod }
                }
            }
        }
    });
    if let Some(tz) = &spec.time_zone {
        cron["spec"]["timeZone"] = json!(tz);
    }
    serde_json::from_value(cron).expect("backup cronjob is valid")
}

// --- manifest backups (made on this PC) ---------------------------------------------

/// Namespaced kinds a manifest backup saves from every namespace. Never Secrets.
pub const MANIFEST_KINDS: &[&str] = &[
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "CronJob",
    "Service",
    "ConfigMap",
    "PersistentVolumeClaim",
    "Ingress",
    "HorizontalPodAutoscaler",
    "ServiceAccount",
    "Role",
    "RoleBinding",
    // How k3s's own add-ons (Traefik and so on) are installed and configured.
    "HelmChart",
    "HelmChartConfig",
];

/// Cluster-wide settings a manifest backup saves.
pub const CLUSTER_KINDS: &[&str] = &[
    "Namespace",
    "StorageClass",
    "IngressClass",
    "PriorityClass",
    "ClusterRole",
    "ClusterRoleBinding",
    "CustomResourceDefinition",
];

/// Kinds a cluster may not serve or this connection may not list; the
/// backup notes them and carries on.
pub const OPTIONAL_KINDS: &[&str] = &["HelmChart", "HelmChartConfig", "IngressClass", "PriorityClass", "CustomResourceDefinition"];

/// Worth saving: not made by a controller (its owner recreates it) and not
/// one the cluster creates for itself.
pub fn keep_in_manifest_backup(kind: &str, obj: &serde_json::Value) -> bool {
    let meta = &obj["metadata"];
    let name = meta["name"].as_str().unwrap_or_default();
    let controlled = meta["ownerReferences"].as_array().is_some_and(|o| o.iter().any(|r| r["controller"] == json!(true)));
    if controlled {
        return false;
    }
    match kind {
        "ConfigMap" => name != "kube-root-ca.crt",
        "ServiceAccount" => name != "default",
        "Service" => !(name == "kubernetes" && meta["namespace"] == json!("default")),
        "Namespace" => !matches!(name, "default" | "kube-system" | "kube-public" | "kube-node-lease"),
        "PriorityClass" => !name.starts_with("system-"),
        "ClusterRole" | "ClusterRoleBinding" => {
            !name.starts_with("system:") && meta["labels"]["kubernetes.io/bootstrapping"] != json!("rbac-defaults")
        }
        _ => true,
    }
}

/// A line for `secrets-needed.md`: where the Secret is, its type and key
/// names. Never values. `None` for ones the cluster or Helm makes.
pub fn secret_line(secret: &serde_json::Value) -> Option<String> {
    let ty = secret["type"].as_str().unwrap_or("Opaque");
    if matches!(ty, "helm.sh/release.v1" | "kubernetes.io/service-account-token" | "bootstrap.kubernetes.io/token") {
        return None;
    }
    let meta = &secret["metadata"];
    let mut keys: Vec<&str> = secret["data"].as_object().map(|d| d.keys().map(String::as_str).collect()).unwrap_or_default();
    keys.sort_unstable();
    Some(format!(
        "- `{}/{}` ({ty}): {}",
        meta["namespace"].as_str().unwrap_or_default(),
        meta["name"].as_str().unwrap_or_default(),
        if keys.is_empty() { "no keys".to_string() } else { keys.join(", ") }
    ))
}

/// A filled-in spec for tests here and in other modules.
#[cfg(test)]
pub(crate) fn sample_spec() -> BackupSpec {
    BackupSpec {
        namespace: "shop".into(),
        name: "uploads".into(),
        claim: "uploads".into(),
        schedule: "30 1 * * *".into(),
        time_zone: Some("Europe/London".into()),
        keep: 7,
        max_age_hours: 36,
        claim_size: "20Gi".into(),
        storage_class: Some("local-path".into()),
        offline: true,
        workloads: vec!["Deployment/web".into(), "StatefulSet/db".into()],
        image: DEFAULT_IMAGE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::DISABLED_REPLICAS_ANNOTATION;

    fn spec() -> BackupSpec {
        sample_spec()
    }

    #[test]
    fn validation() {
        assert_eq!(validate(&spec()), Ok(()));
        let bad = |f: fn(&mut BackupSpec)| {
            let mut s = spec();
            f(&mut s);
            validate(&s).unwrap_err()
        };
        assert!(bad(|s| s.name = "My_Files".into()).contains("lowercase"));
        assert!(bad(|s| s.name = "a".repeat(MAX_NAME_LEN + 1)).contains("at most"));
        assert!(bad(|s| s.claim = "portside-backup-x".into()).contains("holds backups"));
        assert!(bad(|s| s.schedule = "every night".into()).contains("cron"));
        assert!(bad(|s| s.time_zone = Some("../etc".into())).contains("time zone"));
        assert!(bad(|s| s.keep = 0).contains("Keep"));
        assert!(bad(|s| s.claim_size = "lots".into()).contains("size"));
        assert!(bad(|s| s.workloads.push("DaemonSet/agent".into())).contains("only Deployments"));
        assert!(bad(|s| s.workloads.push("Deployment/x; rm -rf /".into())).contains("only Deployments"));
        assert!(bad(|s| s.workloads.clear()).contains("Nothing to take offline"));
        let mut live = spec();
        live.offline = false;
        live.workloads.clear();
        live.schedule = "@daily".into();
        assert_eq!(validate(&live), Ok(()));
    }

    #[test]
    fn offline_cronjob() {
        let s = spec();
        let cron = backup_cronjob(&s);
        assert_eq!(cron.metadata.name.as_deref(), Some("portside-backup-uploads"));
        assert_eq!(spec_of(&cron), Some(s.clone()));
        assert!(is_backup(&cron.metadata));
        let cs = &cron.spec;
        assert_eq!(cs.schedule, "30 1 * * *");
        assert_eq!(cs.time_zone.as_deref(), Some("Europe/London"));
        assert_eq!(cs.concurrency_policy.as_deref(), Some("Forbid"));
        let job = cs.job_template.spec.as_ref().unwrap();
        assert_eq!(job.backoff_limit, Some(0), "never stops the app twice in one run");
        let pod = job.template.spec.as_ref().unwrap();
        assert_eq!(pod.service_account_name.as_deref(), Some("portside-backup-uploads"));
        assert_eq!(pod.automount_service_account_token, Some(true));
        let vols = pod.volumes.as_ref().unwrap();
        let data = vols[0].persistent_volume_claim.as_ref().unwrap();
        assert_eq!((data.claim_name.as_str(), data.read_only), ("uploads", Some(true)), "the volume is only ever read");
        assert_eq!(vols[1].persistent_volume_claim.as_ref().unwrap().claim_name, "portside-backup-uploads");
        let c = &pod.containers[0];
        let cmd = c.command.as_ref().unwrap();
        assert_eq!(&cmd[3..], ["sh", "Deployment/web", "StatefulSet/db"], "workloads are positional arguments, not shell text");
        assert_eq!(c.env.as_ref().unwrap().iter().find(|e| e.name == "KEEP").unwrap().value.as_deref(), Some("7"));
    }

    #[test]
    fn live_cronjob_has_no_api_access() {
        let mut s = spec();
        s.offline = false;
        let pod = backup_cronjob(&s).spec.job_template.spec.unwrap().template.spec.unwrap();
        assert_eq!(pod.automount_service_account_token, Some(false));
        assert_eq!(pod.service_account_name, None);
        assert_eq!(&pod.containers[0].command.as_ref().unwrap()[3..], ["sh"], "nothing to stop");
    }

    #[test]
    fn role_covers_only_the_stopped_workloads() {
        let r = role(&spec());
        let rules = r.rules.unwrap();
        let find = |res: &str| rules.iter().find(|r| r.resources.as_ref().unwrap()[0] == res).unwrap();
        assert_eq!(find("deployments").resource_names.as_deref(), Some(&["web".to_string()][..]));
        assert_eq!(find("statefulsets/scale").resource_names.as_deref(), Some(&["db".to_string()][..]));
        assert_eq!(find("pods").verbs, ["list"]);
        let b = role_binding(&spec());
        assert_eq!(b.role_ref.name, "portside-backup-uploads");
        assert_eq!(b.subjects.unwrap()[0].name, "portside-backup-uploads");
    }

    #[test]
    fn claim() {
        let pvc = backup_claim(&spec());
        assert_eq!(pvc.metadata.name.as_deref(), Some("portside-backup-uploads"));
        let ps = pvc.spec.unwrap();
        assert_eq!(ps.storage_class_name.as_deref(), Some("local-path"));
        assert_eq!(ps.resources.unwrap().requests.unwrap()["storage"].0, "20Gi");
        assert_eq!(backup_name("portside-backup-uploads"), Some("uploads"));
        assert_eq!(backup_name("uploads"), None);
    }

    #[test]
    fn script_is_posix_sh_and_restores_on_exit() {
        // Things the busybox shell lacks.
        for bashism in ["pipefail", "[[", "$'", "<<<", "function "] {
            assert!(!SCRIPT.contains(bashism), "{bashism}");
        }
        assert!(SCRIPT.contains("trap start_again EXIT"));
        assert!(SCRIPT.contains(DISABLED_REPLICAS_ANNOTATION) && SCRIPT.contains(OFFLINE_ANNOTATION), "annotation names match the constants");
        assert!(SCRIPT.contains("mv -- \"$out.part\" \"$out\""), "renamed only once verified");
    }

    #[test]
    fn manifest_backup_skips_what_the_cluster_makes() {
        let obj = |name: &str, extra: serde_json::Value| {
            let mut v = json!({ "metadata": { "name": name, "namespace": "shop" } });
            if let serde_json::Value::Object(m) = extra {
                for (k, x) in m {
                    v["metadata"][k] = x;
                }
            }
            v
        };
        assert!(keep_in_manifest_backup("Deployment", &obj("web", json!({}))));
        assert!(!keep_in_manifest_backup("ConfigMap", &obj("kube-root-ca.crt", json!({}))));
        assert!(!keep_in_manifest_backup("ServiceAccount", &obj("default", json!({}))));
        assert!(!keep_in_manifest_backup("Service", &obj("x", json!({ "ownerReferences": [{ "controller": true }] }))), "its owner recreates it");
        assert!(keep_in_manifest_backup("Service", &obj("x", json!({ "ownerReferences": [{ "controller": false }] }))));
        assert!(!keep_in_manifest_backup("ClusterRole", &obj("system:node", json!({}))));
        assert!(!keep_in_manifest_backup("ClusterRole", &obj("admin", json!({ "labels": { "kubernetes.io/bootstrapping": "rbac-defaults" } }))));
        assert!(keep_in_manifest_backup("ClusterRole", &obj("my-reader", json!({}))));
        assert!(!keep_in_manifest_backup("Namespace", &obj("kube-system", json!({}))));
        assert!(keep_in_manifest_backup("Namespace", &obj("shop", json!({}))));
    }

    #[test]
    fn secret_lines_never_have_values() {
        let s = json!({ "type": "Opaque", "metadata": { "name": "db", "namespace": "shop" }, "data": { "password": "aHVudGVyMg==", "user": "YWRtaW4=" } });
        let line = secret_line(&s).unwrap();
        assert_eq!(line, "- `shop/db` (Opaque): password, user");
        assert!(!line.contains("aHVudGVy"));
        assert_eq!(secret_line(&json!({ "type": "helm.sh/release.v1", "metadata": {} })), None);
    }

    #[test]
    fn old_specs_get_the_default_image() {
        let mut v = serde_json::to_value(spec()).unwrap();
        v.as_object_mut().unwrap().remove("image");
        assert_eq!(serde_json::from_value::<BackupSpec>(v).unwrap().image, DEFAULT_IMAGE);
    }
}
