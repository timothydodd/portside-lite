//! The background engine: owns the cluster connection, polls state + usage,
//! runs issue detection, records samples, and pulls container logs into the
//! store. The Tauri layer spawns [`Monitor::run_poll_loop`] and
//! [`Monitor::run_log_loop`] and forwards [`EventSink`] callbacks to the UI.

pub mod forwards;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use futures::stream::{self, StreamExt};
use portside_core::logline::{detect_level, split_timestamp, strip_ansi};
use portside_core::{alerts, issues, now_ms, summarize, ClusterSnapshot, Connection, Issue, Settings, Severity};
use portside_kube::logs::LogRequest;
use portside_kube::ClusterClient;
use portside_store::{LogCursor, NewLog, Store};
use serde::Serialize;
use tokio::sync::{Mutex, Notify};

/// Parallel container log pulls per cycle.
const LOG_CONCURRENCY: usize = 6;
/// Longest single message kept; longer lines are truncated.
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// How often retention pruning runs.
const PRUNE_EVERY_MS: i64 = 60 * 60 * 1000;

/// Callbacks for pushing state to the UI.
pub trait EventSink: Send + Sync + 'static {
    fn snapshot(&self, snapshot: &ClusterSnapshot);
    fn status(&self, status: &Status);
    fn settings_changed(&self, settings: &Settings);
    fn logs_synced(&self, lines: usize);
    /// Show a desktop notification.
    fn notify(&self, title: &str, body: &str);
    /// Update the tray icon's tooltip with a one-glance status.
    fn tray_status(&self, tooltip: &str);
    /// The set of port-forwards (or their counters) changed.
    fn forwards_changed(&self, forwards: &[forwards::ForwardInfo]);
}

/// Failed polls of the active cluster before "can't reach" fires (≈45 s at
/// the default interval), so a blip doesn't notify.
const ACTIVE_OUTAGE_POLLS: u32 = 3;
/// Background checks are minutes apart, so one failure is already meaningful.
const SWEEP_OUTAGE_CHECKS: u32 = 1;
/// Upper bound for one background check of one cluster.
const SWEEP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Outage {
    fails: u32,
    notified: bool,
}

/// Per-profile line in the tray tooltip.
#[derive(Default, Clone, Copy)]
struct TrayLine {
    critical: usize,
    unreachable: bool,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// unconfigured | connecting | connected | error
    pub state: String,
    pub message: Option<String>,
    pub cluster_id: Option<String>,
    pub server_version: Option<String>,
    pub last_poll_ms: Option<i64>,
    pub last_poll_duration_ms: Option<i64>,
    pub last_log_sync_ms: Option<i64>,
    pub last_log_sync_lines: usize,
    pub log_sync_errors: usize,
    /// Monitoring paused by the user; data on screen is not refreshing.
    pub paused: bool,
}

pub struct Monitor {
    store: Arc<Store>,
    sink: Arc<dyn EventSink>,
    settings: RwLock<Settings>,
    status: RwLock<Status>,
    snapshot: RwLock<Option<Arc<ClusterSnapshot>>>,
    /// Current connection; `None` forces a reconnect on the next cycle.
    client: Mutex<Option<Arc<ClusterClient>>>,
    poll_wake: Notify,
    log_wake: Notify,
    sweep_wake: Notify,
    last_prune_ms: RwLock<i64>,
    /// Issue keys already notified, per cluster id.
    alerted: std::sync::Mutex<HashMap<String, HashSet<String>>>,
    /// Consecutive connection failures, per cluster id.
    outages: std::sync::Mutex<HashMap<String, Outage>>,
    /// Tray tooltip state, per profile id.
    tray: std::sync::Mutex<HashMap<String, TrayLine>>,
    /// Active Service port-forwards by id.
    forwards: std::sync::Mutex<HashMap<u64, forwards::ForwardEntry>>,
    next_forward: std::sync::atomic::AtomicU64,
}

fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> impl std::future::Future<Output = T> {
    async move { tokio::task::spawn_blocking(f).await.expect("blocking task panicked") }
}

