//! Tauri commands: thin wrappers over the monitor, store and kube crates.
//! Store queries run on a blocking thread; every command returns
//! `Result<T, String>` so errors surface as rejected promises in the UI.

use std::sync::Arc;

use portside_core::logline::{detect_level, split_timestamp, strip_ansi};
use portside_core::{ClusterSnapshot, Connection, EventInfo, LogRecord, Settings};
use portside_monitor::Status;
use portside_store::{
    ErrorPattern, HistogramBucket, IssueHistoryEntry, LogQuery, LogSource, PodLogTotals, Sample, StorageStats, Store,
    WorkloadRef,
};
use serde::Serialize;
use tauri::State;

use crate::AppState;

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Run a store query off the async runtime, scoped to the active cluster.
async fn with_store<T: Send + 'static>(
    state: &State<'_, AppState>,
    f: impl FnOnce(&Store, &str) -> portside_store::Result<T> + Send + 'static,
) -> CmdResult<T> {
    let cluster = state
        .monitor
        .cluster_id()
        .ok_or_else(|| "No cluster connection configured.".to_string())?;
    let store = Arc::clone(state.monitor.store());
    tokio::task::spawn_blocking(move || f(&store, &cluster))
        .await
        .map_err(err)?
        .map_err(err)
}

// --- settings / connection -------------------------------------------------

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.monitor.settings()
}

