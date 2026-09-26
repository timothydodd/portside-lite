// Mirrors the serde DTOs in crates/portside-core/src/{models,settings}.rs,
// crates/portside-store and crates/portside-monitor. Keep in sync.

export type Severity = "critical" | "warning" | "info";

export type ActionKind =
  | "viewLogs"
  | "viewPreviousLogs"
  | "deletePod"
  | "rolloutRestart"
  | "scale"
  | "cordon"
  | "uncordon";

export interface ConditionInfo {
  type: string;
  status: string;
  reason: string | null;
  message: string | null;
  lastTransitionMs: number | null;
}

export interface NodeInfo {
  name: string;
  ready: boolean;
  unschedulable: boolean;
  roles: string[];
  kubeletVersion: string;
  osImage: string;
  kernelVersion: string;
  internalIp: string | null;
  cpuCapacity: number;
  cpuAllocatable: number;
  memCapacity: number;
  memAllocatable: number;
  cpuUsage: number | null;
  memUsage: number | null;
  cpuRequests: number;
  memRequests: number;
  podCount: number;
  podCapacity: number;
  conditions: ConditionInfo[];
  createdMs: number | null;
}

export interface ContainerInfo {
  name: string;
  image: string;
  init: boolean;
  ready: boolean;
  restarts: number;
  state: "running" | "waiting" | "terminated" | "unknown";
  reason: string | null;
  message: string | null;
  startedMs: number | null;
  lastTerminatedReason: string | null;
  lastTerminatedExitCode: number | null;
  lastTerminatedMs: number | null;
}

export interface PodInfo {
  namespace: string;
  name: string;
  uid: string;
  node: string | null;
  phase: string;
  status: string;
  readyContainers: number;
  totalContainers: number;
  restarts: number;
  lastRestartMs: number | null;
  createdMs: number | null;
  deletingSinceMs: number | null;
  ownerKind: string | null;
  ownerName: string | null;
  podIp: string | null;
  qosClass: string | null;
  cpuUsage: number | null;
  memUsage: number | null;
  cpuRequests: number;
  memRequests: number;
  memLimits: number;
  containers: ContainerInfo[];
  conditions: ConditionInfo[];
}

export interface WorkloadInfo {
  kind: "Deployment" | "StatefulSet" | "DaemonSet" | "Job" | "CronJob";
  namespace: string;
  name: string;
  desired: number;
  ready: number;
  available: number;
  updated: number;
  failed: number;
  images: string[];
  paused: boolean;
  /** Scaled to 0 through Portside; the replica count Restore brings back. */
  disabledReplicas: number | null;
  conditionMessage: string | null;
  createdMs: number | null;
  schedule: string | null;
  lastScheduleMs: number | null;
}

export interface VolumeClaimInfo {
  namespace: string;
  name: string;
  phase: string;
  storageClass: string | null;
  capacity: string | null;
  volumeName: string | null;
  createdMs: number | null;
}

export interface EventInfo {
  namespace: string;
  objectKind: string;
  objectName: string;
  reason: string;
  message: string;
  type: string;
  count: number;
  firstMs: number | null;
  lastMs: number | null;
  source: string | null;
}

export interface Issue {
  key: string;
  severity: Severity;
  category: "pod" | "node" | "workload" | "storage" | "event" | "logs";
  rule: string;
  kind: string;
  namespace: string | null;
  name: string;
  title: string;
  detail: string;
  hint: string | null;
  sinceMs: number | null;
  actions: ActionKind[];
  firstSeenMs: number | null;
}

export interface ClusterTotals {
  nodes: number;
  nodesReady: number;
  pods: number;
  podsRunning: number;
  podsPending: number;
  podsFailed: number;
  cpuAllocatable: number;
  memAllocatable: number;
  cpuUsage: number | null;
  memUsage: number | null;
  cpuRequests: number;
  memRequests: number;
}

export interface ClusterSnapshot {
  collectedAtMs: number;
  clusterId: string;
  serverVersion: string | null;
  metricsAvailable: boolean;
  totals: ClusterTotals;
  nodes: NodeInfo[];
  pods: PodInfo[];
  workloads: WorkloadInfo[];
  volumes: VolumeClaimInfo[];
  events: EventInfo[];
  issues: Issue[];
  namespaces: string[];
}