impl Monitor {
    pub fn new(store: Arc<Store>, sink: Arc<dyn EventSink>) -> Arc<Self> {
        let settings = store.load_settings().unwrap_or_default();
        let status = Status {
            state: if settings.active_connection().is_some() { "connecting" } else { "unconfigured" }.into(),
            cluster_id: settings.active_connection().map(Connection::cluster_id),
            paused: settings.monitoring_paused,
            ..Default::default()
        };
        Arc::new(Self {
            store,
            sink,
            settings: RwLock::new(settings),
            status: RwLock::new(status),
            snapshot: RwLock::new(None),
            client: Mutex::new(None),
            poll_wake: Notify::new(),
            log_wake: Notify::new(),
            sweep_wake: Notify::new(),
            last_prune_ms: RwLock::new(0),
            alerted: Default::default(),
            outages: Default::default(),
            tray: Default::default(),
            forwards: Default::default(),
            next_forward: std::sync::atomic::AtomicU64::new(1),
        })
    }

    pub fn store(&self) -> &Arc<Store> {
        &self.store
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.status.read().unwrap().clone()
    }

    pub fn snapshot(&self) -> Option<Arc<ClusterSnapshot>> {
        self.snapshot.read().unwrap().clone()
    }

    pub fn cluster_id(&self) -> Option<String> {
        self.settings.read().unwrap().active_connection().map(Connection::cluster_id)
    }

    /// Persist new settings. A changed connection drops the current client and
    /// cached snapshot so the next cycle connects to the new cluster.
    pub async fn update_settings(&self, mut new: Settings) -> Result<Settings, String> {
        new.normalize();
        let store = Arc::clone(&self.store);
        let to_save = new.clone();
        blocking(move || store.save_settings(&to_save)).await.map_err(|e| e.to_string())?;
        let (connection_changed, resumed) = {
            let mut cur = self.settings.write().unwrap();
            // Switching profiles, or editing the active one, means a new cluster.
            let changed = cur.active_connection() != new.active_connection();
            let resumed = cur.monitoring_paused && !new.monitoring_paused;
            *cur = new.clone();
            (changed, resumed)
        };
        if connection_changed {
            *self.client.lock().await = None;
            *self.snapshot.write().unwrap() = None;
            self.set_status(|s| {
                *s = Status {
                    state: if new.active_connection().is_some() { "connecting" } else { "unconfigured" }.into(),
                    cluster_id: new.active_connection().map(Connection::cluster_id),
                    ..Default::default()
                }
            });
        }
        self.set_status(|s| s.paused = new.monitoring_paused);
        // Keep other listeners (tray menu, other views) in step with the save.
        self.sink.settings_changed(&new);
        self.refresh_tray();
        self.refresh_now();
        self.log_wake.notify_one();
        if resumed {
            self.sweep_wake.notify_one();
        }
        Ok(new)
    }

    /// Stop or restart all polling, log pulls and background checks.
    pub async fn set_paused(&self, paused: bool) -> Result<Settings, String> {
        let mut s = self.settings();
        s.monitoring_paused = paused;
        self.update_settings(s).await
    }

    /// Wake the poll loop immediately.
    pub fn refresh_now(&self) {
        self.poll_wake.notify_one();
    }

    pub fn sync_logs_now(&self) {
        self.log_wake.notify_one();
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        let snapshot = {
            let mut s = self.status.write().unwrap();
            f(&mut s);
            s.clone()
        };
        self.sink.status(&snapshot);
    }

    /// The live client, connecting if needed.
    pub async fn client(&self) -> Result<Arc<ClusterClient>, String> {
        let mut guard = self.client.lock().await;
        if let Some(c) = guard.as_ref() {
            return Ok(Arc::clone(c));
        }
        let conn = self
            .settings
            .read()
            .unwrap()
            .active_connection()
            .cloned()
            .ok_or_else(|| "No cluster connection configured — open Settings.".to_string())?;
        self.set_status(|s| {
            s.state = "connecting".into();
            s.message = None;
        });
        let cc = Arc::new(portside_kube::connect(&conn).await.map_err(|e| e.to_string())?);
        self.pin_host_key(&conn, cc.host_key_fingerprint.as_deref()).await;
        *guard = Some(Arc::clone(&cc));
        Ok(cc)
    }