#[tauri::command]
pub async fn save_settings(state: State<'_, AppState>, settings: Settings) -> CmdResult<Settings> {
    state.monitor.update_settings(settings).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResult {
    server_version: String,
    host_key_fingerprint: Option<String>,
}

#[tauri::command]
pub async fn test_connection(connection: Connection) -> CmdResult<TestResult> {
    let (server_version, host_key_fingerprint) = portside_kube::test_connection(&connection).await.map_err(err)?;
    Ok(TestResult { server_version, host_key_fingerprint })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubeContexts {
    contexts: Vec<String>,
    current: Option<String>,
}

#[tauri::command]
pub fn list_kube_contexts(kubeconfig_path: Option<String>) -> CmdResult<KubeContexts> {
    let (contexts, current) = portside_kube::list_contexts(kubeconfig_path.as_deref()).map_err(err)?;
    Ok(KubeContexts { contexts, current })
}

#[tauri::command]
pub fn get_status(state: State<'_, AppState>) -> Status {
    state.monitor.status()
}

#[tauri::command]
pub fn get_snapshot(state: State<'_, AppState>) -> Option<ClusterSnapshot> {
    state.monitor.snapshot().map(|s| (*s).clone())
}

#[tauri::command]
pub fn refresh_now(state: State<'_, AppState>) {
    state.monitor.refresh_now();
}

#[tauri::command]
pub fn sync_logs_now(state: State<'_, AppState>) {
    state.monitor.sync_logs_now();
}

// --- history / analytics ---------------------------------------------------

#[tauri::command]
pub async fn cluster_history(state: State<'_, AppState>, since_ms: i64, bucket_ms: i64) -> CmdResult<Vec<Sample>> {
    with_store(&state, move |s, c| s.cluster_history(c, since_ms, bucket_ms)).await
}

#[tauri::command]
pub async fn node_history(state: State<'_, AppState>, node: String, since_ms: i64, bucket_ms: i64) -> CmdResult<Vec<Sample>> {
    with_store(&state, move |s, c| s.node_history(c, &node, since_ms, bucket_ms)).await
}

#[tauri::command]
pub async fn pod_history(
    state: State<'_, AppState>,
    namespace: String,
    pod: String,
    since_ms: i64,
    bucket_ms: i64,
) -> CmdResult<Vec<Sample>> {
    with_store(&state, move |s, c| s.pod_history(c, &namespace, &pod, since_ms, bucket_ms)).await
}

#[tauri::command]
pub async fn query_logs(state: State<'_, AppState>, query: LogQuery) -> CmdResult<Vec<LogRecord>> {
    with_store(&state, move |s, c| s.query_logs(c, &query)).await
}

#[tauri::command]
pub async fn log_histogram(state: State<'_, AppState>, query: LogQuery, bucket_ms: i64) -> CmdResult<Vec<HistogramBucket>> {
    with_store(&state, move |s, c| s.log_histogram(c, &query, bucket_ms)).await
}

#[tauri::command]
pub async fn top_error_pods(state: State<'_, AppState>, since_ms: i64, limit: u32) -> CmdResult<Vec<PodLogTotals>> {
    with_store(&state, move |s, c| s.top_pods_by_errors(c, since_ms, limit)).await
}

#[tauri::command]
pub async fn pod_log_counts(state: State<'_, AppState>, since_ms: i64) -> CmdResult<Vec<PodLogTotals>> {
    with_store(&state, move |s, c| s.pod_log_counts(c, since_ms)).await
}

#[tauri::command]
pub async fn error_patterns(state: State<'_, AppState>, since_ms: i64, limit: usize) -> CmdResult<Vec<ErrorPattern>> {
    with_store(&state, move |s, c| s.top_error_patterns(c, since_ms, limit)).await
}

#[tauri::command]
pub async fn issue_history(state: State<'_, AppState>, since_ms: i64, limit: u32) -> CmdResult<Vec<IssueHistoryEntry>> {
    with_store(&state, move |s, c| s.issue_history(c, since_ms, limit)).await
}

#[tauri::command]
pub async fn storage_stats(state: State<'_, AppState>) -> CmdResult<StorageStats> {
    with_store(&state, |s, c| s.stats(c)).await
}

#[tauri::command]
pub async fn prune_now(state: State<'_, AppState>) -> CmdResult<usize> {
    let days = state.monitor.settings().retention_days.max(1) as i64;
    let cutoff = portside_core::now_ms() - days * 86_400_000;
    with_store(&state, move |s, _| s.prune(cutoff)).await
}

#[tauri::command]
pub async fn clear_cluster_data(state: State<'_, AppState>) -> CmdResult<()> {
    with_store(&state, |s, c| s.clear_cluster(c)).await
}

// --- live cluster reads ----------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveLogLine {
    ts_ms: i64,
    level: &'static str,
    message: String,
}

/// Tail a container's logs straight from the cluster (not the store).
#[tauri::command]
pub async fn live_logs(
    state: State<'_, AppState>,
    namespace: String,
    pod: String,
    container: String,
    previous: bool,
    tail_lines: Option<i64>,
) -> CmdResult<Vec<LiveLogLine>> {
    let cc = state.monitor.client().await?;
    let text = portside_kube::logs::fetch(
        &cc.client,
        &portside_kube::logs::LogRequest {
            namespace: &namespace,
            pod: &pod,
            container: &container,
            since_ns: None,
            since_seconds: None,
            tail_lines: Some(tail_lines.unwrap_or(1000)),
            previous,
            limit_bytes: Some(8 * 1024 * 1024),
        },
    )
    .await
    .map_err(err)?;
    Ok(text
        .lines()
        .filter_map(|raw| {
            let (ts, msg) = split_timestamp(raw)?;
            let msg = strip_ansi(msg).into_owned();
            Some(LiveLogLine {
                ts_ms: ts / 1_000_000,
                level: detect_level(&msg).as_str(),
                message: msg,
            })
        })
        .collect())
}

#[tauri::command]
pub async fn get_manifest(
    state: State<'_, AppState>,
    kind: String,
    namespace: Option<String>,
    name: String,
) -> CmdResult<String> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::get_yaml(&cc.client, &kind, namespace.as_deref(), &name)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn object_events(
    state: State<'_, AppState>,
    kind: String,
    namespace: Option<String>,
    name: String,
) -> CmdResult<Vec<EventInfo>> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::events_for(&cc.client, &kind, namespace.as_deref(), &name)
        .await
        .map_err(err)
}

// --- actions ---------------------------------------------------------------
// Each refreshes state afterwards so the UI reflects the change quickly.

#[tauri::command]
pub async fn delete_pod(state: State<'_, AppState>, namespace: String, name: String, force: bool) -> CmdResult<()> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::delete_pod(&cc.client, &namespace, &name, force).await.map_err(err)?;
    state.monitor.refresh_now();
    Ok(())
}

#[tauri::command]
pub async fn rollout_restart(state: State<'_, AppState>, kind: String, namespace: String, name: String) -> CmdResult<()> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::rollout_restart(&cc.client, &kind, &namespace, &name).await.map_err(err)?;
    state.monitor.refresh_now();
    Ok(())
}

