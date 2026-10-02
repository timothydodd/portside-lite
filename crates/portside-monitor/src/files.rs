//! Volume file browser sessions. A session is one helper pod mounting one
//! PersistentVolumeClaim; views that open the same claim share it (ref
//! counted), and it's deleted shortly after the last view closes, when the
//! user switches clusters, or when the app quits.
//!
//! Writes need two things: the helper mounted the claim read-write (only done
//! when nothing else was using it), and a live re-check right before the
//! write still finds nothing using it. So scaling the app back up turns
//! writes off again even mid-session.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use portside_core::files::{self as core_files, FileListing, HELPER_LIFETIME_SECS};
use portside_core::now_ms;
use portside_kube::files as kube_files;
use portside_kube::ClusterClient;
use serde::Serialize;
use tokio::sync::watch;

use crate::Monitor;

/// How long a session with no open views keeps its helper, so flipping
/// between tabs doesn't restart the pod every time.
const IDLE_CLOSE: Duration = Duration::from_secs(30);
/// Upper bound for deleting a helper on shutdown or cluster switch.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const PROGRESS_EVERY: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSession {
    pub id: u64,
    pub profile_id: String,
    pub namespace: String,
    pub claim: String,
    /// The helper pod.
    pub pod: String,
    /// Files can be changed right now.
    pub writable: bool,
    /// Why not, when `writable` is false.
    pub blockers: Vec<String>,
    /// Running/pending pods that mount the claim (helpers excluded).
    pub mounted_by: Vec<String>,
    pub started_ms: i64,
    /// When the helper stops on its own.
    pub expires_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileProgress {
    pub transfer_id: String,
    /// What's moving right now (a file name).
    pub label: String,
    pub done: u64,
    /// `None` for folder downloads (the archive size isn't known ahead).
    pub total: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSummary {
    pub files: usize,
    pub folders: usize,
    pub bytes: u64,
}

pub(crate) struct SessionEntry {
    info: FileSession,
    client: Arc<ClusterClient>,
    /// The helper has the claim mounted read-write.
    mounted_rw: bool,
    refs: usize,
    /// Bumped on every open/close so a stale idle timer does nothing.
    generation: u64,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Calls `emit` at most every [`PROGRESS_EVERY`], plus on the first call.
fn throttled(emit: impl Fn(u64) + Send + Sync) -> impl Fn(u64) + Send + Sync {
    let last = StdMutex::new(None::<Instant>);
    move |n| {
        let mut l = last.lock().unwrap();
        if l.is_none_or(|t| t.elapsed() >= PROGRESS_EVERY) {
            *l = Some(Instant::now());
            emit(n);
        }
    }
}

impl Monitor {
    /// Open (or join) a file browser session for `namespace/claim` on the
    /// active cluster, starting the helper pod if needed.
    pub async fn open_files(&self, namespace: &str, claim: &str) -> Result<FileSession, String> {
        let profile_id = self.settings().active_connection_id.ok_or("No cluster connection configured.")?;
        let mut sessions = self.file_sessions.lock().await;
        if let Some(entry) =
            sessions.values_mut().find(|e| e.info.profile_id == profile_id && e.info.namespace == namespace && e.info.claim == claim)
        {
            self.refresh_entry(entry).await?;
            entry.refs += 1;
            entry.generation += 1;
            return Ok(entry.info.clone());
        }
        let client = self.client().await?;
        let id = self.next_file_session.fetch_add(1, Ordering::Relaxed);
        let mut entry = SessionEntry {
            info: FileSession {
                id,
                profile_id,
                namespace: namespace.into(),
                claim: claim.into(),
                pod: String::new(),
                writable: false,
                blockers: Vec::new(),
                mounted_by: Vec::new(),
                started_ms: 0,
                expires_ms: 0,
            },
            client,
            mounted_rw: false,
            refs: 1,
            generation: 0,
        };
        self.refresh_entry(&mut entry).await?;
        let info = entry.info.clone();
        sessions.insert(id, entry);
        Ok(info)
    }

    /// Re-check who uses the claim; (re)start the helper when it's gone or
    /// when it should now be mounted the other way (read-only ↔ read-write).
    pub async fn refresh_files(&self, id: u64) -> Result<FileSession, String> {
        let mut sessions = self.file_sessions.lock().await;
        let entry = sessions.get_mut(&id).ok_or("That file browser was closed. Reopen the volume.")?;
        self.refresh_entry(entry).await?;
        Ok(entry.info.clone())
    }

    async fn refresh_entry(&self, entry: &mut SessionEntry) -> Result<(), String> {
        let c = &entry.client.client;
        let (ns, claim) = (entry.info.namespace.clone(), entry.info.claim.clone());
        let users = kube_files::claim_users(c, &ns, &claim).await.map_err(err)?;
        let want_rw = users.blockers.is_empty();
        let alive = !entry.info.pod.is_empty() && kube_files::helper_running(c, &ns, &entry.info.pod).await.unwrap_or(false);
        if !alive || entry.mounted_rw != want_rw {
            if !entry.info.pod.is_empty() {
                let _ = kube_files::stop_helper(c, &ns, &entry.info.pod).await;
                entry.info.pod.clear();
            }
            // Read-only next to a running app: share its node, which
            // ReadWriteOnce storage needs to attach the volume twice.
            let node = users.pods.iter().find_map(|(_, n)| n.clone());
            let image = self.settings().files_helper_image;
            let pod = kube_files::start_helper(c, &ns, &claim, &image, node.as_deref().filter(|_| !want_rw), !want_rw)
                .await
                .map_err(err)?;
            entry.info.pod = pod;
            entry.mounted_rw = want_rw;
            entry.info.started_ms = now_ms();
            entry.info.expires_ms = entry.info.started_ms + HELPER_LIFETIME_SECS as i64 * 1000;
        }
        entry.info.writable = entry.mounted_rw && users.blockers.is_empty();
        entry.info.blockers = users.blockers;
        entry.info.mounted_by = users.pods.into_iter().map(|(p, _)| p).collect();
        Ok(())
    }

    /// A view closed. The helper goes once no view has used it for a while.
    pub async fn close_files(self: &Arc<Self>, id: u64) {
        let generation = {
            let mut sessions = self.file_sessions.lock().await;
            let Some(entry) = sessions.get_mut(&id) else { return };
            entry.refs = entry.refs.saturating_sub(1);
            entry.generation += 1;
            if entry.refs > 0 {
                return;
            }
            entry.generation
        };
        let monitor = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(IDLE_CLOSE).await;
            let entry = {
                let mut sessions = monitor.file_sessions.lock().await;
                match sessions.get(&id) {
                    Some(e) if e.refs == 0 && e.generation == generation => sessions.remove(&id),
                    _ => None,
                }
            };
            if let Some(e) = entry {
                stop(&e).await;
            }
        });
    }

    /// Delete every helper now (quit, or the active cluster changed).
    pub async fn close_all_files(&self) {
        let entries: Vec<SessionEntry> = self.file_sessions.lock().await.drain().map(|(_, e)| e).collect();
        futures::future::join_all(entries.iter().map(stop)).await;
    }

    async fn session(&self, id: u64) -> Result<(Arc<ClusterClient>, FileSession), String> {
        let sessions = self.file_sessions.lock().await;
        let e = sessions.get(&id).ok_or("That file browser was closed. Reopen the volume.")?;
        Ok((Arc::clone(&e.client), e.info.clone()))
    }

    /// The session, if a write is allowed right now (checked live).
    async fn writable_session(&self, id: u64) -> Result<(Arc<ClusterClient>, FileSession), String> {
        let (client, info) = self.session(id).await?;
        let rw = self.file_sessions.lock().await.get(&id).is_some_and(|e| e.mounted_rw);
        if !rw {
            return Err(format!("Read-only: {}", info.blockers.join(" ")));
        }
        let users = kube_files::claim_users(&client.client, &info.namespace, &info.claim).await.map_err(err)?;
        if !users.blockers.is_empty() {
            let msg = format!("Writes are off now. {}", users.blockers.join(" "));
            if let Some(e) = self.file_sessions.lock().await.get_mut(&id) {
                e.info.writable = false;
                e.info.blockers = users.blockers;
                e.info.mounted_by = users.pods.into_iter().map(|(p, _)| p).collect();
            }
            return Err(msg);
        }
        Ok((client, info))
    }

    pub async fn list_files(&self, id: u64, path: &str) -> Result<FileListing, String> {
        let rel = core_files::clean_rel(path)?;
        let (client, info) = self.session(id).await?;
        kube_files::list(&client.client, &info.namespace, &info.pod, &rel).await.map_err(err)
    }

    pub async fn make_dir(&self, id: u64, dir: &str, name: &str) -> Result<(), String> {
        let dir = core_files::clean_rel(dir)?;
        core_files::valid_name(name)?;
        let (client, info) = self.writable_session(id).await?;
        if kube_files::list(&client.client, &info.namespace, &info.pod, &dir).await.map_err(err)?.entries.iter().any(|e| e.name == name) {
            return Err(format!("Something named {name} is already there."));
        }
        let cmd = core_files::mkdirs_command(&dir, &[name.to_string()]);
        kube_files::run(&client.client, &info.namespace, &info.pod, cmd).await.map(drop).map_err(err)
    }

    pub async fn delete_path(&self, id: u64, path: &str) -> Result<(), String> {
        let rel = core_files::clean_rel(path)?;
        if rel.is_empty() {
            return Err("The volume root can't be deleted.".into());
        }
        let (client, info) = self.writable_session(id).await?;
        kube_files::run(&client.client, &info.namespace, &info.pod, core_files::remove_command(&rel)).await.map(drop).map_err(err)
    }

    pub async fn rename_path(&self, id: u64, path: &str, new_name: &str) -> Result<(), String> {
        let rel = core_files::clean_rel(path)?;
        core_files::valid_name(new_name)?;
        if rel.is_empty() {
            return Err("The volume root can't be renamed.".into());
        }
        let parent = rel.rsplit_once('/').map_or("", |(p, _)| p);
        let to = core_files::join(parent, new_name);
        let (client, info) = self.writable_session(id).await?;
        kube_files::run(&client.client, &info.namespace, &info.pod, core_files::rename_command(&rel, &to))
            .await
            .map(drop)
            .map_err(err)
    }

    fn progress_fn(&self, transfer_id: &str) -> impl Fn(&str, u64, Option<u64>) + Send + Sync + use<> {
        let sink = Arc::clone(&self.sink);
        let id = transfer_id.to_string();
        move |label: &str, done, total| {
            sink.file_progress(&FileProgress { transfer_id: id.clone(), label: label.to_string(), done, total });
        }
    }

    /// Run `fut` until it finishes or the user cancels `transfer_id`.
    async fn cancellable<T>(&self, transfer_id: &str, fut: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
        let (tx, mut rx) = watch::channel(false);
        self.transfers.lock().unwrap().insert(transfer_id.to_string(), tx);
        let res = tokio::select! {
            r = fut => r,
            _ = rx.wait_for(|c| *c) => Err("Cancelled.".to_string()),
        };
        self.transfers.lock().unwrap().remove(transfer_id);
        res
    }

    pub fn cancel_transfer(&self, transfer_id: &str) {
        if let Some(tx) = self.transfers.lock().unwrap().get(transfer_id) {
            let _ = tx.send(true);
        }
    }

    /// Save a file, or a folder as .tar.gz, to `dest` on this computer.
    pub async fn download_path(&self, id: u64, path: &str, folder: bool, dest: &str, transfer_id: &str) -> Result<u64, String> {
        let rel = core_files::clean_rel(path)?;
        let (client, info) = self.session(id).await?;
        let dest = PathBuf::from(dest);
        let label = rel.rsplit('/').next().unwrap_or("volume").to_string();
        let total = if folder {
            None
        } else {
            let (dir, name) = rel.rsplit_once('/').unwrap_or(("", rel.as_str()));
            kube_files::list(&client.client, &info.namespace, &info.pod, dir)
                .await
                .ok()
                .and_then(|l| l.entries.into_iter().find(|e| e.name == name).map(|e| e.size))
        };
        let emit = self.progress_fn(transfer_id);
        let progress = throttled(move |n| emit(&label, n, total));
        let res = self
            .cancellable(transfer_id, async {
                kube_files::download(&client.client, &info.namespace, &info.pod, &rel, folder, &dest, &progress)
                    .await
                    .map_err(err)
            })
            .await;
        if res.is_err() {
            // Cancelled mid-copy: don't leave half a file behind.
            let _ = tokio::fs::remove_file(&dest).await;
        }
        res
    }

    /// Upload local files and folders (with their contents) into `dir`.
    /// Existing files with the same name are replaced; the UI confirms that.
    pub async fn upload_paths(&self, id: u64, dir: &str, sources: Vec<String>, transfer_id: &str) -> Result<UploadSummary, String> {
        let dir = core_files::clean_rel(dir)?;
        let (client, info) = self.writable_session(id).await?;
        let paths: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
        let tree = tokio::task::spawn_blocking(move || kube_files::walk_local(&paths))
            .await
            .map_err(err)?
            .map_err(|e| format!("Couldn't read what you picked: {e}"))?;
        let total: u64 = tree.files.iter().map(|f| f.2).sum();
        let emit = self.progress_fn(transfer_id);
        self.cancellable(transfer_id, async {
            let c = &client.client;
            if !tree.dirs.is_empty() {
                kube_files::run(c, &info.namespace, &info.pod, core_files::mkdirs_command(&dir, &tree.dirs)).await.map_err(err)?;
            }
            let mut done = 0u64;
            for (local, rel, size) in &tree.files {
                let label = rel.clone();
                let base = done;
                let progress = throttled(|n| emit(&label, base + n, Some(total)));
                progress(0);
                kube_files::upload_file(c, &info.namespace, &info.pod, local, &core_files::join(&dir, rel), *size, &progress)
                    .await
                    .map_err(|e| format!("{rel}: {e}"))?;
                done += size;
            }
            Ok(UploadSummary { files: tree.files.len(), folders: tree.dirs.len(), bytes: total })
        })
        .await
    }
}

async fn stop(e: &SessionEntry) {
    if e.info.pod.is_empty() {
        return;
    }
    let _ = tokio::time::timeout(STOP_TIMEOUT, kube_files::stop_helper(&e.client.client, &e.info.namespace, &e.info.pod)).await;
}

pub(crate) type Sessions = tokio::sync::Mutex<HashMap<u64, SessionEntry>>;
pub(crate) type Transfers = StdMutex<HashMap<String, watch::Sender<bool>>>;