    /// Connect to any saved profile (e.g. a copy target). Reuses the live
    /// client for the active profile; other profiles get a one-off connection
    /// the caller drops when done. SSH host keys are pinned the same way.
    pub async fn connect_profile(&self, profile_id: &str) -> Result<Arc<ClusterClient>, String> {
        let (conn, is_active) = {
            let s = self.settings.read().unwrap();
            let p = s
                .connections
                .iter()
                .find(|p| p.id == profile_id)
                .ok_or_else(|| "That connection no longer exists.".to_string())?;
            (p.connection.clone(), s.active_connection_id.as_deref() == Some(profile_id))
        };
        if is_active {
            return self.client().await;
        }
        let cc = Arc::new(portside_kube::connect(&conn).await.map_err(|e| e.to_string())?);
        self.pin_host_key(&conn, cc.host_key_fingerprint.as_deref()).await;
        Ok(cc)
    }

    /// Trust-on-first-use: remember the SSH host key after the first
    /// successful connect so later connects detect a changed key.
    async fn pin_host_key(&self, conn: &Connection, fingerprint: Option<&str>) {
        let (Connection::Ssh(ssh), Some(fp)) = (conn, fingerprint) else { return };
        if ssh.host_key_fingerprint.as_deref().is_some_and(|f| !f.is_empty()) {
            return;
        }
        let updated = {
            let mut s = self.settings.write().unwrap();
            // Pin onto the profile we actually connected with — the user may
            // have switched profiles while this connect was in flight.
            if let Some(p) = s.connections.iter_mut().find(|p| p.connection == *conn) {
                if let Connection::Ssh(cur) = &mut p.connection {
                    cur.host_key_fingerprint = Some(fp.to_string());
                }
            }
            s.clone()
        };
        let store = Arc::clone(&self.store);
        let to_save = updated.clone();
        let _ = blocking(move || store.save_settings(&to_save)).await;
        self.sink.settings_changed(&updated);
    }

    fn is_current(&self, cluster: &str) -> bool {
        self.cluster_id().as_deref() == Some(cluster)
    }

    async fn drop_client(&self) {
        *self.client.lock().await = None;
    }

    // --- poll loop --------------------------------------------------------

    pub async fn run_poll_loop(self: Arc<Self>) {
        loop {
            let interval = {
                let s = self.settings.read().unwrap();
                if s.active_connection().is_some() { s.poll_interval_secs.max(5) } else { 3600 }
            };
            let run = {
                let s = self.settings.read().unwrap();
                s.active_connection().is_some() && !s.monitoring_paused
            };
            if run {
                self.poll_once().await;
                self.maybe_prune().await;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
                _ = self.poll_wake.notified() => {}
            }
        }
    }

