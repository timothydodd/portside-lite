//! Backups this PC makes for every saved connection, on the schedules in
//! Settings (local time): copying the volume backups' archives off each
//! cluster, and manifest backups of each cluster. A check every few minutes
//! runs whatever is due, so a run missed while the PC was off or Portside
//! closed happens soon after the next start. A failed run is retried an hour
//! later rather than at every check.
//!
//! On disk, under the backup folder:
//! `<cluster>/volumes/<namespace>/<backup>/<backup>-<UTC time>.tar.gz` and
//! `<cluster>/manifests/<local time>/…` (see `manifests::manifest_backup`).

use std::collections::HashMap;
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
const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER_MS: i64 = 60 * 60 * 1000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupJob {
    /// Copy volume backup archives to this PC.
    Pull,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalBackupStatus {
    pub folder: String,
    /// What it's doing right now, if anything.
    pub activity: Option<String>,
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

/// State kept on the monitor.
#[derive(Default)]
pub(crate) struct LocalBackups {
    records: std::sync::Mutex<HashMap<String, ProfileRecord>>,
    forced: std::sync::Mutex<Vec<BackupJob>>,
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
    /// The configured backup folder, else `backups` in the app's data folder.
    pub fn backup_root(&self) -> PathBuf {
        let configured = self.settings.read().unwrap().backup_dir.clone().filter(|d| !d.trim().is_empty());
        configured.map(PathBuf::from).unwrap_or_else(|| self.data_dir.join("backups"))
    }

    fn cluster_folder(p: &portside_core::ConnectionProfile) -> String {
        safe_segment(&p.connection.cluster_id())
    }

    pub fn local_backup_status(&self) -> LocalBackupStatus {
        let settings = self.settings();
        let records = self.local_backups.records.lock().unwrap().clone();
        LocalBackupStatus {
            folder: self.backup_root().to_string_lossy().into_owned(),
            activity: self.local_backups.activity.read().unwrap().clone(),
            profiles: settings
                .connections
                .iter()
                .map(|p| {
                    let r = records.get(&p.id).cloned().unwrap_or_default();
                    ProfileBackupStatus { profile_id: p.id.clone(), name: p.name.clone(), folder: Self::cluster_folder(p), pull: r.pull, manifests: r.manifests }
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

    /// Run these jobs for every connection now, whatever their schedule.
    pub fn run_local_backups_now(&self, jobs: &[BackupJob]) {
        self.local_backups.forced.lock().unwrap().extend_from_slice(jobs);
        self.local_backups.wake.notify_one();
    }

    pub async fn run_local_backup_loop(self: std::sync::Arc<Self>) {
        // Let startup (and the first poll) settle first.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            if !self.settings.read().unwrap().monitoring_paused {
                self.local_backups_once().await;
            }
            tokio::select! {
                _ = tokio::time::sleep(CHECK_EVERY) => {}
                _ = self.local_backups.wake.notified() => {}
            }
        }
    }

    fn set_activity(&self, what: Option<String>) {
        *self.local_backups.activity.write().unwrap() = what;
    }

    fn record(&self, profile_id: &str, job: BackupJob, started: i64, result: Result<String, String>) {
        let json = {
            let mut records = self.local_backups.records.lock().unwrap();
            let r = records.entry(profile_id.to_string()).or_default();
            let j = match job {
                BackupJob::Pull => &mut r.pull,
                BackupJob::Manifests => &mut r.manifests,
            };
            j.last_attempt_ms = Some(started);
            match result {
                Ok(summary) => {
                    j.last_success_ms = Some(started);
                    j.error = None;
                    j.summary = Some(summary);
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

    async fn local_backups_once(&self) {
        let settings = self.settings();
        let forced: Vec<BackupJob> = std::mem::take(&mut *self.local_backups.forced.lock().unwrap());
        let root = self.backup_root();
        for p in &settings.connections {
            let rec = self.local_backups.records.lock().unwrap().get(&p.id).cloned().unwrap_or_default();
            let (now, now_local) = (now_ms(), Local::now().naive_local());
            let manifests = forced.contains(&BackupJob::Manifests)
                || (settings.manifest_backup && due(&settings.manifest_backup_schedule, &rec.manifests, now_local, now));
            let pull = forced.contains(&BackupJob::Pull) || (settings.backup_pull && due(&settings.backup_pull_schedule, &rec.pull, now_local, now));
            if !manifests && !pull {
                continue;
            }
            self.set_activity(Some(format!("Connecting to {}", p.name)));
            let connected = match tokio::time::timeout(CONNECT_TIMEOUT, self.connect_profile(&p.id)).await {
                Ok(Ok(cc)) => Ok(cc),
                Ok(Err(e)) => Err(format!("Couldn't connect: {e}")),
                Err(_) => Err(format!("Couldn't connect within {} s.", CONNECT_TIMEOUT.as_secs())),
            };
            let cc = match connected {
                Ok(cc) => cc,
                Err(e) => {
                    for (job, wanted) in [(BackupJob::Manifests, manifests), (BackupJob::Pull, pull)] {
                        if wanted {
                            self.record(&p.id, job, now, Err(e.clone()));
                        }
                    }
                    continue;
                }
            };
            let folder = root.join(Self::cluster_folder(p));
            if manifests {
                self.set_activity(Some(format!("Backing up {}'s manifests", p.name)));
                let started = now_ms();
                let result = self.manifest_backup_once(&cc, &p.name, &folder.join("manifests"), settings.manifest_backup_keep).await;
                self.record(&p.id, BackupJob::Manifests, started, result);
            }
            if pull {
                let started = now_ms();
                let result = self.pull_backups(&cc, &p.name, &folder.join("volumes"), &settings).await;
                self.record(&p.id, BackupJob::Pull, started, result);
            }
        }
        self.set_activity(None);
    }

    async fn manifest_backup_once(&self, cc: &ClusterClient, name: &str, dir: &Path, keep: u32) -> Result<String, String> {
        let backup = portside_kube::manifests::manifest_backup(&cc.client, name, now_ms()).await.map_err(err)?;
        let stamp = Local::now().format("%Y-%m-%d_%H%M").to_string();
        let summary = match backup.skipped.len() {
            0 => format!("{} objects", backup.objects),
            n => format!("{} objects; {n} kinds left out (see README.md)", backup.objects),
        };
        let dir = dir.to_path_buf();
        blocking(move || -> Result<(), String> {
            write_folder(&dir, &stamp, &backup.files)?;
            prune(&dir, keep, |e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'));
            Ok(())
        })
        .await?;
        Ok(summary)
    }

    /// Copy every volume backup's archives that aren't here yet, then keep
    /// the newest `backup_keep_local` of each.
    async fn pull_backups(&self, cc: &ClusterClient, profile: &str, dir: &Path, settings: &Settings) -> Result<String, String> {
        let backups = portside_kube::backups::list_with_storage(&cc.client).await.map_err(err)?;
        let (mut files, mut bytes, mut errors) = (0, 0, Vec::new());
        for (ns, name) in &backups {
            self.set_activity(Some(format!("Copying backups of {ns}/{name} from {profile}")));
            let local = dir.join(safe_segment(ns)).join(safe_segment(name));
            match self.pull_one(cc, ns, name, &local, &settings.files_helper_image).await {
                Ok((n, b)) => {
                    files += n;
                    bytes += b;
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
        let summary = match (backups.len(), files) {
            (0, _) => "no volume backups with storage yet".to_string(),
            (n, 0) => format!("nothing new from {n} backup{}", if n == 1 { "" } else { "s" }),
            (n, f) => format!("{f} new archive{} ({}) from {n} backup{}", if f == 1 { "" } else { "s" }, mb(bytes), if n == 1 { "" } else { "s" }),
        };
        if errors.is_empty() {
            Ok(summary)
        } else {
            Err(format!("{summary}, but: {}", errors.join("; ")))
        }
    }

    async fn pull_one(&self, cc: &ClusterClient, ns: &str, name: &str, local: &Path, image: &str) -> Result<(usize, u64), String> {
        let client = &cc.client;
        // Its own owner, so it never takes an open file browser's helper for an orphan.
        let owner = format!("{}-pull", self.install_id);
        let pod = kube_files::start_helper(client, ns, &object_name(name), image, None, true, &owner).await.map_err(err)?;
        let result = async {
            let listing = kube_files::list(client, ns, &pod, "").await.map_err(err)?;
            tokio::fs::create_dir_all(local).await.map_err(|e| format!("Couldn't create {}: {e}", local.display()))?;
            let (mut n, mut b) = (0, 0);
            let prefix = format!("{name}-");
            for e in listing.entries.iter().filter(|e| e.kind == "file" && e.name.starts_with(&prefix) && e.name.ends_with(ARCHIVE_SUFFIX)) {
                // Names come from the cluster: only plain file names are written here.
                if safe_segment(&e.name) != e.name {
                    continue;
                }
                let dest = local.join(&e.name);
                if tokio::fs::metadata(&dest).await.is_ok_and(|m| m.len() == e.size) {
                    continue;
                }
                kube_files::download(client, ns, &pod, &e.name, false, &dest, &|_| {}).await.map_err(err)?;
                n += 1;
                b += e.size;
            }
            Ok::<_, String>((n, b))
        }
        .await;
        let _ = kube_files::stop_helper(client, ns, &pod).await;
        result
    }
}

/// Write `files` into `<dir>/<stamp>`: staged in a hidden folder, renamed
/// into place once everything is written, so a half-written backup never
/// looks like a finished one.
fn write_folder(dir: &Path, stamp: &str, files: &[(String, String)]) -> Result<(), String> {
    let io = |what: &str, p: &Path, e: std::io::Error| format!("Couldn't {what} {}: {e}", p.display());
    let staging = dir.join(format!(".partial-{stamp}"));
    let _ = std::fs::remove_dir_all(&staging);
    for (rel, contents) in files {
        // Paths are built from safe segments; refuse anything else outright.
        if rel.split('/').any(|seg| seg.is_empty() || seg == "." || seg == ".." || safe_segment(seg) != seg) {
            return Err(format!("Refusing to write {rel}"));
        }
        let path = staging.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io("create", parent, e))?;
        }
        std::fs::write(&path, contents).map_err(|e| io("write", &path, e))?;
    }
    let mut target = dir.join(stamp);
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stamp}-{n}"));
        n += 1;
    }
    std::fs::rename(&staging, &target).map_err(|e| io("save", &target, e))
}

/// Keep the newest `keep` entries of `dir` that `matches` (by name, which
/// sorts by time for these); 0 keeps everything.
fn prune(dir: &Path, keep: u32, matches: impl Fn(&std::fs::DirEntry) -> bool) {
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
    fn writes_whole_folders_and_prunes() {
        let dir = std::env::temp_dir().join(format!("portside-backups-test-{}", now_ms()));
        let files = vec![("cluster/Namespace.yaml".to_string(), "a".to_string()), ("namespaces/shop/Deployment.yaml".to_string(), "b".to_string())];
        for stamp in ["2026-10-01_0300", "2026-10-02_0300", "2026-10-03_0300"] {
            write_folder(&dir, stamp, &files).unwrap();
        }
        write_folder(&dir, "2026-10-03_0300", &files).unwrap();
        assert!(dir.join("2026-10-03_0300-2/namespaces/shop/Deployment.yaml").exists(), "same minute twice: kept apart");
        assert!(write_folder(&dir, "x", &[("../evil".into(), String::new())]).is_err());
        prune(&dir, 2, |e| e.path().is_dir() && !e.file_name().to_string_lossy().starts_with('.'));
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
        left.sort();
        assert_eq!(left, ["2026-10-03_0300", "2026-10-03_0300-2"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
