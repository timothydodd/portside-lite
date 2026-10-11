//! DTOs sent to the UI. Everything serializes camelCase to match the
//! TypeScript types in `src/lib/types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSnapshot {
    pub collected_at_ms: i64,
    pub cluster_id: String,
    pub server_version: Option<String>,
    /// False when metrics.k8s.io (metrics-server) isn't answering.
    pub metrics_available: bool,
    pub totals: ClusterTotals,
    pub nodes: Vec<NodeInfo>,
    pub pods: Vec<PodInfo>,
    pub workloads: Vec<WorkloadInfo>,
    pub volumes: Vec<VolumeClaimInfo>,
    pub services: Vec<ServiceInfo>,
    pub ingresses: Vec<IngressInfo>,
    pub autoscalers: Vec<AutoscalerInfo>,
    pub persistent_volumes: Vec<PersistentVolumeInfo>,
    pub storage_classes: Vec<StorageClassInfo>,
    /// ConfigMaps and Secrets. Key names and sizes only, never values.
    pub configs: Vec<ConfigInfo>,
    pub service_accounts: Vec<ServiceAccountInfo>,
    /// Image registries: every host an image is pulled from, plus hosts a
    /// pull secret holds credentials for.
    pub registries: Vec<RegistryInfo>,
    /// Volume backups Portside set up (CronJobs labelled `portside-lite/backup`).
    pub backups: Vec<BackupInfo>,
    /// Recent Warning events, newest first.
    pub events: Vec<EventInfo>,
    pub issues: Vec<Issue>,
    pub namespaces: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ClusterTotals {
    pub nodes: usize,
    pub nodes_ready: usize,
    pub pods: usize,
    pub pods_running: usize,
    pub pods_pending: usize,
    pub pods_failed: usize,
    pub cpu_allocatable: f64,
    pub mem_allocatable: f64,
    pub cpu_usage: Option<f64>,
    pub mem_usage: Option<f64>,
    pub cpu_requests: f64,
    pub mem_requests: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct NodeInfo {
    pub name: String,
    pub ready: bool,
    /// Cordoned.
    pub unschedulable: bool,
    pub roles: Vec<String>,
    pub kubelet_version: String,
    pub os_image: String,
    pub kernel_version: String,
    pub internal_ip: Option<String>,
    pub cpu_capacity: f64,
    pub cpu_allocatable: f64,
    pub mem_capacity: f64,
    pub mem_allocatable: f64,
    pub cpu_usage: Option<f64>,
    pub mem_usage: Option<f64>,
    pub cpu_requests: f64,
    pub mem_requests: f64,
    pub pod_count: usize,
    pub pod_capacity: i64,
    pub conditions: Vec<ConditionInfo>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConditionInfo {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PodInfo {
    pub namespace: String,
    pub name: String,
    pub uid: String,
    pub node: Option<String>,
    pub phase: String,
    /// kubectl-style status column (CrashLoopBackOff, Completed, Terminating…).
    pub status: String,
    pub ready_containers: usize,
    pub total_containers: usize,
    pub restarts: i32,
    pub last_restart_ms: Option<i64>,
    pub created_ms: Option<i64>,
    pub deleting_since_ms: Option<i64>,
    pub owner_kind: Option<String>,
    /// For ReplicaSet-owned pods this is the Deployment name.
    pub owner_name: Option<String>,
    pub pod_ip: Option<String>,
    pub qos_class: Option<String>,
    pub cpu_usage: Option<f64>,
    pub mem_usage: Option<f64>,
    pub cpu_requests: f64,
    pub mem_requests: f64,
    pub mem_limits: f64,
    pub containers: Vec<ContainerInfo>,
    pub conditions: Vec<ConditionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub init: bool,
    pub ready: bool,
    pub restarts: i32,
    /// running | waiting | terminated | unknown
    pub state: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub started_ms: Option<i64>,
    pub last_terminated_reason: Option<String>,
    pub last_terminated_exit_code: Option<i32>,
    pub last_terminated_ms: Option<i64>,
}

/// Annotation holding the replica count to restore when a workload that was
/// disabled (scaled to 0) through Portside is enabled again.
pub const DISABLED_REPLICAS_ANNOTATION: &str = "portside-lite/disabled-replicas";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkloadInfo {
    /// Deployment | StatefulSet | DaemonSet | Job | CronJob
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub desired: i32,
    pub ready: i32,
    pub available: i32,
    pub updated: i32,
    /// Jobs: failed pod count.
    pub failed: i32,
    pub images: Vec<String>,
    /// Paused deployment or suspended (cron)job.
    pub paused: bool,
    /// Disabled through Portside: scaled to 0 with the prior replica count
    /// remembered in [`DISABLED_REPLICAS_ANNOTATION`]. `None` = not disabled.
    pub disabled_replicas: Option<i32>,
    /// Scaled to 0 by this backup (its name) for a copy. Still set when no
    /// run is going = a run left it offline.
    pub backup_offline: Option<String>,
    /// Failure/progress condition message worth surfacing, if any.
    pub condition_message: Option<String>,
    pub created_ms: Option<i64>,
    /// CronJobs only.
    pub schedule: Option<String>,
    pub last_schedule_ms: Option<i64>,
    pub last_successful_ms: Option<i64>,
    /// PersistentVolumeClaims its pods mount (StatefulSet template claims
    /// included), for the Files tab.
    pub claims: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VolumeClaimInfo {
    pub namespace: String,
    pub name: String,
    pub phase: String,
    pub storage_class: Option<String>,
    pub capacity: Option<String>,
    pub volume_name: Option<String>,
    pub created_ms: Option<i64>,
    pub access_modes: Vec<String>,
    /// Running or pending pods that mount it (file-browser helpers excluded).
    pub mounted_by: Vec<String>,
    /// Workloads whose pods mount it, e.g. "Deployment/web".
    pub used_by: Vec<String>,
    /// Why files can't be written right now; empty = writable.
    pub write_blockers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServicePort {
    pub name: Option<String>,
    pub port: i32,
    pub target_port: Option<String>,
    pub node_port: Option<i32>,
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub namespace: String,
    pub name: String,
    /// ClusterIP | NodePort | LoadBalancer | ExternalName
    #[serde(rename = "type")]
    pub type_: String,
    pub cluster_ip: Option<String>,
    /// Load-balancer IPs/hostnames, external IPs, or the ExternalName target.
    pub external: Vec<String>,
    pub ports: Vec<ServicePort>,
    pub selector: std::collections::BTreeMap<String, String>,
    /// Pods the selector matches (running, not terminating).
    pub pods_matched: usize,
    /// No pods on purpose: the selector matches a workload scaled to 0, or a
    /// CronJob between runs.
    pub idle: bool,
    /// Of those, pods that are Ready (receiving traffic).
    pub pods_ready: usize,
    pub pod_names: Vec<String>,
    /// "host/path (ingress-name)" for each Ingress rule routing here.
    pub routes: Vec<String>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressRoute {
    /// "*" when the rule has no host; "(default)" for the default backend.
    pub host: String,
    pub path: String,
    pub service: Option<String>,
    /// Port number or name.
    pub port: Option<String>,
    /// A non-Service backend (`resource:`), as "Kind/name".
    pub resource: Option<String>,
    /// The backend Service exists in the Ingress's namespace.
    pub service_found: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IngressInfo {
    pub namespace: String,
    pub name: String,
    pub class: Option<String>,
    pub routes: Vec<IngressRoute>,
    /// Hosts covered by a `tls:` entry.
    pub tls_hosts: Vec<String>,
    /// Load-balancer IPs/hostnames the controller published.
    pub address: Vec<String>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AutoscalerInfo {
    pub namespace: String,
    pub name: String,
    pub target_kind: String,
    pub target_name: String,
    pub min_replicas: i32,
    pub max_replicas: i32,
    pub current_replicas: i32,
    pub desired_replicas: i32,
    /// "cpu 45% / 80%", "memory 300Mi / 500Mi", …
    pub metrics: Vec<String>,
    /// Message of a False `AbleToScale` / `ScalingActive` condition.
    pub problem: Option<String>,
    /// `ScalingLimited` is True because it wants more than `max_replicas`.
    pub at_max: bool,
    pub last_scale_ms: Option<i64>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PersistentVolumeInfo {
    pub name: String,
    /// Available | Bound | Released | Failed | Pending
    pub phase: String,
    pub capacity: Option<String>,
    pub storage_class: Option<String>,
    /// Retain | Delete | Recycle
    pub reclaim_policy: Option<String>,
    pub access_modes: Vec<String>,
    /// "namespace/name" of the claim it's (or was) bound to.
    pub claim: Option<String>,
    /// Where the data lives: "hostPath /var/lib/…", "nfs host:/path", "csi driver.example", …
    pub source: Option<String>,
    /// Node it's pinned to (local volumes / local-path).
    pub node: Option<String>,
    pub message: Option<String>,
    /// When it entered its current phase (`lastPhaseTransitionTime`).
    pub phase_since_ms: Option<i64>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct StorageClassInfo {
    pub name: String,
    pub provisioner: String,
    pub reclaim_policy: Option<String>,
    pub binding_mode: Option<String>,
    pub allow_expansion: bool,
    pub is_default: bool,
    pub volumes: usize,
    pub claims: usize,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    /// ConfigMap | Secret
    pub kind: String,
    pub namespace: String,
    pub name: String,
    /// Secrets: Opaque, kubernetes.io/tls, helm.sh/release.v1, …
    pub secret_type: Option<String>,
    pub keys: Vec<String>,
    pub size_bytes: usize,
    pub immutable: bool,
    /// Workloads whose pod spec references it, as "Kind/name".
    pub used_by: Vec<String>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ServiceAccountInfo {
    pub namespace: String,
    pub name: String,
    /// `imagePullSecrets`: every pod running as this account gets them.
    pub image_pull_secrets: Vec<String>,
    /// Of those, the ones that don't exist in the namespace.
    pub missing_pull_secrets: Vec<String>,
    /// `automountServiceAccountToken`; `None` = the default (mounted).
    pub automount_token: Option<bool>,
    /// Roles bound to it: "Role/x (RoleBinding y)", "ClusterRole/x (ClusterRoleBinding y)".
    pub roles: Vec<String>,
    /// Workloads whose pods run as it, as "Kind/name".
    pub used_by: Vec<String>,
    pub created_ms: Option<i64>,
}

/// A credential for a registry: a `kubernetes.io/dockerconfigjson` (or
/// legacy `dockercfg`) Secret listing the host. Never the values.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegistryCredential {
    pub namespace: String,
    pub secret: String,
    /// Username stored with it, if any (not secret; helps tell credentials apart).
    pub username: Option<String>,
    /// ServiceAccounts that hand it to their pods.
    pub service_accounts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct RegistryInfo {
    /// "docker.io", "ghcr.io", "registry.local:5000", …
    pub host: String,
    /// Distinct images in use from it, without the host.
    pub images: Vec<String>,
    /// Workloads pulling from it, as "namespace/Kind/name".
    pub used_by: Vec<String>,
    pub credentials: Vec<RegistryCredential>,
    /// Workloads pulling from it with no pull secret for it (their own or
    /// their ServiceAccount's), as "namespace/Kind/name". Fine for public
    /// registries or node-level credentials (k3s `registries.yaml`); the
    /// usual cause of pull errors on private ones.
    pub without_credentials: Vec<String>,
    /// Containers currently stuck on ImagePullBackOff / ErrImagePull with an image from here.
    pub pull_errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub namespace: String,
    /// The backup's name; its CronJob and claim are `portside-backup-<name>`.
    pub name: String,
    pub cron_job: String,
    /// What it was set up with. `None` if the CronJob's annotation is missing
    /// or unreadable (edited by hand).
    pub spec: Option<crate::backup::BackupSpec>,
    pub schedule: String,
    pub time_zone: Option<String>,
    /// Paused: the CronJob is suspended.
    pub suspended: bool,
    /// A run is in progress.
    pub running: bool,
    pub last_schedule_ms: Option<i64>,
    pub last_successful_ms: Option<i64>,
    /// The newest finished run failed: why, and when it started.
    pub last_failure: Option<String>,
    pub last_failure_ms: Option<i64>,
    /// The backup claim's phase and size (`None` = it doesn't exist).
    pub claim_phase: Option<String>,
    pub claim_capacity: Option<String>,
    pub created_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EventInfo {
    pub namespace: String,
    pub object_kind: String,
    pub object_name: String,
    pub reason: String,
    pub message: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub count: i32,
    pub first_ms: Option<i64>,
    pub last_ms: Option<i64>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Critical,
    Warning,
    Info,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "critical",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// Operations the UI can offer against an issue's object.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ActionKind {
    ViewLogs,
    ViewPreviousLogs,
    DeletePod,
    RolloutRestart,
    Scale,
    Cordon,
    Uncordon,
    /// Start a volume backup now (the issue's object is its CronJob or one of its Jobs).
    RunBackup,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    /// Stable identity across polls: `<rule>:<kind>/<ns>/<name>[/<sub>]`.
    pub key: String,
    pub severity: Severity,
    /// pod | node | workload | storage | event | logs
    pub category: String,
    pub rule: String,
    pub kind: String,
    pub namespace: Option<String>,
    pub name: String,
    pub title: String,
    pub detail: String,
    /// Plain-language next step.
    pub hint: Option<String>,
    /// When the underlying condition started, if known.
    pub since_ms: Option<i64>,
    pub actions: Vec<ActionKind>,
    /// Filled from the issue history store: first time Portside saw it.
    #[serde(default)]
    pub first_seen_ms: Option<i64>,
}

/// One persisted log line.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRecord {
    pub id: i64,
    pub ts_ms: i64,
    pub namespace: String,
    pub pod: String,
    pub container: String,
    pub level: String,
    pub message: String,
}