export interface Status {
  state: "unconfigured" | "connecting" | "connected" | "error";
  message: string | null;
  clusterId: string | null;
  serverVersion: string | null;
  lastPollMs: number | null;
  lastPollDurationMs: number | null;
  lastLogSyncMs: number | null;
  lastLogSyncLines: number;
  logSyncErrors: number;
  paused: boolean;
}

// --- settings ----------------------------------------------------------------

export type SshAuth =
  | { kind: "key"; privateKeyPath: string; passphrase: string | null }
  | { kind: "password"; password: string };

export interface SshConnection {
  host: string;
  port: number;
  username: string;
  auth: SshAuth;
  kubeconfigCommand: string;
  apiHost: string;
  apiPort: number;
  hostKeyFingerprint: string | null;
  /** Sent to sudo on stdin; blank = passwordless sudo (or the SSH password). */
  sudoPassword: string | null;
}

export type Connection =
  | { mode: "local"; kubeconfigPath: string | null; context: string | null }
  | ({ mode: "ssh" } & SshConnection);

export interface ConnectionProfile {
  id: string;
  name: string;
  connection: Connection;
}

export interface Settings {
  connections: ConnectionProfile[];
  activeConnectionId: string | null;
  pollIntervalSecs: number;
  collectLogs: boolean;
  logIntervalSecs: number;
  initialLogLookbackHours: number;
  maxLogBytesPerPull: number;
  retentionDays: number;
  excludedNamespaces: string[];
  highUsagePercent: number;
  restartWarningThreshold: number;
  errorLogSpikePerHour: number;
  closeToTray: boolean;
  notifyCritical: boolean;
  notifyWarnings: boolean;
  backgroundCheckMinutes: number;
  monitoringPaused: boolean;
}

// --- store ---------------------------------------------------------------

export type LogLevel = "trace" | "debug" | "info" | "warning" | "error";

export interface LogRecord {
  id: number;
  tsMs: number;
  namespace: string;
  pod: string;
  container: string;
  level: LogLevel;
  message: string;
}

export interface LogQuery {
  search?: string | null;
  namespace?: string | null;
  pod?: string | null;
  container?: string | null;
  levels?: LogLevel[];
  sinceMs?: number | null;
  untilMs?: number | null;
  beforeId?: number | null;
  limit?: number | null;
}

export interface Sample {
  tsMs: number;
  cpu: number;
  mem: number;
}

export interface HistogramBucket {
  bucketMs: number;
  trace: number;
  debug: number;
  info: number;
  warning: number;
  error: number;
}

export interface PodLogTotals {
  namespace: string;
  pod: string;
  errors: number;
  warnings: number;
  total: number;
}

export interface ErrorPattern {
  pattern: string;
  sample: string;
  count: number;
  pods: string[];
  lastMs: number;
}

export interface IssueHistoryEntry {
  id: number;
  key: string;
  severity: Severity;
  category: string;
  kind: string;
  namespace: string | null;
  name: string;
  title: string;
  detail: string;
  firstSeenMs: number;
  lastSeenMs: number;
  resolvedMs: number | null;
}

export interface StorageStats {
  logLines: number;
  oldestLogMs: number | null;
  nodeSamples: number;
  podSamples: number;
  issuesTracked: number;
  dbBytes: number;
}

export interface LiveLogLine {
  tsMs: number;
  level: LogLevel;
  message: string;
}

// --- manifests ---------------------------------------------------------------

export interface ObjectRef {
  kind: string;
  name: string;
}

export interface RefStatus extends ObjectRef {
  /** Exists in the source namespace. */
  exists: boolean;
  /** ConfigMap/Secret can be copied along; others are warnings. */
  copyable: boolean;
}

export interface CopyResult extends ObjectRef {
  outcome: "created" | "updated" | "error";
  message: string | null;
}

/** A workload the editor / copy dialog acts on. */
export interface ManifestTarget {
  kind: string;
  namespace: string;
  name: string;
}

// --- import --------------------------------------------------------------------

export interface SourceFile {
  name: string;
  content: string;
}

export interface ManifestDoc {
  index: number;
  source: string;
  apiVersion: string | null;
  kind: string | null;
  name: string | null;
  namespace: string | null;
  error: string | null;
}

export interface ImportResult {
  index: number;
  source: string;
  kind: string;
  name: string;
  namespace: string | null;
  outcome: "created" | "updated" | "error";
  message: string | null;
}
