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
  /** PersistentVolumeClaims its pods mount. */
  claims: string[];
}

export interface VolumeClaimInfo {
  namespace: string;
  name: string;
  phase: string;
  storageClass: string | null;
  capacity: string | null;
  volumeName: string | null;
  createdMs: number | null;
  accessModes: string[];
  /** Running or pending pods that mount it. */
  mountedBy: string[];
  /** Workloads whose pods mount it, e.g. "Deployment/web". */
  usedBy: string[];
  /** Why files can't be written right now; empty = writable. */
  writeBlockers: string[];
}

export interface ServicePort {
  name: string | null;
  port: number;
  targetPort: string | null;
  nodePort: number | null;
  protocol: string;
}

export interface ServiceInfo {
  namespace: string;
  name: string;
  type: "ClusterIP" | "NodePort" | "LoadBalancer" | "ExternalName" | string;
  clusterIp: string | null;
  external: string[];
  ports: ServicePort[];
  selector: Record<string, string>;
  podsMatched: number;
  podsReady: number;
  podNames: string[];
  routes: string[];
  createdMs: number | null;
}

export interface ConfigInfo {
  kind: "ConfigMap" | "Secret";
  namespace: string;
  name: string;
  secretType: string | null;
  keys: string[];
  sizeBytes: number;
  immutable: boolean;
  usedBy: string[];
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
  category: "pod" | "node" | "workload" | "network" | "storage" | "event" | "logs";
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
  services: ServiceInfo[];
  /** ConfigMaps and Secrets: key names and sizes only. */
  configs: ConfigInfo[];
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
  /** Archive folder; null = "archives" in the app data folder. */
  archiveDir: string | null;
  /** Image for the pod that mounts a volume for the file browser. */
  filesHelperImage: string;
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
  /** Pods of this workload, including gone ones. Needs `namespace`. */
  workload?: WorkloadRef | null;
  levels?: LogLevel[];
  sinceMs?: number | null;
  untilMs?: number | null;
  beforeId?: number | null;
  limit?: number | null;
}

export interface WorkloadRef {
  kind: string;
  name: string;
}

/** A pod with stored log lines, alive or gone. */
export interface LogSource {
  namespace: string;
  pod: string;
  /** Recorded controller; null for pods seen before owners were recorded. */
  ownerKind: string | null;
  ownerName: string | null;
  lines: number;
  errors: number;
  warnings: number;
  firstMs: number;
  lastMs: number;
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

/** An object that belongs with a workload (export / copy). */
export interface RelatedRef extends ObjectRef {
  reason: string;
  /** Exists in the workload's namespace. */
  exists: boolean;
  /** Holds credentials; never pre-selected. */
  sensitive: boolean;
  defaultSelected: boolean;
}

export interface ConfigEntry {
  key: string;
  /** Text value; null for binary entries (read-only). */
  value: string | null;
  binary: boolean;
  size: number;
}

export interface ConfigData {
  kind: "ConfigMap" | "Secret";
  namespace: string;
  name: string;
  resourceVersion: string;
  secretType: string | null;
  immutable: boolean;
  entries: ConfigEntry[];
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

// --- port-forwarding ---------------------------------------------------------

export interface ForwardInfo {
  id: number;
  profileId: string;
  clusterName: string;
  namespace: string;
  service: string;
  servicePort: number;
  localPort: number;
  pod: string | null;
  targetPort: number | null;
  activeConnections: number;
  totalConnections: number;
  lastError: string | null;
  startedMs: number;
}

// --- archives ------------------------------------------------------------------

export interface ArchivedObject extends ObjectRef {
  /** Removed from the cluster when archived. */
  removed: boolean;
  error: string | null;
}

/** archive.json in an archive folder. */
export interface ArchiveMeta {
  /** Folder path under the archive root. */
  id: string;
  format: number;
  kind: string;
  namespace: string;
  name: string;
  clusterId: string;
  /** Profile it came from (Restore's default target). */
  profileId: string;
  connectionName: string;
  archivedMs: number;
  replicas: number | null;
  images: string[];
  objects: ArchivedObject[];
  logLines: number;
  restoredMs: number | null;
  restoredTo: string | null;
}

export interface ArchivePlanItem extends RelatedRef {
  /** Other workloads/Services that still use it; removing it would break them. */
  usedBy: string[];
}

export interface RemoveResult extends ObjectRef {
  outcome: "removed" | "already gone" | "error";
  message: string | null;
}

export interface ArchiveOutcome {
  archive: ArchiveMeta;
  results: RemoveResult[];
}

// --- volume files ----------------------------------------------------------------

/** A file browser session: one helper pod mounting one claim. */
export interface FileSession {
  id: number;
  profileId: string;
  namespace: string;
  claim: string;
  pod: string;
  writable: boolean;
  /** Why it's read-only, when `writable` is false. */
  blockers: string[];
  mountedBy: string[];
  startedMs: number;
  expiresMs: number;
}

export interface FileEntry {
  name: string;
  kind: "file" | "dir" | "link" | "other";
  size: number;
  modifiedMs: number | null;
  linkToDir: boolean;
}

export interface FileListing {
  path: string;
  entries: FileEntry[];
  totalBytes: number | null;
  freeBytes: number | null;
}

export interface FileProgress {
  transferId: string;
  label: string;
  done: number;
  total: number | null;
}

export interface UploadSummary {
  files: number;
  folders: number;
  bytes: number;
}

export interface LocalPathInfo {
  path: string;
  name: string;
  dir: boolean;
}