    async fn poll_once(&self) {
        let started = now_ms();
        // Label everything with the cluster that was active when the poll
        // began; if the user switches mid-poll, the result is discarded below.
        let Some(cluster) = self.cluster_id() else { return };
        let (profile_id, profile_name) = {
            let s = self.settings.read().unwrap();
            let p = s.active_profile();
            (p.map(|p| p.id.clone()).unwrap_or_default(), p.map(|p| p.name.clone()).unwrap_or_default())
        };
        let cc = match self.client().await {
            Ok(c) => c,
            Err(e) => {
                if self.is_current(&cluster) {
                    self.record_outage(&profile_id, &profile_name, &cluster, Some(&e), ACTIVE_OUTAGE_POLLS);
                    self.set_status(|s| {
                        s.state = "error".into();
                        s.message = Some(e);
                    });
                }
                return;
            }
        };

        let objects = match portside_kube::collect::fetch_objects(&cc.client).await {
            Ok(o) => o,
            Err(e) => {
                if self.is_current(&cluster) {
                    // Treat any failure as a dead connection; the next cycle reconnects.
                    self.drop_client().await;
                    self.record_outage(&profile_id, &profile_name, &cluster, Some(&e.to_string()), ACTIVE_OUTAGE_POLLS);
                    self.set_status(|s| {
                        s.state = "error".into();
                        s.message = Some(e.to_string());
                    });
                }
                return;
            }
        };
        let usage = portside_kube::collect::fetch_usage(&cc.client).await;
        let settings = self.settings();
        if !self.is_current(&cluster) {
            return; // switched clusters while polling — this data belongs to the old one
        }
        let now = now_ms();

        let mut snap = summarize::build_snapshot(&cluster, &objects, &usage, now);
        let mut found = issues::detect(&snap, &settings, now);

        let store = Arc::clone(&self.store);
        let cluster2 = cluster.clone();
        let spikes = blocking(move || store.pod_log_counts(&cluster2, now - 3_600_000)).await;
        if let Ok(counts) = spikes {
            let alive: std::collections::HashSet<(&str, &str)> =
                snap.pods.iter().map(|p| (p.namespace.as_str(), p.name.as_str())).collect();
            let counts: Vec<(String, String, i64)> = counts
                .into_iter()
                .filter(|c| alive.contains(&(c.namespace.as_str(), c.pod.as_str())))
                .map(|c| (c.namespace, c.pod, c.errors))
                .collect();
            found.extend(issues::log_spike_issues(&counts, &settings));
            issues::sort_dedupe(&mut found);
        }

        // Record samples + reconcile issue history.
        let node_samples: Vec<(String, f64, f64)> = snap
            .nodes
            .iter()
            .filter_map(|n| Some((n.name.clone(), n.cpu_usage?, n.mem_usage?)))
            .collect();
        let pod_samples: Vec<(String, String, f64, f64)> = snap
            .pods
            .iter()
            .filter_map(|p| Some((p.namespace.clone(), p.name.clone(), p.cpu_usage?, p.mem_usage?)))
            .collect();
        let store = Arc::clone(&self.store);
        let cluster2 = cluster.clone();
        // Remember each pod's workload so its logs stay findable by workload
        // once the pod (or the workload) is gone.
        let pod_owners: Vec<(String, String, String, String)> = snap
            .pods
            .iter()
            .filter_map(|p| Some((p.namespace.clone(), p.name.clone(), p.owner_kind.clone()?, p.owner_name.clone()?)))
            .collect();
        let synced = blocking(move || {
            let _ = store.record_samples(&cluster2, now, &node_samples, &pod_samples);
            let _ = store.record_pod_owners(&cluster2, now, &pod_owners);
            let mut found = found;
            let _ = store.sync_issues(&cluster2, now, &mut found);
            found
        })
        .await;
        snap.issues = synced;
        self.record_outage(&profile_id, &profile_name, &cluster, None, ACTIVE_OUTAGE_POLLS);
        self.process_alerts(&profile_id, &profile_name, &cluster, &snap.issues, &settings);

        let snap = Arc::new(snap);
        *self.snapshot.write().unwrap() = Some(Arc::clone(&snap));
        self.sink.snapshot(&snap);
        self.set_status(|s| {
            s.state = "connected".into();
            s.message = (!snap.metrics_available)
                .then(|| "metrics-server isn't responding — CPU/memory usage unavailable.".into());
            s.cluster_id = Some(cluster);
            s.server_version = snap.server_version.clone();
            s.last_poll_ms = Some(now);
            s.last_poll_duration_ms = Some(now_ms() - started);
        });
    }

    // --- alerts & tray -----------------------------------------------------

    /// Notify about newly qualifying problems on a cluster and refresh the tray.
    fn process_alerts(&self, profile_id: &str, profile_name: &str, cluster: &str, found: &[Issue], settings: &Settings) {
        let critical = found.iter().filter(|i| i.severity == Severity::Critical).count();
        self.tray.lock().unwrap().entry(profile_id.to_string()).or_default().critical = critical;
        self.refresh_tray();

        let message = {
            let mut map = self.alerted.lock().unwrap();
            let seen = map.entry(cluster.to_string()).or_default();
            let (fresh, remember) = alerts::diff_alerts(seen, found, settings.notify_warnings);
            let msg = alerts::alert_message(profile_name, &fresh);
            *seen = remember;
            msg
        };
        if let (true, Some((title, body))) = (settings.notify_critical, message) {
            self.sink.notify(&title, &body);
        }
    }

