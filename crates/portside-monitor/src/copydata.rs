//! Copy a workload to another cluster, volume data included. The objects go
//! in two phases around the data: everything the workload depends on first
//! (claims among them), then each chosen claim's files, then the workload
//! itself, so nothing on the target has the claims mounted while they're
//! written. The files travel through this PC: a tar stream from a read-only
//! helper on the source into a temporary file, then into a read-write
//! helper on the target.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use portside_core::manifest::ObjectRef;
use portside_kube::copy::{self, CopyOutcome, CopyPlan, WORKLOAD_KINDS};
use portside_kube::files as kube_files;
use portside_kube::manifests::CopyResult;
use portside_kube::ClusterClient;
use serde::Deserialize;

use crate::files::throttled;
use crate::Monitor;

/// The `kind` of a result row for a claim's files.
pub const VOLUME_FILES: &str = "Volume files";
/// How long the target's workloads (scaled to 0 for the copy) get to let go
/// of a claim.
const RELEASE_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyRequest {
    pub kind: String,
    pub namespace: String,
    pub name: String,
    pub extras: Vec<ObjectRef>,
    pub target_connection_id: String,
    pub target_namespace: String,
    /// Claims (among `extras`) whose files go too.
    #[serde(default)]
    pub data_claims: Vec<String>,
    /// For progress events and cancelling.
    #[serde(default)]
    pub transfer_id: String,
    pub dry_run: bool,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn row(name: &str, outcome: &str, message: Option<String>) -> CopyResult {
    CopyResult { kind: VOLUME_FILES.into(), name: name.into(), outcome: outcome.into(), message }
}

fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", UNITS[i]) }
}

impl Monitor {
    /// Copy a workload (plus chosen related objects, plus the files of
    /// chosen claims) from the active cluster to any saved connection.
    pub async fn copy_workload(self: &Arc<Self>, req: CopyRequest) -> Result<CopyOutcome, String> {
        let target_namespace = req.target_namespace.trim().to_string();
        if target_namespace.is_empty() {
            return Err("Pick a target namespace.".into());
        }
        let src = self.client().await?;
        let dst = self.connect_profile(&req.target_connection_id).await?;
        let workload = ObjectRef { kind: req.kind.clone(), name: req.name.clone() };
        let plan = copy::prepare(&src.client, &dst.client, &req.namespace, &workload, &req.extras, &target_namespace).await;
        let is_workload = |kind: &str| WORKLOAD_KINDS.contains(&kind);

        let mut results = plan.unreadable.clone();
        results.extend(copy::apply(&dst.client, &plan, &req.namespace, &target_namespace, req.dry_run, |k| !is_workload(k)).await.map_err(err)?);

        let mut warnings = plan.warnings.clone();
        for claim in &req.data_claims {
            if !plan.contains("PersistentVolumeClaim", claim) {
                results.push(row(claim, "error", Some("the claim itself isn't being copied, so its files can't be".into())));
                continue;
            }
            if req.dry_run {
                match self.data_blockers(&dst, &target_namespace, claim, &plan).await {
                    Ok(blockers) if !blockers.is_empty() => warnings.push(format!(
                        "The files of {claim} can't be written on the target while something else uses the claim there: {}",
                        blockers.join(" ")
                    )),
                    Ok(_) => {}
                    Err(e) => warnings.push(format!("Couldn't check who uses {claim} on the target: {e}")),
                }
                results.push(row(claim, "copied", Some("through this PC; the target's claim is written while nothing there uses it".into())));
                continue;
            }
            // A claim that wasn't created can't take files.
            let claim_ok = results.iter().any(|r| r.kind == "PersistentVolumeClaim" && &r.name == claim && r.outcome != "error");
            if !claim_ok {
                results.push(row(claim, "error", Some("the claim wasn't created on the target".into())));
                continue;
            }
            let outcome = self.copy_volume_data(&dst, &req.namespace, &target_namespace, claim, &plan, &req.transfer_id).await;
            results.push(match outcome {
                Ok(bytes) => row(claim, "copied", Some(format!("{} transferred", fmt_bytes(bytes)))),
                Err(e) => row(claim, "error", Some(e)),
            });
        }

        results.extend(copy::apply(&dst.client, &plan, &req.namespace, &target_namespace, req.dry_run, is_workload).await.map_err(err)?);
        if let Some(r) = copy::attach_pull_secrets(&src.client, &dst.client, &req.namespace, &target_namespace, &plan, &results, req.dry_run).await {
            results.push(r);
        }
        if !req.dry_run {
            self.refresh_now();
        }
        Ok(CopyOutcome { results, warnings })
    }

