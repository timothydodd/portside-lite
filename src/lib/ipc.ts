import { invoke } from "@tauri-apps/api/core";
import type {
  ClusterSnapshot,
  Connection,
  CopyResult,
  ErrorPattern,
  EventInfo,
  HistogramBucket,
  ImportResult,
  IssueHistoryEntry,
  LiveLogLine,
  LogQuery,
  LogRecord,
  ManifestDoc,
  ObjectRef,
  PodLogTotals,
  RefStatus,
  Sample,
  Settings,
  SourceFile,
  Status,
  StorageStats,
} from "./types";

// --- settings / connection ---

export const getSettings = () => invoke<Settings>("get_settings");
export const saveSettings = (settings: Settings) => invoke<Settings>("save_settings", { settings });
export const testConnection = (connection: Connection) =>
  invoke<{ serverVersion: string; hostKeyFingerprint: string | null }>("test_connection", { connection });
export const listKubeContexts = (kubeconfigPath: string | null) =>
  invoke<{ contexts: string[]; current: string | null }>("list_kube_contexts", { kubeconfigPath });
export const getStatus = () => invoke<Status>("get_status");
export const getSnapshot = () => invoke<ClusterSnapshot | null>("get_snapshot");
export const refreshNow = () => invoke<void>("refresh_now");
export const syncLogsNow = () => invoke<void>("sync_logs_now");

// --- history / analytics ---

export const clusterHistory = (sinceMs: number, bucketMs: number) =>
  invoke<Sample[]>("cluster_history", { sinceMs, bucketMs });
export const nodeHistory = (node: string, sinceMs: number, bucketMs: number) =>
  invoke<Sample[]>("node_history", { node, sinceMs, bucketMs });
export const podHistory = (namespace: string, pod: string, sinceMs: number, bucketMs: number) =>
  invoke<Sample[]>("pod_history", { namespace, pod, sinceMs, bucketMs });
export const queryLogs = (query: LogQuery) => invoke<LogRecord[]>("query_logs", { query });
export const logHistogram = (query: LogQuery, bucketMs: number) =>
  invoke<HistogramBucket[]>("log_histogram", { query, bucketMs });
export const topErrorPods = (sinceMs: number, limit: number) =>
  invoke<PodLogTotals[]>("top_error_pods", { sinceMs, limit });
export const podLogCounts = (sinceMs: number) => invoke<PodLogTotals[]>("pod_log_counts", { sinceMs });
export const errorPatterns = (sinceMs: number, limit: number) =>
  invoke<ErrorPattern[]>("error_patterns", { sinceMs, limit });
export const issueHistory = (sinceMs: number, limit: number) =>
  invoke<IssueHistoryEntry[]>("issue_history", { sinceMs, limit });
export const storageStats = () => invoke<StorageStats>("storage_stats");
export const pruneNow = () => invoke<number>("prune_now");
export const clearClusterData = () => invoke<void>("clear_cluster_data");

// --- live cluster reads ---

export const liveLogs = (namespace: string, pod: string, container: string, previous: boolean, tailLines?: number) =>
  invoke<LiveLogLine[]>("live_logs", { namespace, pod, container, previous, tailLines: tailLines ?? null });
export const getManifest = (kind: string, namespace: string | null, name: string) =>
  invoke<string>("get_manifest", { kind, namespace, name });
export const objectEvents = (kind: string, namespace: string | null, name: string) =>
  invoke<EventInfo[]>("object_events", { kind, namespace, name });

// --- actions ---

export const deletePod = (namespace: string, name: string, force = false) =>
  invoke<void>("delete_pod", { namespace, name, force });
export const rolloutRestart = (kind: string, namespace: string, name: string) =>
  invoke<void>("rollout_restart", { kind, namespace, name });
export const scaleWorkload = (kind: string, namespace: string, name: string, replicas: number) =>
  invoke<void>("scale_workload", { kind, namespace, name, replicas });
export const setCordon = (node: string, cordoned: boolean) => invoke<void>("set_cordon", { node, cordoned });

// --- manifests ---

export const exportManifests = (kind: string, namespace: string | null, name: string | null) =>
  invoke<string>("export_manifests", { kind, namespace, name });
export const editManifest = (kind: string, namespace: string | null, name: string) =>
  invoke<string>("edit_manifest", { kind, namespace, name });
export const applyManifest = (yaml: string, kind: string, namespace: string | null, name: string, dryRun: boolean) =>
  invoke<string>("apply_manifest", { yaml, kind, namespace, name, dryRun });
export const saveTextFile = (path: string, contents: string) => invoke<void>("save_text_file", { path, contents });
export const workloadReferences = (kind: string, namespace: string, name: string) =>
  invoke<RefStatus[]>("workload_references", { kind, namespace, name });
export const copyToCluster = (
  kind: string,
  namespace: string,
  name: string,
  extras: ObjectRef[],
  targetConnectionId: string,
  targetNamespace: string,
  dryRun: boolean,
) =>
  invoke<CopyResult[]>("copy_to_cluster", { kind, namespace, name, extras, targetConnectionId, targetNamespace, dryRun });
export const setWorkloadDisabled = (kind: string, namespace: string, name: string, disabled: boolean) =>
  invoke<number>("set_workload_disabled", { kind, namespace, name, disabled });
export const deleteWorkload = (kind: string, namespace: string, name: string) =>
  invoke<void>("delete_workload", { kind, namespace, name });

// --- service mode ---

export const testNotification = () => invoke<void>("test_notification");
export const checkAllNow = () => invoke<void>("check_all_now");
export const setMonitoringPaused = (paused: boolean) => invoke<Settings>("set_monitoring_paused", { paused });

// --- import ---

export const readTextFiles = (paths: string[]) => invoke<SourceFile[]>("read_text_files", { paths });
export const parseManifests = (sources: SourceFile[]) => invoke<ManifestDoc[]>("parse_manifests", { sources });
export const importManifests = (
  sources: SourceFile[],
  include: number[],
  targetConnectionId: string,
  namespaceOverride: string | null,
  dryRun: boolean,
) => invoke<ImportResult[]>("import_manifests", { sources, include, targetConnectionId, namespaceOverride, dryRun });