    /// Track consecutive failures; notify once when a cluster becomes
    /// unreachable and once when it recovers.
    fn record_outage(&self, profile_id: &str, profile_name: &str, cluster: &str, error: Option<&str>, threshold: u32) {
        let message = {
            let mut map = self.outages.lock().unwrap();
            let o = map.entry(cluster.to_string()).or_default();
            match error {
                Some(e) => {
                    o.fails += 1;
                    (o.fails >= threshold && !o.notified).then(|| {
                        o.notified = true;
                        let short: String = e.chars().take(180).collect();
                        (format!("Can't reach {profile_name}"), short)
                    })
                }
                None => {
                    let was_down = o.notified;
                    *o = Outage::default();
                    was_down.then(|| (format!("{profile_name} is reachable again"), "Monitoring resumed.".to_string()))
                }
            }
        };
        let down = self.outages.lock().unwrap().get(cluster).is_some_and(|o| o.notified);
        self.tray.lock().unwrap().entry(profile_id.to_string()).or_default().unreachable = down;
        self.refresh_tray();
        if let (true, Some((title, body))) = (self.settings.read().unwrap().notify_critical, message) {
            self.sink.notify(&title, &body);
        }
    }

    fn refresh_tray(&self) {
        let settings = self.settings();
        let tray = self.tray.lock().unwrap();
        let mut lines = Vec::new();
        for p in &settings.connections {
            let Some(t) = tray.get(&p.id) else { continue };
            if t.unreachable {
                lines.push(format!("{}: unreachable", p.name));
            } else if t.critical > 0 {
                lines.push(format!("{}: {} critical", p.name, t.critical));
            }
        }
        let mut tooltip = if settings.monitoring_paused {
            "Portside Lite — monitoring paused".to_string()
        } else if lines.is_empty() {
            "Portside Lite — all clear".to_string()
        } else {
            format!("Portside Lite\n{}", lines.join("\n"))
        };
        // Windows caps tray tooltips at 127 characters.
        if tooltip.chars().count() > 120 {
            tooltip = tooltip.chars().take(118).collect::<String>() + "…";
        }
        self.sink.tray_status(&tooltip);
    }

    /// Poll the active cluster and run a background sweep right away.
    pub fn check_all_now(&self) {
        self.refresh_now();
        self.sweep_wake.notify_one();
    }

    // --- background sweep of the other connections -------------------------

