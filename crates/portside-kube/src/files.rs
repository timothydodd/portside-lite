//! Volume file browsing on the cluster: start and stop the helper pod that
//! mounts a claim, and run file operations in it over `exec` (the same
//! websocket channel `kubectl cp` uses, so it works over the SSH tunnel too).
//! The scripts and parsing live in `portside_core::files`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod};
use kube::api::{Api, AttachParams, AttachedProcess, DeleteParams, ListParams, PostParams};
use kube::Client;
use portside_core::files::{self as core_files, ClaimUsers, FileListing};
use portside_core::summarize::ClusterObjects;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{KubeError, Result};

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// How long a new helper may take to start (image pull, volume attach).
const START_TIMEOUT: Duration = Duration::from_secs(120);
const CHUNK: usize = 64 * 1024;
/// Most stdout kept from a non-streaming command (a listing).
const MAX_CAPTURE: usize = 32 * 1024 * 1024;

/// Pods and workloads in the claim's namespace that use it, read live.
pub async fn claim_users(client: &Client, namespace: &str, claim: &str) -> Result<ClaimUsers> {
    let (pods, deployments, statefulsets, daemonsets, jobs, cronjobs) = futures::try_join!(
        list_in::<Pod>(client, namespace),
        list_in::<Deployment>(client, namespace),
        list_in::<StatefulSet>(client, namespace),
        list_in::<DaemonSet>(client, namespace),
        list_in::<Job>(client, namespace),
        list_in::<CronJob>(client, namespace),
    )?;
    let objs = ClusterObjects { pods, deployments, statefulsets, daemonsets, jobs, cronjobs, ..Default::default() };
    Ok(core_files::claim_users(namespace, claim, &objs))
}

async fn list_in<K>(client: &Client, namespace: &str) -> Result<Vec<K>>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope> + Clone + serde::de::DeserializeOwned + std::fmt::Debug,
    K::DynamicType: Default,
{
    Ok(Api::<K>::namespaced(client.clone(), namespace).list(&ListParams::default()).await?.items)
}

/// Start a helper pod mounting `claim` and wait until it runs. Finished
/// helpers left over for this claim are removed first.
pub async fn start_helper(
    client: &Client,
    namespace: &str,
    claim: &str,
    image: &str,
    node: Option<&str>,
    read_only: bool,
    owner: &str,
) -> Result<String> {
    let pvcs = Api::<PersistentVolumeClaim>::namespaced(client.clone(), namespace);
    if pvcs.get_opt(claim).await?.is_none() {
        return Err(other(format!("PersistentVolumeClaim {namespace}/{claim} doesn't exist.")));
    }
    let pods = Api::<Pod>::namespaced(client.clone(), namespace);
    let stale = pods.list(&ListParams::default().labels(&format!("{}=true", core_files::HELPER_LABEL))).await?;
    for p in stale.items.iter().filter(|p| core_files::helper_claim(p) == Some(claim)) {
        let phase = p.status.as_ref().and_then(|s| s.phase.as_deref());
        // Finished helpers, and live ones this install started earlier: the
        // caller has no session for the claim (or just stopped its helper), so
        // one of ours still running was orphaned by a crash or a kill.
        if matches!(phase, Some("Succeeded" | "Failed")) || core_files::helper_owner(p) == Some(owner) {
            if let Some(name) = &p.metadata.name {
                let _ = pods.delete(name, &DeleteParams::default().grace_period(0)).await;
            }
        }
    }

    let spec = core_files::helper_pod(namespace, claim, image, node, read_only, owner);
    let created = pods.create(&PostParams::default(), &spec).await.map_err(|e| match &e {
        kube::Error::Api(s) if s.code == 403 || s.code == 422 => other(format!(
            "The cluster refused the file-browser pod: {}. A Pod Security policy that forbids root pods would do this.",
            s.message
        )),
        _ => KubeError::from(e),
    })?;
    let name = created.metadata.name.clone().ok_or_else(|| other("helper pod has no name"))?;
    match wait_running(client, namespace, &name, image).await {
        Ok(()) => Ok(name),
        Err(e) => {
            let _ = stop_helper(client, namespace, &name).await;
            Err(e)
        }
    }
}

