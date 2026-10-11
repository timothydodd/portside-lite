//! Volume backups on the cluster: install or change one, run it now, pause it
//! and remove it. The objects themselves are built in `portside_core::backup`.

use std::time::Duration;

use k8s_openapi::api::apps::v1::{Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{PersistentVolumeClaim, Pod, ServiceAccount};
use k8s_openapi::api::rbac::v1::{Role, RoleBinding};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams, PostParams, PropagationPolicy};
use kube::Client;
use portside_core::backup::{self, BackupSpec, BACKUP_LABEL};

use crate::{KubeError, Result};

const FIELD_MANAGER: &str = "portside-lite";

fn other(msg: impl Into<String>) -> KubeError {
    KubeError::Other(msg.into())
}

/// Create or change a backup: the backup claim if it doesn't exist (an
/// existing one keeps its size), the access an offline backup needs, then the
/// CronJob. Not while a run is going: it may have the app stopped. Returns
/// what was done.
pub async fn install(client: &Client, spec: &BackupSpec, dry_run: bool) -> Result<Vec<String>> {
    backup::validate(spec).map_err(other)?;
    let ns = spec.namespace.as_str();
    let object = backup::object_name(&spec.name);
    let mut done = Vec::new();

    let crons: Api<CronJob> = Api::namespaced(client.clone(), ns);
    let existing = crons.get_opt(&object).await?;
    if existing.as_ref().is_some_and(|c| !backup::is_backup(&c.metadata)) {
        return Err(other(format!("A CronJob named {object} already exists in {ns} and isn't a Portside backup. Pick another name.")));
    }
    if existing.as_ref().and_then(|c| c.status.as_ref()?.active.as_ref()).is_some_and(|a| !a.is_empty()) {
        return Err(other(format!("Backup {} is running. Change it once the run has finished.", spec.name)));
    }

    let claims: Api<PersistentVolumeClaim> = Api::namespaced(client.clone(), ns);
    if claims.get_opt(&spec.claim).await?.is_none() {
        return Err(other(format!("PersistentVolumeClaim {ns}/{} doesn't exist.", spec.claim)));
    }
    if spec.offline {
        for w in &spec.workloads {
            let (kind, name) = w.split_once('/').unwrap_or_default();
            let found = match kind {
                "Deployment" => Api::<Deployment>::namespaced(client.clone(), ns).get_opt(name).await?.is_some(),
                _ => Api::<StatefulSet>::namespaced(client.clone(), ns).get_opt(name).await?.is_some(),
            };
            if !found {
                return Err(other(format!("{kind} {name} doesn't exist in {ns} any more. Pick the workloads to stop again.")));
            }
        }
    }

    match claims.get_opt(&object).await? {
        Some(c) if !backup::is_backup(&c.metadata) => {
            return Err(other(format!("A PersistentVolumeClaim named {object} already exists in {ns} and isn't a Portside backup. Pick another name.")));
        }
        Some(c) => {
            let size = c.status.and_then(|s| s.capacity).and_then(|c| c.get("storage").map(|q| q.0.clone()));
            done.push(format!("kept the existing backup storage ({})", size.as_deref().unwrap_or("size unknown")));
        }
        None => {
            claims
                .create(&PostParams { dry_run, field_manager: Some(FIELD_MANAGER.into()) }, &backup::backup_claim(spec))
                .await?;
            done.push(format!("created {} of backup storage ({object})", spec.claim_size));
        }
    }

    let mut pp = PatchParams::apply(FIELD_MANAGER).force();
    pp.dry_run = dry_run;
    // The account that stops and starts the workloads: only while offline,
    // and its Role names exactly the workloads in the spec.
    if spec.offline {
        Api::<ServiceAccount>::namespaced(client.clone(), ns).patch(&object, &pp, &Patch::Apply(&backup::service_account(spec))).await?;
        Api::<Role>::namespaced(client.clone(), ns).patch(&object, &pp, &Patch::Apply(&backup::role(spec))).await?;
        Api::<RoleBinding>::namespaced(client.clone(), ns).patch(&object, &pp, &Patch::Apply(&backup::role_binding(spec))).await?;
        done.push(format!("allowed it to stop and start {}", spec.workloads.join(", ")));
    } else if !dry_run {
        delete_access(client, ns, &object).await?;
    }
    crons.patch(&object, &pp, &Patch::Apply(&backup::backup_cronjob(spec))).await?;
    done.push(format!("{} CronJob {object}", if existing.is_some() { "updated" } else { "created" }));
    if dry_run {
        done.push("dry run, nothing changed".into());
    }
    Ok(done)
}

fn cron_api(client: &Client, namespace: &str) -> Api<CronJob> {
    Api::namespaced(client.clone(), namespace)
}

async fn get_backup(client: &Client, namespace: &str, name: &str) -> Result<CronJob> {
    let object = backup::object_name(name);
    let cron = cron_api(client, namespace)
        .get_opt(&object)
        .await?
        .ok_or_else(|| other(format!("Backup {namespace}/{name} doesn't exist any more.")))?;
    if !backup::is_backup(&cron.metadata) {
        return Err(other(format!("CronJob {object} isn't a Portside backup.")));
    }
    Ok(cron)
}