    pub async fn run_sweep_loop(self: Arc<Self>) {
        // Let startup (and the first active poll) settle first.
        tokio::time::sleep(Duration::from_secs(30)).await;
        loop {
            let (enabled, minutes) = {
                let s = self.settings.read().unwrap();
                (s.notify_critical && !s.monitoring_paused, s.background_check_minutes.max(5) as u64)
            };
            if enabled {
                self.sweep_once().await;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(minutes * 60)) => {}
                _ = self.sweep_wake.notified() => {}
            }
        }
    }

    async fn sweep_once(&self) {
        let settings = self.settings();
        let active = settings.active_connection_id.clone();
        for p in settings.connections.iter().filter(|p| Some(&p.id) != active.as_ref()) {
            let cluster = p.connection.cluster_id();
            let check = async {
                let cc = self.connect_profile(&p.id).await?;
                let objects = portside_kube::collect::fetch_objects(&cc.client).await.map_err(|e| e.to_string())?;
                let usage = portside_kube::collect::fetch_usage(&cc.client).await;
                Ok::<_, String>((objects, usage))
            };
            let result = match tokio::time::timeout(SWEEP_TIMEOUT, check).await {
                Ok(r) => r,
                Err(_) => Err(format!("no response within {} s", SWEEP_TIMEOUT.as_secs())),
            };
            match result {
                Ok((objects, usage)) => {
                    let now = now_ms();
                    let snap = summarize::build_snapshot(&cluster, &objects, &usage, now);
                    let found = issues::detect(&snap, &settings, now);
                    // Keep problem history for background clusters too.
                    let store = Arc::clone(&self.store);
                    let c = cluster.clone();
                    let found = blocking(move || {
                        let mut f = found;
                        let _ = store.sync_issues(&c, now, &mut f);
                        f
                    })
                    .await;
                    self.record_outage(&p.id, &p.name, &cluster, None, SWEEP_OUTAGE_CHECKS);
                    self.process_alerts(&p.id, &p.name, &cluster, &found, &settings);
                }
                Err(e) => self.record_outage(&p.id, &p.name, &cluster, Some(&e), SWEEP_OUTAGE_CHECKS),
            }
        }
    }

    async fn maybe_prune(&self) {
        let now = now_ms();
        if now - *self.last_prune_ms.read().unwrap() < PRUNE_EVERY_MS {
            return;
        }
        *self.last_prune_ms.write().unwrap() = now;
        let days = self.settings.read().unwrap().retention_days.max(1) as i64;
        let store = Arc::clone(&self.store);
        let _ = blocking(move || store.prune(now - days * 86_400_000)).await;
    }

    // --- log loop ---------------------------------------------------------

    pub async fn run_log_loop(self: Arc<Self>) {
        // Let the first state poll land before competing with it.
        tokio::time::sleep(Duration::from_secs(5)).await;
        loop {
            let (enabled, interval) = {
                let s = self.settings.read().unwrap();
                (s.collect_logs && s.active_connection().is_some() && !s.monitoring_paused, s.log_interval_secs.max(10))
            };
            if enabled {
                self.sync_logs_once().await;
            }
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(interval)) => {}
                _ = self.log_wake.notified() => {}
            }
        }
    }

    async fn sync_logs_once(&self) {
        let Some(snap) = self.snapshot() else { return };
        let Ok(cc) = self.client().await else { return };
        let settings = self.settings();
        let cluster = snap.cluster_id.clone();

        let mut targets = Vec::new();
        for p in &snap.pods {
            if settings.excluded_namespaces.iter().any(|n| n == &p.namespace) {
                continue;
            }
            if p.phase != "Running" && p.phase != "Failed" && p.phase != "Succeeded" {
                continue;
            }
            for c in p.containers.iter().filter(|c| !c.init) {
                targets.push((p.namespace.clone(), p.name.clone(), p.uid.clone(), c.name.clone(), c.restarts));
            }
        }

        let results: Vec<Result<usize, String>> = stream::iter(targets)
            .map(|(ns, pod, uid, container, restarts)| {
                let cc = Arc::clone(&cc);
                let store = Arc::clone(&self.store);
                let cluster = cluster.clone();
                let settings = &settings;
                async move {
                    sync_container(&cc, &store, &cluster, settings, &ns, &pod, &uid, &container, restarts).await
                }
            })
            .buffer_unordered(LOG_CONCURRENCY)
            .collect()
            .await;

        let lines: usize = results.iter().filter_map(|r| r.as_ref().ok()).sum();
        let errors = results.iter().filter(|r| r.is_err()).count();
        self.set_status(|s| {
            s.last_log_sync_ms = Some(now_ms());
            s.last_log_sync_lines = lines;
            s.log_sync_errors = errors;
        });
        self.sink.logs_synced(lines);
    }
}