#[tauri::command]
pub async fn scale_workload(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    replicas: i32,
) -> CmdResult<()> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::scale(&cc.client, &kind, &namespace, &name, replicas).await.map_err(err)?;
    state.monitor.refresh_now();
    Ok(())
}

#[tauri::command]
pub async fn set_cordon(state: State<'_, AppState>, node: String, cordoned: bool) -> CmdResult<()> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::set_cordon(&cc.client, &node, cordoned).await.map_err(err)?;
    state.monitor.refresh_now();
    Ok(())
}

// --- manifests: export / edit / copy ----------------------------------------

/// Clean YAML for one object, or every object of `kind` when `name` is null.
#[tauri::command]
pub async fn export_manifests(
    state: State<'_, AppState>,
    kind: String,
    namespace: Option<String>,
    name: Option<String>,
) -> CmdResult<String> {
    let cc = state.monitor.client().await?;
    portside_kube::manifests::export_yaml(&cc.client, &kind, namespace.as_deref(), name.as_deref())
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn edit_manifest(
    state: State<'_, AppState>,
    kind: String,
    namespace: Option<String>,
    name: String,
) -> CmdResult<String> {
    let cc = state.monitor.client().await?;
    portside_kube::manifests::edit_yaml(&cc.client, &kind, namespace.as_deref(), &name)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn apply_manifest(
    state: State<'_, AppState>,
    yaml: String,
    kind: String,
    namespace: Option<String>,
    name: String,
    dry_run: bool,
) -> CmdResult<String> {
    let cc = state.monitor.client().await?;
    let msg = portside_kube::manifests::apply_edit(&cc.client, &yaml, &kind, namespace.as_deref(), &name, dry_run)
        .await
        .map_err(err)?;
    if !dry_run {
        state.monitor.refresh_now();
    }
    Ok(msg)
}

/// Write text to a path the user picked in a save dialog.
#[tauri::command]
pub async fn save_text_file(path: String, contents: String) -> CmdResult<()> {
    tokio::fs::write(&path, contents).await.map_err(|e| format!("Couldn't write {path}: {e}"))
}

/// Objects that belong with a workload (for the export and copy dialogs).
#[tauri::command]
pub async fn related_objects(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
) -> CmdResult<Vec<portside_core::manifest::RelatedRef>> {
    let cc = state.monitor.client().await?;
    portside_kube::manifests::related(&cc.client, &kind, &namespace, &name).await.map_err(err)
}

/// A workload plus chosen related objects as one multi-document YAML.
#[tauri::command]
pub async fn export_bundle(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    extras: Vec<portside_core::manifest::ObjectRef>,
) -> CmdResult<String> {
    let cc = state.monitor.client().await?;
    portside_kube::manifests::export_bundle(&cc.client, &kind, &namespace, &name, &extras)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn get_config(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
) -> CmdResult<portside_kube::config::ConfigData> {
    let cc = state.monitor.client().await?;
    portside_kube::config::get_config(&cc.client, &kind, &namespace, &name).await.map_err(err)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn save_config(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    resource_version: String,
    text: std::collections::BTreeMap<String, String>,
    keep_binary: Vec<String>,
) -> CmdResult<portside_kube::config::ConfigData> {
    let cc = state.monitor.client().await?;
    let saved = portside_kube::config::save_config(&cc.client, &kind, &namespace, &name, &resource_version, text, &keep_binary)
        .await
        .map_err(err)?;
    state.monitor.refresh_now();
    Ok(saved)
}

/// Copy a workload (plus chosen related objects) from the active cluster
/// to any saved connection, into `target_namespace`.
#[tauri::command]
pub async fn copy_to_cluster(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    extras: Vec<portside_core::manifest::ObjectRef>,
    target_connection_id: String,
    target_namespace: String,
    dry_run: bool,
) -> CmdResult<Vec<portside_kube::manifests::CopyResult>> {
    let target_namespace = target_namespace.trim().to_string();
    if target_namespace.is_empty() {
        return Err("Pick a target namespace.".into());
    }
    let src = state.monitor.client().await?;
    let dst = state.monitor.connect_profile(&target_connection_id).await?;
    // Dependencies first so the workload's pods find them on start.
    let mut items = extras;
    items.push(portside_core::manifest::ObjectRef { kind, name });
    portside_kube::manifests::copy_objects(&src.client, &dst.client, &namespace, &items, &target_namespace, dry_run)
        .await
        .map_err(err)
}

/// Scale to 0 remembering the replica count (`disabled = true`), or restore
/// it. Returns the replica count now set.
#[tauri::command]
pub async fn set_workload_disabled(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    disabled: bool,
) -> CmdResult<i32> {
    let cc = state.monitor.client().await?;
    let replicas = portside_kube::actions::set_disabled(&cc.client, &kind, &namespace, &name, disabled)
        .await
        .map_err(err)?;
    state.monitor.refresh_now();
    Ok(replicas)
}

#[tauri::command]
pub async fn delete_workload(state: State<'_, AppState>, kind: String, namespace: String, name: String) -> CmdResult<()> {
    let cc = state.monitor.client().await?;
    portside_kube::actions::delete_workload(&cc.client, &kind, &namespace, &name).await.map_err(err)?;
    state.monitor.refresh_now();
    Ok(())
}

// --- service mode --------------------------------------------------------------

#[tauri::command]
pub fn test_notification(app: tauri::AppHandle) -> CmdResult<()> {
    use tauri_plugin_notification::NotificationExt;
    app.notification()
        .builder()
        .title("Portside Lite notifications work")
        .body("You'll be told here when a cluster has a new critical problem or becomes unreachable.")
        .show()
        .map_err(err)
}

/// Poll the active cluster and sweep every other connection right away.
#[tauri::command]
pub fn check_all_now(state: State<'_, AppState>) {
    state.monitor.check_all_now();
}

/// Pause or resume all polling (also toggled from the tray menu).
#[tauri::command]
pub async fn set_monitoring_paused(state: State<'_, AppState>, paused: bool) -> CmdResult<Settings> {
    state.monitor.set_paused(paused).await
}

// --- import ------------------------------------------------------------------

/// Largest file import will read; manifests are text, so this is generous.
const MAX_IMPORT_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Read files the user picked in the open dialog.
#[tauri::command]
pub async fn read_text_files(paths: Vec<String>) -> CmdResult<Vec<portside_core::manifest::SourceFile>> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let meta = tokio::fs::metadata(&path).await.map_err(|e| format!("{path}: {e}"))?;
        if meta.len() > MAX_IMPORT_FILE_BYTES {
            return Err(format!("{path} is larger than 5 MB; that's not a manifest file."));
        }
        let content = tokio::fs::read_to_string(&path).await.map_err(|e| format!("{path}: {e}"))?;
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(path);
        out.push(portside_core::manifest::SourceFile { name, content });
    }
    Ok(out)
}

/// Split sources into the objects they contain, for the import preview.
#[tauri::command]
pub fn parse_manifests(sources: Vec<portside_core::manifest::SourceFile>) -> Vec<portside_core::manifest::ManifestDoc> {
    portside_core::manifest::parse_sources(&sources).into_iter().map(|(d, _)| d).collect()
}

/// Apply the chosen documents (by `index` from `parse_manifests`) to any
/// saved connection.
#[tauri::command]
pub async fn import_manifests(
    state: State<'_, AppState>,
    sources: Vec<portside_core::manifest::SourceFile>,
    include: Vec<usize>,
    target_connection_id: String,
    namespace_override: Option<String>,
    dry_run: bool,
) -> CmdResult<Vec<portside_kube::manifests::ImportResult>> {
    let docs: Vec<_> = portside_core::manifest::parse_sources(&sources)
        .into_iter()
        .filter(|(d, _)| include.contains(&d.index))
        .filter_map(|(d, v)| v.map(|v| (d, v)))
        .collect();
    if docs.is_empty() {
        return Err("Nothing valid selected to import.".into());
    }
    let dst = state.monitor.connect_profile(&target_connection_id).await?;
    let results = portside_kube::manifests::import_docs(&dst.client, docs, namespace_override.as_deref(), dry_run).await;
    if !dry_run {
        state.monitor.refresh_now();
    }
    Ok(results)
}

// --- port-forwarding -----------------------------------------------------------

/// Forward a Service port in the active cluster to 127.0.0.1. `local_port`
/// null picks a free, memorable port.
#[tauri::command]
pub async fn start_port_forward(
    state: State<'_, AppState>,
    namespace: String,
    service: String,
    service_port: i32,
    local_port: Option<u16>,
) -> CmdResult<portside_monitor::forwards::ForwardInfo> {
    state.monitor.start_forward(&namespace, &service, service_port, local_port).await
}

#[tauri::command]
pub fn stop_port_forward(state: State<'_, AppState>, id: u64) {
    state.monitor.stop_forward(id);
}

#[tauri::command]
pub fn list_port_forwards(state: State<'_, AppState>) -> Vec<portside_monitor::forwards::ForwardInfo> {
    state.monitor.list_forwards()
}

// --- stored logs by pod / workload --------------------------------------------

/// Pods with stored log lines (alive or gone), optionally for one namespace
/// and/or workload, newest activity first.
#[tauri::command]
pub async fn log_sources(
    state: State<'_, AppState>,
    namespace: Option<String>,
    workload: Option<WorkloadRef>,
) -> CmdResult<Vec<LogSource>> {
    with_store(&state, move |s, c| s.log_sources(c, namespace.as_deref(), workload.as_ref())).await
}

// --- archives -------------------------------------------------------------------

use portside_core::archive::{archive_id, ArchiveMeta, ArchivePlanItem, ArchivedObject, ARCHIVE_FORMAT};
use portside_core::manifest::{apply_order, ObjectRef};
use portside_store::archive as archives;

/// Where archives live: the configured folder, else `archives` in app data.
fn archive_dir(state: &State<'_, AppState>) -> std::path::PathBuf {
    state
        .monitor
        .settings()
        .archive_dir
        .filter(|d| !d.trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| state.data_dir.join("archives"))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> portside_store::Result<T> + Send + 'static) -> CmdResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub fn archive_root(state: State<'_, AppState>) -> String {
    archive_dir(&state).to_string_lossy().into_owned()
}

/// What archiving a workload would save, and which of those objects other
/// workloads still use (those shouldn't be removed).
#[tauri::command]
pub async fn archive_plan(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
) -> CmdResult<Vec<ArchivePlanItem>> {
    let cc = state.monitor.client().await?;
    portside_kube::archive::plan(&cc.client, &kind, &namespace, &name).await.map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveOutcome {
    archive: ArchiveMeta,
    /// One row per object removed from the cluster.
    results: Vec<portside_kube::manifests::CopyResult>,
}

/// Archive a workload: save it plus `keep` (related objects) to its archive
/// folder, optionally with its stored logs, and only once that's on disk,
/// remove the workload and the chosen `remove` objects from the cluster.
/// `remove` may only name objects that were saved.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn archive_workload(
    state: State<'_, AppState>,
    kind: String,
    namespace: String,
    name: String,
    keep: Vec<ObjectRef>,
    remove: Vec<ObjectRef>,
    include_logs: bool,
) -> CmdResult<ArchiveOutcome> {
    // Archiving ends in a delete; only what the archive can bring back qualifies.
    if !portside_kube::archive::WORKLOAD_KINDS.contains(&kind.as_str()) {
        return Err(format!("Archiving a {kind} isn't supported."));
    }
    let settings = state.monitor.settings();
    let profile = settings.active_profile().ok_or("No cluster connection configured.")?.clone();
    let cluster_id = profile.connection.cluster_id();
    let cc = state.monitor.client().await?;

    // 1. Everything to save, as one re-appliable file; check it parses back whole.
    let manifest = portside_kube::manifests::export_bundle(&cc.client, &kind, &namespace, &name, &keep)
        .await
        .map_err(err)?;
    let parsed = portside_core::manifest::parse_sources(&[portside_core::manifest::SourceFile {
        name: archives::MANIFEST_FILE.into(),
        content: manifest.clone(),
    }]);
    if parsed.len() != keep.len() + 1 || parsed.iter().any(|(d, v)| d.error.is_some() || v.is_none()) {
        return Err("The exported manifest didn't read back cleanly, so nothing was archived or removed.".into());
    }

    // 2. Write the archive folder.
    let workload = ObjectRef { kind: kind.clone(), name: name.clone() };
    let mut saved: Vec<ObjectRef> = keep.clone();
    saved.push(workload.clone());
    saved.sort_by_key(|r| apply_order(&r.kind));
    let info = state
        .monitor
        .snapshot()
        .and_then(|s| s.workloads.iter().find(|w| w.kind == kind && w.namespace == namespace && w.name == name).cloned());
    let meta = ArchiveMeta {
        id: archive_id(&cluster_id, &namespace, &kind, &name),
        format: ARCHIVE_FORMAT,
        kind: kind.clone(),
        namespace: namespace.clone(),
        name: name.clone(),
        cluster_id: cluster_id.clone(),
        profile_id: profile.id.clone(),
        connection_name: profile.name.clone(),
        archived_ms: portside_core::now_ms(),
        replicas: info.as_ref().filter(|_| matches!(kind.as_str(), "Deployment" | "StatefulSet")).map(|w| w.disabled_replicas.unwrap_or(w.desired)),
        images: info.map(|w| w.images).unwrap_or_default(),
        objects: saved.iter().map(|r| ArchivedObject { kind: r.kind.clone(), name: r.name.clone(), removed: false, error: None }).collect(),
        log_lines: 0,
        restored_ms: None,
        restored_to: None,
    };
    let root = archive_dir(&state);
    let store = Arc::clone(state.monitor.store());
    let (c2, ns2, w2) = (cluster_id.clone(), namespace.clone(), WorkloadRef { kind: kind.clone(), name: name.clone() });
    let root2 = root.clone();
    let mut meta = blocking(move || {
        let write_logs = move |out: &mut dyn std::io::Write| store.write_workload_logs(&c2, &ns2, &w2, out);
        archives::create(&root2, meta, &manifest, include_logs.then_some(&write_logs as &dyn Fn(&mut dyn std::io::Write) -> _))
    })
    .await
    .map_err(|e| format!("Couldn't write the archive to {}: {e}. Nothing was removed.", root.display()))?;

    // 3. Bring it down: the workload always, plus chosen objects that were saved.
    let mut to_remove: Vec<ObjectRef> = remove.into_iter().filter(|r| keep.contains(r)).collect();
    to_remove.push(workload);
    let results = portside_kube::archive::remove_objects(&cc.client, &namespace, &to_remove).await;
    for r in &results {
        if let Some(o) = meta.objects.iter_mut().find(|o| o.kind == r.kind && o.name == r.name) {
            o.removed = r.outcome != "error";
            o.error = r.message.clone();
        }
    }
    let (root2, meta2) = (root.clone(), meta.clone());
    let noted = blocking(move || archives::save_meta(&root2, &meta2)).await;
    state.monitor.refresh_now();
    // The removal has happened either way, so the error has to say so.
    noted.map_err(|e| format!("The archive was saved and the objects were removed from the cluster, but its archive.json couldn't be updated with that: {e}"))?;
    Ok(ArchiveOutcome { archive: meta, results })
}

#[tauri::command]
pub async fn list_archives(state: State<'_, AppState>) -> CmdResult<Vec<ArchiveMeta>> {
    let root = archive_dir(&state);
    blocking(move || archives::list(&root)).await
}

#[tauri::command]
pub async fn archive_manifest(state: State<'_, AppState>, id: String) -> CmdResult<String> {
    let root = archive_dir(&state);
    blocking(move || archives::read_manifest(&root, &id)).await
}

/// Re-apply an archive's manifest to any saved connection (server-side
/// apply, dependencies first, missing namespaces created). A clean real run
/// marks the archive restored; the folder is kept.
#[tauri::command]
pub async fn restore_archive(
    state: State<'_, AppState>,
    id: String,
    target_connection_id: String,
    namespace_override: Option<String>,
    dry_run: bool,
) -> CmdResult<Vec<portside_kube::manifests::ImportResult>> {
    let root = archive_dir(&state);
    let (root2, id2) = (root.clone(), id.clone());
    let manifest = blocking(move || archives::read_manifest(&root2, &id2)).await?;
    let docs: Vec<_> = portside_core::manifest::parse_sources(&[portside_core::manifest::SourceFile {
        name: archives::MANIFEST_FILE.into(),
        content: manifest,
    }])
    .into_iter()
    .filter_map(|(d, v)| v.map(|v| (d, v)))
    .collect();
    if docs.is_empty() {
        return Err("The archive's manifest.yaml has no objects in it.".into());
    }
    let dst = state.monitor.connect_profile(&target_connection_id).await?;
    let results = portside_kube::manifests::import_docs(&dst.client, docs, namespace_override.as_deref(), dry_run).await;
    if !dry_run {
        if results.iter().all(|r| r.outcome != "error") {
            let target = state
                .monitor
                .settings()
                .connections
                .iter()
                .find(|p| p.id == target_connection_id)
                .map(|p| p.name.clone());
            blocking(move || {
                let mut meta = archives::read_meta(&root, &id)?;
                meta.restored_ms = Some(portside_core::now_ms());
                meta.restored_to = target;
                archives::save_meta(&root, &meta)
            })
            .await?;
        }
        state.monitor.refresh_now();
    }
    Ok(results)
}

#[tauri::command]
pub async fn delete_archive(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    let root = archive_dir(&state);
    blocking(move || archives::delete(&root, &id)).await
}

/// Open an archive's folder (or the archive root when `id` is null) in the
/// system file manager.
#[tauri::command]
pub fn open_archive_folder(state: State<'_, AppState>, id: Option<String>) -> CmdResult<()> {
    let root = archive_dir(&state);
    let path = match id {
        Some(id) => archives::dir(&root, &id).map_err(err)?,
        None => {
            std::fs::create_dir_all(&root).map_err(|e| format!("Couldn't create {}: {e}", root.display()))?;
            root
        }
    };
    tauri_plugin_opener::open_path(&path, None::<&str>).map_err(err)
}

// --- volume files ----------------------------------------------------------------

#[tauri::command]
pub async fn open_volume_files(
    state: State<'_, AppState>,
    namespace: String,
    claim: String,
) -> CmdResult<portside_monitor::files::FileSession> {
    state.monitor.open_files(&namespace, &claim).await
}

#[tauri::command]
pub async fn refresh_volume_files(state: State<'_, AppState>, id: u64) -> CmdResult<portside_monitor::files::FileSession> {
    state.monitor.refresh_files(id).await
}

/// Called once when the UI starts: after a window reload no view holds the
/// sessions the previous page opened.
#[tauri::command]
pub async fn release_volume_files(state: State<'_, AppState>) -> CmdResult<()> {
    state.monitor.release_all_files().await;
    Ok(())
}

#[tauri::command]
pub async fn close_volume_files(state: State<'_, AppState>, id: u64) -> CmdResult<()> {
    state.monitor.close_files(id).await;
    Ok(())
}

#[tauri::command]
pub async fn list_volume_files(state: State<'_, AppState>, id: u64, path: String) -> CmdResult<portside_core::files::FileListing> {
    state.monitor.list_files(id, &path).await
}

#[tauri::command]
pub async fn make_volume_dir(state: State<'_, AppState>, id: u64, dir: String, name: String) -> CmdResult<()> {
    state.monitor.make_dir(id, &dir, &name).await
}

#[tauri::command]
pub async fn delete_volume_path(state: State<'_, AppState>, id: u64, path: String) -> CmdResult<()> {
    state.monitor.delete_path(id, &path).await
}

#[tauri::command]
pub async fn rename_volume_path(state: State<'_, AppState>, id: u64, path: String, new_name: String) -> CmdResult<()> {
    state.monitor.rename_path(id, &path, &new_name).await
}

#[tauri::command]
pub async fn download_volume_path(
    state: State<'_, AppState>,
    id: u64,
    path: String,
    folder: bool,
    dest: String,
    transfer_id: String,
) -> CmdResult<u64> {
    state.monitor.download_path(id, &path, folder, &dest, &transfer_id).await
}

#[tauri::command]
pub async fn upload_volume_files(
    state: State<'_, AppState>,
    id: u64,
    dir: String,
    sources: Vec<String>,
    transfer_id: String,
) -> CmdResult<portside_monitor::files::UploadSummary> {
    state.monitor.upload_paths(id, &dir, sources, &transfer_id).await
}

#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, transfer_id: String) {
    state.monitor.cancel_transfer(&transfer_id);
}

/// Names and kinds of local paths (drag and drop gives bare paths), so the
/// UI can check for clashes before uploading.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPathInfo {
    path: String,
    name: String,
    dir: bool,
}

#[tauri::command]
pub async fn local_path_info(paths: Vec<String>) -> CmdResult<Vec<LocalPathInfo>> {
    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        let meta = tokio::fs::metadata(&path).await.map_err(|e| format!("{path}: {e}"))?;
        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.clone());
        out.push(LocalPathInfo { path, name, dir: meta.is_dir() });
    }
    Ok(out)
}