/// Start a run now, the way `kubectl create job --from=cronjob/…` does. The
/// Job is owned by the CronJob, so it shows in the backup's history.
pub async fn run_now(client: &Client, namespace: &str, name: &str) -> Result<String> {
    let cron = get_backup(client, namespace, name).await?;
    if cron.status.as_ref().and_then(|s| s.active.as_ref()).is_some_and(|a| !a.is_empty()) {
        return Err(other(format!("Backup {name} is already running.")));
    }
    let cron_name = cron.metadata.name.clone().unwrap_or_default();
    let template = cron.spec.job_template.clone();
    let secs = portside_core::now_ms() / 1000;
    let mut meta = template.metadata.unwrap_or_default();
    meta.name = Some(format!("{cron_name}-{secs}"));
    meta.namespace = Some(namespace.into());
    meta.annotations.get_or_insert_with(Default::default).insert("cronjob.kubernetes.io/instantiate".into(), "manual".into());
    meta.owner_references = Some(vec![OwnerReference {
        api_version: "batch/v1".into(),
        kind: "CronJob".into(),
        name: cron_name,
        uid: cron.metadata.uid.clone().unwrap_or_default(),
        controller: Some(true),
        block_owner_deletion: Some(true),
    }]);
    let job = Job { metadata: meta, spec: template.spec, status: None };
    let created = Api::<Job>::namespaced(client.clone(), namespace)
        .create(&PostParams { dry_run: false, field_manager: Some(FIELD_MANAGER.into()) }, &job)
        .await?;
    Ok(created.metadata.name.unwrap_or_default())
}

/// Pause (suspend) or resume the schedule. Runs already going finish.
pub async fn set_paused(client: &Client, namespace: &str, name: &str, paused: bool) -> Result<()> {
    get_backup(client, namespace, name).await?;
    let pp = PatchParams { field_manager: Some(FIELD_MANAGER.into()), ..Default::default() };
    cron_api(client, namespace)
        .patch(&backup::object_name(name), &pp, &Patch::Merge(serde_json::json!({ "spec": { "suspend": paused } })))
        .await?;
    Ok(())
}

/// The ServiceAccount, Role and RoleBinding an offline backup uses. Gone already is fine.
async fn delete_access(client: &Client, namespace: &str, object: &str) -> Result<()> {
    let dp = DeleteParams::default();
    let gone = |r: std::result::Result<_, kube::Error>| match r {
        Ok(_) => Ok(()),
        Err(kube::Error::Api(s)) if s.code == 404 => Ok(()),
        Err(e) => Err(KubeError::from(e)),
    };
    gone(Api::<RoleBinding>::namespaced(client.clone(), namespace).delete(object, &dp).await.map(|_| ()))?;
    gone(Api::<Role>::namespaced(client.clone(), namespace).delete(object, &dp).await.map(|_| ()))?;
    gone(Api::<ServiceAccount>::namespaced(client.clone(), namespace).delete(object, &dp).await.map(|_| ()))
}

/// Delete the CronJob, its runs and its access. The claim (and the archives
/// on it) stays; the caller deletes it separately if asked. A run in progress
/// gets TERM and starts its workloads again, so the access goes only once
/// its pods are gone (waits up to 60 s).
pub async fn remove(client: &Client, namespace: &str, name: &str) -> Result<()> {
    get_backup(client, namespace, name).await?;
    let dp = DeleteParams { propagation_policy: Some(PropagationPolicy::Background), ..Default::default() };
    cron_api(client, namespace).delete(&backup::object_name(name), &dp).await?;
    let selector = format!("{BACKUP_LABEL}={name}");
    let jobs: Api<Job> = Api::namespaced(client.clone(), namespace);
    let _ = jobs.delete_collection(&dp, &ListParams::default().labels(&selector)).await;
    let pods: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let _ = pods.delete_collection(&DeleteParams::default(), &ListParams::default().labels(&selector)).await;
    for _ in 0..60 {
        if pods.list(&ListParams::default().labels(&selector)).await?.items.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    delete_access(client, namespace, &backup::object_name(name)).await
}

/// Every backup on the cluster whose storage is bound (so there's something
/// to copy), as (namespace, backup name).
pub async fn list_with_storage(client: &Client) -> Result<Vec<(String, String)>> {
    let lp = ListParams::default().labels(BACKUP_LABEL);
    let crons = Api::<CronJob>::all(client.clone()).list(&lp).await?.items;
    let claims = Api::<PersistentVolumeClaim>::all(client.clone()).list(&lp).await?.items;
    let bound = |ns: &str, object: &str| {
        claims.iter().any(|c| {
            c.metadata.namespace.as_deref() == Some(ns)
                && c.metadata.name.as_deref() == Some(object)
                && c.status.as_ref().and_then(|s| s.phase.as_deref()) == Some("Bound")
        })
    };
    let mut out: Vec<(String, String)> = crons
        .iter()
        .filter_map(|c| {
            let ns = c.metadata.namespace.clone()?;
            let object = c.metadata.name.as_deref()?;
            let name = backup::backup_name(object)?;
            bound(&ns, object).then(|| (ns, name.to_string()))
        })
        .collect();
    out.sort();
    Ok(out)
}