/// Pull new lines for one container since its cursor. When the container has
/// restarted since the last pull, the crashed instance's tail is pulled first
/// — that's usually where the interesting error is.
#[allow(clippy::too_many_arguments)]
async fn sync_container(
    cc: &ClusterClient,
    store: &Arc<Store>,
    cluster: &str,
    settings: &Settings,
    ns: &str,
    pod: &str,
    uid: &str,
    container: &str,
    restarts: i32,
) -> Result<usize, String> {
    let s2 = Arc::clone(store);
    let (c2, u2, k2) = (cluster.to_string(), uid.to_string(), container.to_string());
    let cursor = blocking(move || s2.get_cursor(&c2, &u2, &k2)).await.map_err(|e| e.to_string())?;

    let lookback = settings.initial_log_lookback_hours as i64 * 3600;
    let mut cursor_ts = cursor.map(|c| c.last_ts_ns);
    let mut total = 0;

    if let Some(prev) = cursor {
        if restarts > prev.restart_count {
            let text = portside_kube::logs::fetch(
                &cc.client,
                &LogRequest {
                    namespace: ns,
                    pod,
                    container,
                    since_ns: None,
                    since_seconds: None,
                    tail_lines: Some(500),
                    previous: true,
                    limit_bytes: Some(settings.max_log_bytes_per_pull),
                },
            )
            .await
            .unwrap_or_default();
            let (lines, last) = parse_lines(&text, cursor_ts, ns, pod, container);
            total += lines.len();
            if let Some(last) = last {
                cursor_ts = Some(cursor_ts.map_or(last, |c| c.max(last)));
            }
            persist(store, cluster, uid, container, cursor_ts, restarts, lines).await?;
        }
    }

    let text = portside_kube::logs::fetch(
        &cc.client,
        &LogRequest {
            namespace: ns,
            pod,
            container,
            since_ns: cursor_ts,
            since_seconds: if cursor_ts.is_none() { Some(lookback) } else { None },
            tail_lines: None,
            previous: false,
            limit_bytes: Some(settings.max_log_bytes_per_pull),
        },
    )
    .await
    .map_err(|e| e.to_string())?;
    let (lines, last) = parse_lines(&text, cursor_ts, ns, pod, container);
    total += lines.len();
    let new_cursor = match (cursor_ts, last) {
        (Some(c), Some(l)) => Some(c.max(l)),
        (c, l) => l.or(c),
    };
    // Always write the cursor on first sight, even for a silent container, so
    // the next pull uses `since_time` rather than re-reading the lookback.
    let new_cursor = new_cursor.or(Some((now_ms() - 1000) * 1_000_000));
    persist(store, cluster, uid, container, new_cursor, restarts, lines).await?;
    Ok(total)
}

async fn persist(
    store: &Arc<Store>,
    cluster: &str,
    uid: &str,
    container: &str,
    cursor_ts: Option<i64>,
    restarts: i32,
    lines: Vec<NewLog>,
) -> Result<(), String> {
    let Some(ts) = cursor_ts else { return Ok(()) };
    let store = Arc::clone(store);
    let (c, u, k) = (cluster.to_string(), uid.to_string(), container.to_string());
    blocking(move || {
        store.append_logs(&c, &u, &k, LogCursor { last_ts_ns: ts, restart_count: restarts }, &lines, now_ms())
    })
    .await
    .map_err(|e| e.to_string())
}

