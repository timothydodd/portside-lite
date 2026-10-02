import { invoke } from "@tauri-apps/api/core";
import type {
  ClusterSnapshot,
  ConfigData,
  Connection,
  CopyResult,
  ErrorPattern,
  FileListing,
  FileSession,
  ForwardInfo,
  EventInfo,
  HistogramBucket,
  ArchiveMeta,
  ArchiveOutcome,
  ArchivePlanItem,
  ImportResult,
  IssueHistoryEntry,
  LiveLogLine,
  LocalPathInfo,
  LogQuery,
  LogSource,
  LogRecord,
  ManifestDoc,
  ObjectRef,
  PodLogTotals,
  RelatedRef,
  Sample,
  Settings,
  SourceFile,
  Status,
  StorageStats,
  UploadSummary,
  WorkloadRef,
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
export const logSources = (namespace: string | null, workload: WorkloadRef | null) =>
  invoke<LogSource[]>("log_sources", { namespace, workload });
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
export const relatedObjects = (kind: string, namespace: string, name: string) =>
  invoke<RelatedRef[]>("related_objects", { kind, namespace, name });
export const exportBundle = (kind: string, namespace: string, name: string, extras: ObjectRef[]) =>
  invoke<string>("export_bundle", { kind, namespace, name, extras });
export const getConfig = (kind: string, namespace: string, name: string) =>
  invoke<ConfigData>("get_config", { kind, namespace, name });
export const saveConfig = (
  kind: string,
  namespace: string,
  name: string,
  resourceVersion: string,
  text: Record<string, string>,
  keepBinary: string[],
) => invoke<ConfigData>("save_config", { kind, namespace, name, resourceVersion, text, keepBinary });
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

// --- port-forwarding ---

export const startPortForward = (namespace: string, service: string, servicePort: number, localPort: number | null) =>
  invoke<ForwardInfo>("start_port_forward", { namespace, service, servicePort, localPort });
export const stopPortForward = (id: number) => invoke<void>("stop_port_forward", { id });
export const listPortForwards = () => invoke<ForwardInfo[]>("list_port_forwards");

// --- archives ----------------------------------------------------------------

export const archiveRoot = () => invoke<string>("archive_root");
export const archivePlan = (kind: string, namespace: string, name: string) =>
  invoke<ArchivePlanItem[]>("archive_plan", { kind, namespace, name });
export const archiveWorkload = (
  kind: string,
  namespace: string,
  name: string,
  keep: ObjectRef[],
  remove: ObjectRef[],
  includeLogs: boolean,
) => invoke<ArchiveOutcome>("archive_workload", { kind, namespace, name, keep, remove, includeLogs });
export const listArchives = () => invoke<ArchiveMeta[]>("list_archives");
export const archiveManifest = (id: string) => invoke<string>("archive_manifest", { id });
export const restoreArchive = (id: string, targetConnectionId: string, namespaceOverride: string | null, dryRun: boolean) =>
  invoke<ImportResult[]>("restore_archive", { id, targetConnectionId, namespaceOverride, dryRun });
export const deleteArchive = (id: string) => invoke<void>("delete_archive", { id });
export const openArchiveFolder = (id: string | null) => invoke<void>("open_archive_folder", { id });

// --- volume files ---------------------------------------------------------------

export const openVolumeFiles = (namespace: string, claim: string) =>
  invoke<FileSession>("open_volume_files", { namespace, claim });
export const refreshVolumeFiles = (id: number) => invoke<FileSession>("refresh_volume_files", { id });
export const closeVolumeFiles = (id: number) => invoke<void>("close_volume_files", { id });
export const listVolumeFiles = (id: number, path: string) => invoke<FileListing>("list_volume_files", { id, path });
export const makeVolumeDir = (id: number, dir: string, name: string) => invoke<void>("make_volume_dir", { id, dir, name });
export const deleteVolumePath = (id: number, path: string) => invoke<void>("delete_volume_path", { id, path });
export const renameVolumePath = (id: number, path: string, newName: string) =>
  invoke<void>("rename_volume_path", { id, path, newName });
export const downloadVolumePath = (id: number, path: string, folder: boolean, dest: string, transferId: string) =>
  invoke<number>("download_volume_path", { id, path, folder, dest, transferId });
export const uploadVolumeFiles = (id: number, dir: string, sources: string[], transferId: string) =>
  invoke<UploadSummary>("upload_volume_files", { id, dir, sources, transferId });
export const cancelTransfer = (transferId: string) => invoke<void>("cancel_transfer", { transferId });
export const localPathInfo = (paths: string[]) => invoke<LocalPathInfo[]>("local_path_info", { paths });
