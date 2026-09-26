//! Write operations the UI can trigger. Each mirrors a kubectl command.

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::core::v1::{Event, Node, Pod};
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams};
use kube::Client;
use portside_core::summarize::ClusterObjects;
use portside_core::{EventInfo, DISABLED_REPLICAS_ANNOTATION};
use serde_json::json;

use crate::{KubeError, Result};

/// `kubectl delete pod` (the owning controller recreates it).
pub async fn delete_pod(client: &Client, namespace: &str, name: &str, force: bool) -> Result<()> {
    let api: Api<Pod> = Api::namespaced(client.clone(), namespace);
    let dp = if force {
        DeleteParams { grace_period_seconds: Some(0), ..Default::default() }
    } else {
        DeleteParams::default()
    };
    api.delete(name, &dp).await?;
    Ok(())
}

/// `kubectl rollout restart` — bumps the pod template's restartedAt annotation.
pub async fn rollout_restart(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<()> {
    let now = portside_core::jiff::Timestamp::from_millisecond(portside_core::now_ms())
        .map(|t| t.to_string())
        .unwrap_or_default();
    let patch = json!({
        "spec": { "template": { "metadata": { "annotations": {
            "kubectl.kubernetes.io/restartedAt": now
        }}}}
    });
    let pp = PatchParams::default();
    match kind {
        "Deployment" => {
            Api::<Deployment>::namespaced(client.clone(), namespace).patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        "StatefulSet" => {
            Api::<StatefulSet>::namespaced(client.clone(), namespace).patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        "DaemonSet" => {
            Api::<DaemonSet>::namespaced(client.clone(), namespace).patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        other => return Err(KubeError::Other(format!("can't rollout-restart a {other}"))),
    }
    Ok(())
}

/// `kubectl scale --replicas`.
pub async fn scale(client: &Client, kind: &str, namespace: &str, name: &str, replicas: i32) -> Result<()> {
    if replicas < 0 {
        return Err(KubeError::Other("replicas can't be negative".into()));
    }
    let patch = json!({ "spec": { "replicas": replicas } });
    let pp = PatchParams::default();
    match kind {
        "Deployment" => {
            Api::<Deployment>::namespaced(client.clone(), namespace).patch_scale(name, &pp, &Patch::Merge(&patch)).await?;
        }
        "StatefulSet" => {
            Api::<StatefulSet>::namespaced(client.clone(), namespace).patch_scale(name, &pp, &Patch::Merge(&patch)).await?;
        }
        other => return Err(KubeError::Other(format!("can't scale a {other}"))),
    }
    Ok(())
}

/// Scale a Deployment/StatefulSet to 0 while remembering its replica count
/// (`stopped = true`), or restore the remembered count (`stopped = false`).
/// The count lives in [`DISABLED_REPLICAS_ANNOTATION`] on the object itself;
/// replicas and annotation change in one merge patch. Returns the replica
/// count now set.
pub async fn set_disabled(client: &Client, kind: &str, namespace: &str, name: &str, disabled: bool) -> Result<i32> {
    let pp = PatchParams::default();

    // Current replicas + any remembered count, read from the live object.
    let (current, remembered) = match kind {
        "Deployment" => {
            let d = Api::<Deployment>::namespaced(client.clone(), namespace).get(name).await?;
            (d.spec.and_then(|s| s.replicas).unwrap_or(1), remembered_replicas(&d.metadata))
        }
        "StatefulSet" => {
            let s = Api::<StatefulSet>::namespaced(client.clone(), namespace).get(name).await?;
            (s.spec.and_then(|s| s.replicas).unwrap_or(1), remembered_replicas(&s.metadata))
        }
        other => return Err(KubeError::Other(format!("can't scale a {other} to zero (only Deployments and StatefulSets)"))),
    };

    let (replicas, annotation) = if disabled {
        if current == 0 {
            return Err(KubeError::Other(format!("{name} is already at 0 replicas")));
        }
        (0, json!(current.to_string()))
    } else {
        (remembered.unwrap_or(1).max(1), serde_json::Value::Null) // null removes the annotation
    };
    let patch = json!({
        "metadata": { "annotations": { DISABLED_REPLICAS_ANNOTATION: annotation } },
        "spec": { "replicas": replicas }
    });
    match kind {
        "Deployment" => {
            Api::<Deployment>::namespaced(client.clone(), namespace).patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
        _ => {
            Api::<StatefulSet>::namespaced(client.clone(), namespace).patch(name, &pp, &Patch::Merge(&patch)).await?;
        }
    }
    Ok(replicas)
}

fn remembered_replicas(meta: &kube::api::ObjectMeta) -> Option<i32> {
    meta.annotations.as_ref()?.get(DISABLED_REPLICAS_ANNOTATION)?.parse().ok()
}

/// `kubectl delete <kind> <name>`: removes the workload and (in the
/// background) the pods it owns.
pub async fn delete_workload(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<()> {
    if !matches!(kind, "Deployment" | "StatefulSet" | "DaemonSet" | "Job" | "CronJob") {
        return Err(KubeError::Other(format!("deleting a {kind} isn't supported here")));
    }
    let (api, _) = crate::manifests::api_for_kind(client, kind, Some(namespace)).await?;
    api.delete(name, &DeleteParams::background()).await?;
    Ok(())
}

/// `kubectl cordon` / `uncordon`.
pub async fn set_cordon(client: &Client, node: &str, cordoned: bool) -> Result<()> {
    let patch = json!({ "spec": { "unschedulable": cordoned } });
    Api::<Node>::all(client.clone())
        .patch(node, &PatchParams::default(), &Patch::Merge(&patch))
        .await?;
    Ok(())
}

/// Object manifest as YAML with managedFields stripped (like `kubectl get -o yaml`).
pub async fn get_yaml(client: &Client, kind: &str, namespace: Option<&str>, name: &str) -> Result<String> {
    let mut v = crate::manifests::get_value(client, kind, namespace, name).await?;
    if let Some(m) = v.get_mut("metadata").and_then(|m| m.as_object_mut()) {
        m.remove("managedFields");
    }
    serde_yaml::to_string(&v).map_err(|e| KubeError::Other(e.to_string()))
}

/// All events (normal + warning) for one object, newest first.
pub async fn events_for(client: &Client, kind: &str, namespace: Option<&str>, name: &str) -> Result<Vec<EventInfo>> {
    let api: Api<Event> = match namespace {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    let lp = ListParams::default().fields(&format!("involvedObject.kind={kind},involvedObject.name={name}"));
    let events = api.list(&lp).await?.items;
    let objs = ClusterObjects { events, ..Default::default() };
    let snap = portside_core::summarize::build_snapshot("", &objs, &Default::default(), 0);
    Ok(snap.events)
}