/// Parse timestamped log text, skipping anything at or before `after_ns`
/// (the API's `sinceTime` is inclusive and second-granular).
pub fn parse_lines(text: &str, after_ns: Option<i64>, ns: &str, pod: &str, container: &str) -> (Vec<NewLog>, Option<i64>) {
    let mut out = Vec::new();
    let mut last = None;
    for raw in text.lines() {
        let Some((ts, msg)) = split_timestamp(raw) else { continue };
        if after_ns.is_some_and(|a| ts <= a) {
            continue;
        }
        last = Some(ts);
        let msg = strip_ansi(msg);
        let msg = msg.trim_end();
        if msg.is_empty() {
            continue;
        }
        let mut message = msg.to_string();
        if message.len() > MAX_MESSAGE_BYTES {
            let mut cut = MAX_MESSAGE_BYTES;
            while !message.is_char_boundary(cut) {
                cut -= 1;
            }
            message.truncate(cut);
            message.push('…');
        }
        out.push(NewLog {
            ts_ns: ts,
            namespace: ns.into(),
            pod: pod.into(),
            container: container.into(),
            level: detect_level(&message),
            message,
        });
    }
    (out, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSink {
        notes: std::sync::Mutex<Vec<String>>,
        tooltip: std::sync::Mutex<String>,
    }
    impl EventSink for FakeSink {
        fn snapshot(&self, _: &ClusterSnapshot) {}
        fn status(&self, _: &Status) {}
        fn settings_changed(&self, _: &Settings) {}
        fn logs_synced(&self, _: usize) {}
        fn notify(&self, title: &str, _body: &str) {
            self.notes.lock().unwrap().push(title.to_string());
        }
        fn tray_status(&self, tooltip: &str) {
            *self.tooltip.lock().unwrap() = tooltip.to_string();
        }
        fn forwards_changed(&self, _: &[forwards::ForwardInfo]) {}
    }

    fn monitor() -> (Arc<Monitor>, Arc<FakeSink>) {
        let store = Arc::new(Store::open_in_memory().unwrap());
        let mut settings = Settings::default();
        settings.connections.push(portside_core::ConnectionProfile {
            id: "p1".into(),
            name: "Homelab".into(),
            connection: Connection::Local { kubeconfig_path: None, context: Some("home".into()) },
        });
        settings.active_connection_id = Some("p1".into());
        store.save_settings(&settings).unwrap();
        let sink = Arc::new(FakeSink::default());
        (Monitor::new(store, sink.clone()), sink)
    }

    fn crit(key: &str) -> Issue {
        Issue {
            key: key.into(),
            severity: Severity::Critical,
            category: "pod".into(),
            rule: "r".into(),
            kind: "Pod".into(),
            namespace: None,
            name: key.into(),
            title: format!("down: {key}"),
            detail: String::new(),
            hint: None,
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        }
    }

    #[test]
    fn outage_notifies_after_threshold_and_on_recovery() {
        let (m, sink) = monitor();
        for _ in 0..2 {
            m.record_outage("p1", "Homelab", "c", Some("refused"), 3);
        }
        assert!(sink.notes.lock().unwrap().is_empty(), "blips below the threshold stay quiet");
        m.record_outage("p1", "Homelab", "c", Some("refused"), 3);
        m.record_outage("p1", "Homelab", "c", Some("refused"), 3);
        assert_eq!(*sink.notes.lock().unwrap(), vec!["Can't reach Homelab"], "once, not per poll");
        assert!(sink.tooltip.lock().unwrap().contains("Homelab: unreachable"));
        m.record_outage("p1", "Homelab", "c", None, 3);
        assert_eq!(sink.notes.lock().unwrap().last().unwrap(), "Homelab is reachable again");
        assert!(sink.tooltip.lock().unwrap().contains("all clear"));
    }

    #[tokio::test]
    async fn pause_persists_and_shows_in_tray() {
        let (m, sink) = monitor();
        m.set_paused(true).await.unwrap();
        assert!(m.status().paused);
        assert!(m.store().load_settings().unwrap().monitoring_paused, "survives restart");
        assert!(sink.tooltip.lock().unwrap().contains("paused"));
        m.set_paused(false).await.unwrap();
        assert!(!m.status().paused);
        assert!(!sink.tooltip.lock().unwrap().contains("paused"));
    }

    #[test]
    fn alerts_dedupe_and_respect_toggle() {
        let (m, sink) = monitor();
        let settings = m.settings();
        let issues = vec![crit("a")];
        m.process_alerts("p1", "Homelab", "c", &issues, &settings);
        m.process_alerts("p1", "Homelab", "c", &issues, &settings);
        assert_eq!(sink.notes.lock().unwrap().len(), 1);
        assert!(sink.tooltip.lock().unwrap().contains("Homelab: 1 critical"));

        let mut off = settings.clone();
        off.notify_critical = false;
        m.process_alerts("p1", "Homelab", "c", &[crit("a"), crit("b")], &off);
        assert_eq!(sink.notes.lock().unwrap().len(), 1, "notifications off → silent");
        assert!(sink.tooltip.lock().unwrap().contains("2 critical"), "tray still updates");
    }
    use portside_core::logline::Level;

    #[test]
    fn parse_skips_already_seen_and_blank() {
        let text = "2026-09-26T10:00:00.000000001Z old line\n\
                    2026-09-26T10:00:00.000000002Z \x1b[31mERROR\x1b[0m boom\n\
                    2026-09-26T10:00:00.000000003Z \n\
                    garbage without timestamp\n";
        let first: i64 = "2026-09-26T10:00:00.000000001Z"
            .parse::<portside_core::jiff::Timestamp>()
            .unwrap()
            .as_nanosecond() as i64;
        let (lines, last) = parse_lines(text, Some(first), "ns", "p", "c");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].message, "ERROR boom");
        assert_eq!(lines[0].level, Level::Error);
        assert_eq!(last, Some(first + 2), "blank line still advances the cursor");
    }
}