    /// Who on the target would keep the claim's files from being written:
    /// everything that uses it, except Deployments and StatefulSets in the
    /// copy (those are scaled to 0 for the copy and come back with the
    /// workload phase).
    async fn data_blockers(&self, dst: &ClusterClient, target_namespace: &str, claim: &str, plan: &CopyPlan) -> Result<Vec<String>, String> {
        let users = kube_files::claim_users(&dst.client, target_namespace, claim).await.map_err(err)?;
        let ours = |blocker: &str| {
            plan.fetched.iter().any(|f| {
                matches!(f.item.kind.as_str(), "Deployment" | "StatefulSet") && blocker.starts_with(&format!("{} {} ", f.item.kind, f.item.name))
            })
        };
        Ok(users.blockers.into_iter().filter(|b| !ours(b)).collect())
    }

    /// Scale our own workloads on the target to 0, wait for the claim to be
    /// free, then stream the claim's files from the source to the target.
    async fn copy_volume_data(
        self: &Arc<Self>,
        dst: &Arc<ClusterClient>,
        source_namespace: &str,
        target_namespace: &str,
        claim: &str,
        plan: &CopyPlan,
        transfer_id: &str,
    ) -> Result<u64, String> {
        // 1. Nothing on the target may have it mounted. Our own workloads
        //    there (a copy done earlier) go to 0; the workload phase brings
        //    them back at the source's replica count.
        let users = kube_files::claim_users(&dst.client, target_namespace, claim).await.map_err(err)?;
        let mut scaled = Vec::new();
        for w in &users.workloads {
            let Some((kind, name)) = w.split_once('/') else { continue };
            if matches!(kind, "Deployment" | "StatefulSet") && plan.contains(kind, name) {
                portside_kube::actions::scale(&dst.client, kind, target_namespace, name, 0)
                    .await
                    .map_err(|e| format!("Couldn't scale {w} on the target to 0 so the files can be written: {e}"))?;
                scaled.push(w.clone());
            }
        }
        let deadline = tokio::time::Instant::now() + RELEASE_TIMEOUT;
        let blockers = loop {
            let users = kube_files::claim_users(&dst.client, target_namespace, claim).await.map_err(err)?;
            if users.blockers.is_empty() {
                break Vec::new();
            }
            let only_ours = users.blockers.iter().all(|b| scaled.iter().any(|w| b.starts_with(&w.replace('/', " ")) || b.starts_with("Pod ")));
            if !only_ours || tokio::time::Instant::now() >= deadline {
                break users.blockers;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        };
        if !blockers.is_empty() {
            return Err(format!("Something on the target still uses {claim}, so its files weren't written. {}", blockers.join(" ")));
        }

        // 2. Source → this PC.
        let emit = self.progress_fn(transfer_id);
        let tmp = std::env::temp_dir().join(format!("portside-copy-{}-{}.tar.gz", std::process::id(), portside_core::now_ms()));
        let session = self.open_files(source_namespace, claim).await?;
        let download = {
            let label = format!("{claim}: reading from source");
            let progress = throttled(|n| emit(&label, n, None));
            let (client, info) = self.session(session.id).await?;
            self.cancellable(transfer_id, async {
                kube_files::download(&client.client, &info.namespace, &info.pod, "", true, &tmp, &progress).await.map_err(err)
            })
            .await
        };
        self.close_files(session.id).await;
        let size = match download {
            Ok(n) => n,
            Err(e) => {
                let _ = tokio::fs::remove_file(kube_files::part_path(&tmp)).await;
                return Err(format!("Reading the files from the source failed: {e}"));
            }
        };

        // 3. This PC → target, through a read-write helper of our own.
        let result = self.write_volume_data(dst, target_namespace, claim, &tmp, size, transfer_id).await;
        let _ = tokio::fs::remove_file(&tmp).await;
        result.map(|()| size)
    }

    async fn write_volume_data(&self, dst: &Arc<ClusterClient>, namespace: &str, claim: &str, archive: &PathBuf, size: u64, transfer_id: &str) -> Result<(), String> {
        let client = &dst.client;
        let image = self.settings().files_helper_image;
        // Its own owner, so it never takes an open file browser's helper for an orphan.
        let owner = format!("{}-copy", self.install_id);
        let pod = kube_files::start_helper(client, namespace, claim, &image, None, false, &owner)
            .await
            .map_err(|e| format!("Couldn't start a helper pod on the target to write the files: {e}"))?;
        let emit = self.progress_fn(transfer_id);
        let label = format!("{claim}: writing to target");
        let progress = throttled(|n| emit(&label, n, Some(size)));
        progress(0);
        let result = self
            .cancellable(transfer_id, async {
                // Re-check right before writing: the rule for every write.
                let users = kube_files::claim_users(client, namespace, claim).await.map_err(err)?;
                if !users.blockers.is_empty() {
                    return Err(format!("Something started using {claim} on the target. {}", users.blockers.join(" ")));
                }
                kube_files::extract_archive(client, namespace, &pod, archive, "", size, &progress).await.map_err(err)
            })
            .await;
        let _ = kube_files::stop_helper(client, namespace, &pod).await;
        result.map_err(|e| format!("Writing the files on the target failed: {e}"))
    }
}