async fn wait_running(client: &Client, namespace: &str, name: &str, image: &str) -> Result<()> {
    let pods = Api::<Pod>::namespaced(client.clone(), namespace);
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        let p = pods.get(name).await?;
        let st = p.status.clone().unwrap_or_default();
        let running = st
            .container_statuses
            .as_ref()
            .and_then(|cs| cs.first())
            .is_some_and(|c| c.state.as_ref().is_some_and(|s| s.running.is_some()));
        if running {
            return Ok(());
        }
        if matches!(st.phase.as_deref(), Some("Succeeded" | "Failed")) {
            return Err(other(format!("The file-browser pod stopped before it was ready ({}).", st.reason.or(st.message).unwrap_or_default())));
        }
        let waiting = st
            .container_statuses
            .as_ref()
            .and_then(|cs| cs.first())
            .and_then(|c| c.state.as_ref()?.waiting.clone());
        if let Some(w) = waiting {
            let reason = w.reason.unwrap_or_default();
            if matches!(reason.as_str(), "ErrImagePull" | "ImagePullBackOff" | "InvalidImageName") {
                return Err(other(format!(
                    "Couldn't pull {image} for the file-browser pod ({reason}). Set another image under Settings → Volume files."
                )));
            }
            if reason == "CreateContainerConfigError" || reason == "CreateContainerError" {
                return Err(other(format!("The file-browser pod couldn't start: {}", w.message.unwrap_or(reason))));
            }
        }
        if tokio::time::Instant::now() >= deadline {
            let events = crate::actions::events_for(client, "Pod", Some(namespace), name).await.unwrap_or_default();
            let why = events
                .iter()
                .filter(|e| e.type_ == "Warning")
                .map(|e| format!("{}: {}", e.reason, e.message))
                .last()
                .or_else(|| {
                    st.conditions.as_ref()?.iter().find(|c| c.status == "False").and_then(|c| c.message.clone())
                })
                .unwrap_or_else(|| "no reason reported".into());
            return Err(other(format!("The file-browser pod didn't start within {}s: {why}", START_TIMEOUT.as_secs())));
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

pub async fn stop_helper(client: &Client, namespace: &str, pod: &str) -> Result<()> {
    match Api::<Pod>::namespaced(client.clone(), namespace).delete(pod, &DeleteParams::default().grace_period(0)).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(s)) if s.code == 404 => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Delete a claim. Helper pods for it go first (anyone's): the claim would
/// otherwise sit in Terminating until they exit. The caller checks that
/// nothing else uses it.
pub async fn delete_claim(client: &Client, namespace: &str, claim: &str) -> Result<()> {
    let pods = Api::<Pod>::namespaced(client.clone(), namespace);
    let helpers = pods.list(&ListParams::default().labels(&format!("{}=true", core_files::HELPER_LABEL))).await?;
    for p in helpers.items.iter().filter(|p| core_files::helper_claim(p) == Some(claim)) {
        if let Some(name) = &p.metadata.name {
            let _ = pods.delete(name, &DeleteParams::default().grace_period(0)).await;
        }
    }
    match Api::<PersistentVolumeClaim>::namespaced(client.clone(), namespace).delete(claim, &DeleteParams::default()).await {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(s)) if s.code == 404 => Err(other(format!("PersistentVolumeClaim {namespace}/{claim} is already gone."))),
        Err(e) => Err(e.into()),
    }
}

/// The helper is still there and running (it exits after its lifetime).
pub async fn helper_running(client: &Client, namespace: &str, pod: &str) -> Result<bool> {
    let p = Api::<Pod>::namespaced(client.clone(), namespace).get_opt(pod).await?;
    Ok(p.is_some_and(|p| p.metadata.deletion_timestamp.is_none() && p.status.and_then(|s| s.phase).as_deref() == Some("Running")))
}

// --- exec plumbing -------------------------------------------------------------

async fn exec(client: &Client, namespace: &str, pod: &str, command: Vec<String>, stdin: bool, stdout: bool) -> Result<AttachedProcess> {
    let ap = AttachParams {
        container: Some("files".into()),
        stdin,
        stdout,
        stderr: true,
        max_stdin_buf_size: Some(CHUNK),
        max_stdout_buf_size: Some(CHUNK),
        max_stderr_buf_size: Some(CHUNK),
        ..Default::default()
    };
    Api::<Pod>::namespaced(client.clone(), namespace).exec(pod, command, &ap).await.map_err(|e| match &e {
        kube::Error::Api(s) if s.code == 404 => other("The file-browser pod is gone (it stops after two hours). Reopen the volume."),
        _ => KubeError::from(e),
    })
}

/// Drain stderr in the background so a chatty command can't stall the stream.
fn collect_stderr(proc: &mut AttachedProcess) -> tokio::task::JoinHandle<String> {
    let reader = proc.stderr();
    tokio::spawn(async move {
        let Some(mut r) = reader else { return String::new() };
        let mut buf = Vec::new();
        let _ = (&mut r).take(64 * 1024).read_to_end(&mut buf).await;
        // Keep draining past the cap so the remote side never blocks.
        let _ = tokio::io::copy(&mut r, &mut tokio::io::sink()).await;
        String::from_utf8_lossy(&buf).trim().to_string()
    })
}

/// Wait for the command's exit status; a failure reports its stderr.
async fn finish(mut proc: AttachedProcess, stderr: tokio::task::JoinHandle<String>) -> Result<()> {
    let status = match proc.take_status() {
        Some(s) => s.await,
        None => None,
    };
    let err_text = stderr.await.unwrap_or_default();
    let _ = proc.join().await;
    match status {
        Some(s) if s.status.as_deref() == Some("Success") => Ok(()),
        Some(s) => Err(other(if err_text.is_empty() { s.message.unwrap_or_else(|| "command failed".into()) } else { err_text })),
        None => Err(other(if err_text.is_empty() { "The connection to the file-browser pod closed early.".to_string() } else { err_text })),
    }
}

/// Run a command and return its stdout.
pub async fn run(client: &Client, namespace: &str, pod: &str, command: Vec<String>) -> Result<Vec<u8>> {
    let mut proc = exec(client, namespace, pod, command, false, true).await?;
    let stderr = collect_stderr(&mut proc);
    let mut out = Vec::new();
    if let Some(mut r) = proc.stdout() {
        (&mut r).take(MAX_CAPTURE as u64).read_to_end(&mut out).await.map_err(|e| other(e.to_string()))?;
        let _ = tokio::io::copy(&mut r, &mut tokio::io::sink()).await;
    }
    finish(proc, stderr).await?;
    Ok(out)
}

/// Run a command and copy its stdout into `sink`, reporting bytes so far.
pub async fn run_to<W: AsyncWrite + Unpin>(
    client: &Client,
    namespace: &str,
    pod: &str,
    command: Vec<String>,
    sink: &mut W,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<u64> {
    let mut proc = exec(client, namespace, pod, command, false, true).await?;
    let stderr = collect_stderr(&mut proc);
    let mut total = 0u64;
    if let Some(mut r) = proc.stdout() {
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = r.read(&mut buf).await.map_err(|e| other(e.to_string()))?;
            if n == 0 {
                break;
            }
            sink.write_all(&buf[..n]).await.map_err(|e| other(format!("Couldn't write the local file: {e}")))?;
            total += n as u64;
            progress(total);
        }
    }
    sink.flush().await.map_err(|e| other(e.to_string()))?;
    finish(proc, stderr).await?;
    Ok(total)
}

/// Run a command feeding exactly `len` bytes of `source` to its stdin.
pub async fn run_from<R: AsyncRead + Unpin>(
    client: &Client,
    namespace: &str,
    pod: &str,
    command: Vec<String>,
    source: &mut R,
    len: u64,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<()> {
    let mut proc = exec(client, namespace, pod, command, true, false).await?;
    let stderr = collect_stderr(&mut proc);
    let mut stdin = proc.stdin().ok_or_else(|| other("no stdin"))?;
    let mut sent = 0u64;
    let mut buf = vec![0u8; CHUNK];
    while sent < len {
        let want = CHUNK.min((len - sent) as usize);
        let n = source.read(&mut buf[..want]).await.map_err(|e| other(format!("Couldn't read the local file: {e}")))?;
        if n == 0 {
            proc.abort();
            return Err(other("The local file got shorter while it was uploading."));
        }
        if let Err(e) = stdin.write_all(&buf[..n]).await {
            // The remote side quit early; its stderr says why.
            drop(stdin);
            return Err(finish(proc, stderr).await.err().unwrap_or_else(|| other(e.to_string())));
        }
        sent += n as u64;
        progress(sent);
    }
    stdin.flush().await.map_err(|e| other(e.to_string()))?;
    // `head -c` exits after `len` bytes, so the status arrives without
    // closing stdin (older API servers can't signal stdin EOF).
    let res = finish(proc, stderr).await;
    drop(stdin);
    res
}

// --- operations ------------------------------------------------------------------

pub async fn list(client: &Client, namespace: &str, pod: &str, rel: &str) -> Result<FileListing> {
    let out = run(client, namespace, pod, core_files::list_command(rel)).await.map_err(|e| match e {
        KubeError::Other(m) if m.contains("can't cd") => other(format!("\"{rel}\" isn't a folder (or no longer exists).")),
        e => e,
    })?;
    Ok(core_files::parse_listing(rel, &String::from_utf8_lossy(&out)))
}

/// Local files to upload: (local path, path relative to the upload root, size).
pub struct LocalTree {
    /// Folders to create, parents first, relative to the destination.
    pub dirs: Vec<String>,
    pub files: Vec<(PathBuf, String, u64)>,
}

/// Walk what the user picked: files go to the destination as-is; a folder
/// goes with its contents. Symlinks are skipped.
pub fn walk_local(sources: &[PathBuf]) -> std::io::Result<LocalTree> {
    let mut tree = LocalTree { dirs: Vec::new(), files: Vec::new() };
    fn name_of(p: &Path) -> std::io::Result<String> {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or_else(|| std::io::Error::other(format!("{} has no file name", p.display())))?;
        core_files::valid_name(&name).map_err(std::io::Error::other)?;
        Ok(name)
    }
    fn walk(dir: &Path, rel: &str, tree: &mut LocalTree) -> std::io::Result<()> {
        tree.dirs.push(rel.to_string());
        let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let ft = e.file_type()?;
            let path = e.path();
            let child = format!("{rel}/{}", name_of(&path)?);
            if ft.is_dir() {
                walk(&path, &child, tree)?;
            } else if ft.is_file() {
                tree.files.push((path, child, e.metadata()?.len()));
            }
        }
        Ok(())
    }
    for src in sources {
        let meta = std::fs::metadata(src)?;
        let name = name_of(src)?;
        if meta.is_dir() {
            walk(src, &name, &mut tree)?;
        } else {
            tree.files.push((src.clone(), name, meta.len()));
        }
    }
    Ok(tree)
}

pub async fn upload_file(
    client: &Client,
    namespace: &str,
    pod: &str,
    local: &Path,
    dest_rel: &str,
    size: u64,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<()> {
    let mut f = tokio::fs::File::open(local).await.map_err(|e| other(format!("Couldn't open {}: {e}", local.display())))?;
    run_from(client, namespace, pod, core_files::put_command(dest_rel, size), &mut f, size, progress).await
}

/// Download a file, or a folder as a .tar.gz, to `dest`. A failed or
/// cancelled download leaves nothing behind (the caller removes `dest` if the
/// future is dropped).
pub async fn download(
    client: &Client,
    namespace: &str,
    pod: &str,
    rel: &str,
    folder: bool,
    dest: &Path,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<u64> {
    // Into a sibling first: a failed or cancelled download must not cost the
    // user a file that was already at `dest`.
    let part = part_path(dest);
    let mut f = tokio::fs::File::create(&part).await.map_err(|e| other(format!("Couldn't create {}: {e}", part.display())))?;
    let cmd = if folder { core_files::tar_command(rel) } else { core_files::cat_command(rel) };
    let mut res = run_to(client, namespace, pod, cmd, &mut f, progress).await;
    drop(f);
    if res.is_ok() {
        if let Err(e) = tokio::fs::rename(&part, dest).await {
            res = Err(other(format!("Couldn't save {}: {e}", dest.display())));
        }
    }
    if res.is_err() {
        let _ = tokio::fs::remove_file(&part).await;
    }
    res
}

/// Where a download is written until it's complete (then renamed to `dest`).
pub fn part_path(dest: &Path) -> std::path::PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".portside-part");
    dest.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_file_sits_next_to_the_destination() {
        assert_eq!(part_path(Path::new("/tmp/out/app.db")), Path::new("/tmp/out/app.db.portside-part"));
        assert_eq!(part_path(Path::new("site.tar.gz")), Path::new("site.tar.gz.portside-part"));
    }

    #[test]
    fn walks_folders_parents_first() {
        let root = std::env::temp_dir().join(format!("portside-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("site/css")).unwrap();
        std::fs::write(root.join("site/index.html"), "hi").unwrap();
        std::fs::write(root.join("site/css/a.css"), "body{}").unwrap();
        std::fs::write(root.join("notes.txt"), "1234").unwrap();
        let t = walk_local(&[root.join("site"), root.join("notes.txt")]).unwrap();
        assert_eq!(t.dirs, ["site", "site/css"]);
        let files: Vec<(&str, u64)> = t.files.iter().map(|(_, r, s)| (r.as_str(), *s)).collect();
        assert_eq!(files, [("site/css/a.css", 6), ("site/index.html", 2), ("notes.txt", 4)]);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
