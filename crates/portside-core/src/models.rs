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
    /// ConfigMaps and Secrets. Key names and sizes only, never values.
    pub configs: Vec<ConfigInfo>,
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
    /// Failure/progress condition message worth surfacing, if any.
    pub condition_message: Option<String>,
    pub created_ms: Option<i64>,
    /// CronJobs only.
    pub schedule: Option<String>,
    pub last_schedule_ms: Option<i64>,
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
