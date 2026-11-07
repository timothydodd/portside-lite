//! Backups this PC makes for every saved connection, on the schedules in
//! Settings (local time): copying the volume backups' archives off each
//! cluster, and manifest backups of each cluster. A check every minute queues
//! whatever is due, so a run missed while the PC was off, asleep or Portside
//! closed happens right after the next start or wake. Queued jobs run one at a
//! time, across every connection, whether they came from a schedule or Run
//! now. A failed run is retried an hour later rather than at every check.
//!
//! On disk, under the backup folder:
//! `<cluster>/volumes/<namespace>/<backup>/<backup>-<UTC time>.tar.gz`,
//! `<cluster>/manifests/<local time>/…` (cluster backups, see
//! `clusterbackup.rs`) and `<cluster>/images/` (images only the node had).

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Local, NaiveDateTime, TimeZone};
use portside_core::archive::safe_segment;
use portside_core::backup::{object_name, ARCHIVE_SUFFIX};
use portside_core::{now_ms, schedule, Settings};
use portside_kube::files as kube_files;
use portside_kube::ClusterClient;
use serde::{Deserialize, Serialize};

use crate::{blocking, Monitor};

/// Where the job bookkeeping is kept (the store's settings table).
const STATE_KEY: &str = "local_backups";
const CHECK_EVERY: Duration = Duration::from_secs(60);
/// Lets startup settle before the first check.
const START_DELAY: Duration = Duration::from_secs(10);
/// After the PC wakes, how long to give its network to come back.
const WAKE_GRACE: Duration = Duration::from_secs(30);
const RETRY_AFTER_MS: i64 = 60 * 60 * 1000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupJob {
    /// Copy volume backup archives to this PC.
    Pull,
    /// A cluster backup: manifests, sealed secrets, node files, … (kept as
    /// `manifests` in settings and records for compatibility).
    Manifests,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct JobRecord {
    pub last_attempt_ms: Option<i64>,
    pub last_success_ms: Option<i64>,
    /// Why the last attempt failed (cleared by a success).
    pub error: Option<String>,
    /// What the last successful run did, in a few words.
    pub summary: Option<String>,
    /// Parts the last successful run couldn't save (the rest was saved).
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProfileRecord {
    pub pull: JobRecord,
    pub manifests: JobRecord,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileBackupStatus {
    pub profile_id: String,
    pub name: String,
    /// Its folder under the backup folder.
    pub folder: String,
    pub pull: JobRecord,
    pub manifests: JobRecord,
    /// The job running for this connection, if any.
    pub running: Option<BackupJob>,
    /// Its jobs waiting their turn.
    pub queued: Vec<BackupJob>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBackupStatus {
    pub folder: String,
    /// What it's doing right now, if anything.
    pub activity: Option<String>,
    /// Jobs waiting their turn, across every connection.
    pub queued: usize,
    pub profiles: Vec<ProfileBackupStatus>,
}

/// Archives of one volume backup on this PC.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalArchives {
    pub namespace: String,
    pub name: String,
    pub count: usize,
    pub bytes: u64,
    /// File time of the newest one.
    pub newest_ms: Option<i64>,
}

/// One job for one connection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Queued {
    profile_id: String,
    job: BackupJob,
}

/// Jobs waiting, and the one running. One lock, so a job is never both.
#[derive(Default)]
struct JobQueue {
    waiting: VecDeque<Queued>,
    running: Option<Queued>,
}

impl JobQueue {
    /// Adds it unless it's already waiting or running.
    fn push(&mut self, item: Queued) -> bool {
        if self.running.as_ref() == Some(&item) || self.waiting.contains(&item) {
            return false;
        }
        self.waiting.push_back(item);
        true
    }

    /// Takes the next job and marks it running.
    fn start_next(&mut self) -> Option<Queued> {
        self.running = self.waiting.pop_front();
        self.running.clone()
    }
}

/// State kept on the monitor.
#[derive(Default)]
pub(crate) struct LocalBackups {
    records: std::sync::Mutex<HashMap<String, ProfileRecord>>,
    queue: std::sync::Mutex<JobQueue>,
    activity: std::sync::RwLock<Option<String>>,
    wake: tokio::sync::Notify,
}

impl LocalBackups {
    pub(crate) fn load(store: &portside_store::Store) -> Self {
        let records = store.get_value(STATE_KEY).ok().flatten().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { records: std::sync::Mutex::new(records), ..Default::default() }
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Local time to epoch ms; a time skipped by a DST change counts as an hour later.
fn local_ms(t: NaiveDateTime) -> Option<i64> {
    Local.from_local_datetime(&t).earliest().or_else(|| Local.from_local_datetime(&(t + chrono::Duration::hours(1))).earliest()).map(|t| t.timestamp_millis())
}

/// Due: the most recent scheduled time has passed since the last success, and
/// it hasn't failed in the last hour. Never run = due.
fn due(cron: &str, rec: &JobRecord, now_local: NaiveDateTime, now: i64) -> bool {
    let Some(every) = schedule::parse(cron) else { return false };
    let last_due = local_ms(schedule::last_due(every, now_local)).unwrap_or(now);
    if rec.last_success_ms.is_some_and(|s| s >= last_due) {
        return false;
    }
    !rec.last_attempt_ms.is_some_and(|a| a >= last_due && now - a < RETRY_AFTER_MS)
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

impl Monitor {
    /// The configured backup folder, else `backups` in the app's data folder. A folder that isn't
    /// absolute on this OS (settings carried over from Windows, e.g. `H:\Clusters`, on a Mac) is
    /// ignored: it would resolve against the working directory and scatter backups there.
    pub fn backup_root(&self) -> PathBuf {
        let configured = self.settings.read().unwrap().backup_dir.clone().filter(|d| !d.trim().is_empty());
        configured.map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| self.data_dir.join("backups"))
    }

    fn cluster_folder(p: &portside_core::ConnectionProfile) -> String {
        safe_segment(&p.connection.cluster_id())
    }

    pub fn local_backup_status(&self) -> LocalBackupStatus {
        let settings = self.settings();
        let records = self.local_backups.records.lock().unwrap().clone();
        let (waiting, running) = {
            let q = self.local_backups.queue.lock().unwrap();
            (q.waiting.clone(), q.running.clone())
        };
        LocalBackupStatus {
            folder: self.backup_root().to_string_lossy().into_owned(),
            activity: self.local_backups.activity.read().unwrap().clone(),
            queued: waiting.len(),
            profiles: settings
                .connections
                .iter()
                .map(|p| {
                    let r = records.get(&p.id).cloned().unwrap_or_default();
                    ProfileBackupStatus {
                        profile_id: p.id.clone(),
                        name: p.name.clone(),
                        folder: Self::cluster_folder(p),
                        pull: r.pull,
                        manifests: r.manifests,
                        running: running.as_ref().filter(|q| q.profile_id == p.id).map(|q| q.job),
                        queued: waiting.iter().filter(|q| q.profile_id == p.id).map(|q| q.job).collect(),
                    }
                })
                .collect(),
        }
    }

    /// Archives on this PC for each volume backup of the active connection.
    pub async fn local_archives(&self) -> Vec<LocalArchives> {
        let Some(profile) = self.settings().active_profile().cloned() else { return Vec::new() };
        let dir = self.backup_root().join(Self::cluster_folder(&profile)).join("volumes");
        blocking(move || {
            let mut out = Vec::new();
            for ns in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                for b in std::fs::read_dir(ns.path()).into_iter().flatten().flatten() {
                    let mut set = LocalArchives {
                        namespace: ns.file_name().to_string_lossy().into_owned(),
                        name: b.file_name().to_string_lossy().into_owned(),
                        count: 0,
                        bytes: 0,
                        newest_ms: None,
                    };
                    for f in std::fs::read_dir(b.path()).into_iter().flatten().flatten() {
                        let name = f.file_name().to_string_lossy().into_owned();
                        let Ok(meta) = f.metadata() else { continue };
                        if !name.ends_with(ARCHIVE_SUFFIX) || !meta.is_file() {
                            continue;
                        }
                        set.count += 1;
                        set.bytes += meta.len();
                        let modified = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
                        set.newest_ms = set.newest_ms.max(modified);
                    }
                    out.push(set);
                }
            }
            out
        })
        .await
    }

    /// Queue these jobs for every connection, whatever their schedule. They
    /// run after anything already queued.
    pub fn run_local_backups_now(&self, jobs: &[BackupJob]) {
        let settings = self.settings();
        let mut q = self.local_backups.queue.lock().unwrap();
        for p in &settings.connections {
            for job in jobs {
                q.push(Queued { profile_id: p.id.clone(), job: *job });
            }
        }
        drop(q);
        self.local_backups.wake.notify_one();
    }

    pub async fn run_local_backup_loop(self: std::sync::Arc<Self>) {
        tokio::time::sleep(START_DELAY).await;
        let mut last_check = now_ms();
        loop {
            // Far more time passed than the wait asked for: the PC was asleep.
            // Its network may not be back yet, and a failed run waits an hour.
            if now_ms() - last_check > 2 * CHECK_EVERY.as_millis() as i64 {
                tokio::time::sleep(WAKE_GRACE).await;
            }
            if !self.paused() {
                self.queue_due_jobs();
                self.run_queue().await;
            }
            last_check = now_ms();
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = self.local_backups.wake.notified() => {}
            }
        }
    }

    fn paused(&self) -> bool {
        self.settings.read().unwrap().monitoring_paused
    }

    /// Queues every job whose scheduled time has passed since it last succeeded.
    fn queue_due_jobs(&self) {
        let settings = self.settings();
        let (now, now_local) = (now_ms(), Local::now().naive_local());
        let records = self.local_backups.records.lock().unwrap().clone();
        let mut q = self.local_backups.queue.lock().unwrap();
        for p in &settings.connections {
            let rec = records.get(&p.id).cloned().unwrap_or_default();
            if settings.manifest_backup && due(&settings.manifest_backup_schedule, &rec.manifests, now_local, now) {
                q.push(Queued { profile_id: p.id.clone(), job: BackupJob::Manifests });
            }
            if settings.backup_pull && due(&settings.backup_pull_schedule, &rec.pull, now_local, now) {
                q.push(Queued { profile_id: p.id.clone(), job: BackupJob::Pull });
            }
        }
    }

    fn set_activity(&self, what: Option<String>) {
        *self.local_backups.activity.write().unwrap() = what;
    }

    pub(crate) fn set_backup_activity(&self, what: String) {
        self.set_activity(Some(what));
    }

    fn record(&self, profile_id: &str, job: BackupJob, started: i64, result: Result<(String, Vec<String>), String>) {
        let json = {
            let mut records = self.local_backups.records.lock().unwrap();
            let r = records.entry(profile_id.to_string()).or_default();
            let j = match job {
                BackupJob::Pull => &mut r.pull,
                BackupJob::Manifests => &mut r.manifests,
            };
            j.last_attempt_ms = Some(started);
            match result {
                Ok((summary, warnings)) => {
                    j.last_success_ms = Some(started);
                    j.error = None;
                    j.summary = Some(summary);
                    j.warning = (!warnings.is_empty()).then(|| warnings.join(" "));
                }
                Err(e) => j.error = Some(e),
            }
            // Profiles that were deleted don't need their records.
            let live: Vec<String> = self.settings.read().unwrap().connections.iter().map(|p| p.id.clone()).collect();
            records.retain(|id, _| live.contains(id));
            serde_json::to_string(&*records).unwrap_or_default()
        };
        let _ = self.store.set_value(STATE_KEY, &json);
    }

    /// Runs queued jobs one at a time until the queue is empty (or monitoring
    /// is paused; what's left waits). Jobs queued meanwhile join the line.
    async fn run_queue(&self) {
        // Back-to-back jobs for one connection share its connection (or its failure).
        let mut conn: Option<(String, Result<std::sync::Arc<ClusterClient>, String>)> = None;
        while !self.paused() {
            let Some(item) = self.local_backups.queue.lock().unwrap().start_next() else { break };
            let settings = self.settings();
            // Deleted while it waited.
            let Some(p) = settings.connections.iter().find(|p| p.id == item.profile_id) else { continue };
            if conn.as_ref().is_none_or(|(id, _)| *id != p.id) {
                drop(conn.take()); // close the last one (and its SSH tunnel) before opening another
                self.set_activity(Some(format!("Connecting to {}", p.name)));
                let connected = match tokio::time::timeout(CONNECT_TIMEOUT, self.connect_profile(&p.id)).await {
                    Ok(Ok(cc)) => Ok(cc),
                    Ok(Err(e)) => Err(format!("Couldn't connect: {e}")),
                    Err(_) => Err(format!("Couldn't connect within {} s.", CONNECT_TIMEOUT.as_secs())),
                };
                conn = Some((p.id.clone(), connected));
            }
            let started = now_ms();
            let result = match &conn.as_ref().expect("connected above").1 {
                Ok(cc) => {
                    let folder = self.backup_root().join(Self::cluster_folder(p));
                    match item.job {
                        BackupJob::Manifests => {
                            self.set_activity(Some(format!("Backing up {}", p.name)));
                            self.cluster_backup_once(cc, p, &folder, &settings).await
                        }
                        BackupJob::Pull => self.pull_backups(cc, &p.name, &folder.join("volumes"), &settings).await.map(|s| (s, Vec::new())),
                    }
                }
                Err(e) => Err(e.clone()),
            };
            self.record(&p.id, item.job, started, result);
        }
        self.local_backups.queue.lock().unwrap().running = None;
        self.set_activity(None);
    }

    /// Copy every volume backup's archives that aren't here yet, then keep
    /// the newest `backup_keep_local` of each.
    async fn pull_backups(&self, cc: &ClusterClient, profile: &str, dir: &Path, settings: &Settings) -> Result<String, String> {
        let backups = portside_kube::backups::list_with_storage(&cc.client).await.map_err(err)?;
        let (mut files, mut bytes, mut removed, mut errors) = (0, 0, 0, Vec::new());
        for (ns, name) in &backups {
            self.set_activity(Some(format!("Copying backups of {ns}/{name} from {profile}")));
            let local = dir.join(safe_segment(ns)).join(safe_segment(name));
            match self.pull_one(cc, ns, name, &local, settings).await {
                Ok((n, b, r)) => {
                    files += n;
                    bytes += b;
                    removed += r;
                }
                Err(e) => errors.push(format!("{ns}/{name}: {e}")),
            }
            let keep = settings.backup_keep_local;
            let prefix = format!("{name}-");
            let _ = blocking(move || {
                prune(&local, keep, |e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.starts_with(&prefix) && n.ends_with(ARCHIVE_SUFFIX)
                })
            })
            .await;
        }
        let mut summary = match (backups.len(), files) {
            (0, _) => "no volume backups with storage yet".to_string(),
            (n, 0) => format!("nothing new from {n} backup{}", if n == 1 { "" } else { "s" }),
            (n, f) => format!("{f} new archive{} ({}) from {n} backup{}", if f == 1 { "" } else { "s" }, mb(bytes), if n == 1 { "" } else { "s" }),
        };
        if removed > 0 {
            summary.push_str(&format!("; {removed} verified and removed from the cluster"));
        }
        if errors.is_empty() {
            Ok(summary)
        } else {
            Err(format!("{summary}, but: {}", errors.join("; ")))
        }
    }

    /// Copy one backup's archives that aren't here yet. With
    /// `backup_pull_remove`, each archive here whose SHA-256 matches the
    /// cluster's copy is then deleted from the backup claim, as long as no
    /// run is using it. Returns (files copied, bytes copied, files removed).
    async fn pull_one(&self, cc: &ClusterClient, ns: &str, name: &str, local: &Path, settings: &Settings) -> Result<(usize, u64, usize), String> {
        let client = &cc.client;
        let claim = object_name(name);
        // Deleting needs a read-write helper, which is only safe between runs.
        let removable = settings.backup_pull_remove
            && kube_files::claim_users(client, ns, &claim).await.is_ok_and(|u| portside_core::backup::archives_removable(&u, name));
        // Its own owner, so it never takes an open file browser's helper for an orphan.
        let owner = format!("{}-pull", self.install_id);
        let pod = kube_files::start_helper(client, ns, &claim, &settings.files_helper_image, None, !removable, &owner).await.map_err(err)?;
        let result = async {
            let listing = kube_files::list(client, ns, &pod, "").await.map_err(err)?;
            tokio::fs::create_dir_all(local).await.map_err(|e| format!("Couldn't create {}: {e}", local.display()))?;
            let (mut n, mut b, mut removed) = (0, 0, 0);
            let prefix = format!("{name}-");
            for e in listing.entries.iter().filter(|e| e.kind == "file" && e.name.starts_with(&prefix) && e.name.ends_with(ARCHIVE_SUFFIX)) {
                // Names come from the cluster: only plain file names are written here.
                if safe_segment(&e.name) != e.name {
                    continue;
                }
                let dest = local.join(&e.name);
                if !tokio::fs::metadata(&dest).await.is_ok_and(|m| m.len() == e.size) {
                    kube_files::download(client, ns, &pod, &e.name, false, &dest, &|_| {}).await.map_err(err)?;
                    n += 1;
                    b += e.size;
                }
                if removable {
                    self.verify_and_remove(cc, ns, &pod, &e.name, &dest).await?;
                    removed += 1;
                }
            }
            Ok::<_, String>((n, b, removed))
        }
        .await;
        let _ = kube_files::stop_helper(client, ns, &pod).await;
        result
    }

    /// Delete `file` from the claim only if the copy at `local` has the same SHA-256.
    async fn verify_and_remove(&self, cc: &ClusterClient, ns: &str, pod: &str, file: &str, local: &Path) -> Result<(), String> {
        let client = &cc.client;
        let out = kube_files::run(client, ns, pod, portside_core::files::sha256_command(file)).await.map_err(err)?;
        let remote = portside_core::files::parse_sha256(&String::from_utf8_lossy(&out)).ok_or_else(|| format!("Couldn't read the checksum of {file} on the cluster"))?;
        let path = local.to_path_buf();
        let here = blocking(move || -> std::io::Result<String> {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            std::io::copy(&mut std::fs::File::open(&path)?, &mut hasher)?;
            Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
        })
        .await
        .map_err(|e| format!("Couldn't read {}: {e}", local.display()))?;
        if here != remote {
            // A bad copy: drop it so the next run downloads it again; the cluster keeps its copy.
            let _ = tokio::fs::remove_file(local).await;
            return Err(format!("{file} copied to this PC doesn't match the cluster's (checksum differs); kept on the cluster, will copy again"));
        }
        kube_files::run(client, ns, pod, portside_core::files::remove_command(file)).await.map_err(|e| format!("{file} is safely on this PC, but couldn't be deleted from the cluster: {e}"))?;
        Ok(())
    }
}

/// Keep the newest `keep` entries of `dir` that `matches` (by name, which
/// sorts by time for these); 0 keeps everything.
pub(crate) fn prune(dir: &Path, keep: u32, matches: impl Fn(&std::fs::DirEntry) -> bool) {
    if keep == 0 {
        return;
    }
    let mut entries: Vec<std::fs::DirEntry> = std::fs::read_dir(dir).into_iter().flatten().flatten().filter(|e| matches(e)).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for e in entries.into_iter().skip(keep as usize) {
        let p = e.path();
        let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> NaiveDateTime {
        NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap()
    }

    #[test]
    fn due_after_the_scheduled_time_until_it_succeeds() {
        let daily = "0 2 * * *";
        let now_local = t("2026-10-11 10:00");
        let now = local_ms(now_local).unwrap();
        let due_at = local_ms(t("2026-10-11 02:00")).unwrap();
        let rec = |attempt: Option<i64>, success: Option<i64>| JobRecord { last_attempt_ms: attempt, last_success_ms: success, ..Default::default() };
        assert!(due(daily, &rec(None, None), now_local, now), "never run");
        assert!(due(daily, &rec(Some(due_at - 1000), Some(due_at - 1000)), now_local, now), "last ran before today's 02:00");
        assert!(!due(daily, &rec(Some(due_at + 1000), Some(due_at + 1000)), now_local, now), "already ran today");
        let failed_recently = rec(Some(now - 10 * 60 * 1000), Some(due_at - 86_400_000));
        assert!(!due(daily, &failed_recently, now_local, now), "failed ten minutes ago: wait");
        let failed_earlier = rec(Some(now - 2 * RETRY_AFTER_MS), Some(due_at - 86_400_000));
        assert!(due(daily, &failed_earlier, now_local, now), "failed two hours ago: try again");
        assert!(!due("not a schedule", &rec(None, None), now_local, now));
    }

    #[test]
    fn queue_runs_each_job_once_in_order() {
        let job = |id: &str, job| Queued { profile_id: id.into(), job };
        let mut q = JobQueue::default();
        assert!(q.push(job("a", BackupJob::Pull)));
        assert!(q.push(job("b", BackupJob::Pull)));
        assert!(!q.push(job("a", BackupJob::Pull)), "already waiting");
        assert_eq!(q.start_next(), Some(job("a", BackupJob::Pull)));
        assert!(!q.push(job("a", BackupJob::Pull)), "already running");
        assert!(q.push(job("a", BackupJob::Manifests)));
        assert_eq!(q.start_next(), Some(job("b", BackupJob::Pull)));
        assert!(q.push(job("a", BackupJob::Pull)), "finished, so it can queue again");
        assert_eq!(q.start_next(), Some(job("a", BackupJob::Manifests)));
        assert_eq!(q.start_next(), Some(job("a", BackupJob::Pull)));
        assert_eq!(q.start_next(), None);
        assert_eq!(q.running, None);
    }

    #[test]
    fn prunes_to_the_newest() {
        let dir = std::env::temp_dir().join(format!("portside-backups-test-{}", now_ms()));
        for stamp in ["2026-10-01_0300", "2026-10-02_0300", "2026-10-03_0300", ".partial-x"] {
            std::fs::create_dir_all(dir.join(stamp)).unwrap();
        }
        prune(&dir, 2, |e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'));
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left, [".partial-x", "2026-10-02_0300", "2026-10-03_0300"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
