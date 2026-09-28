//! Cluster side of archiving: work out what an archive should hold and what
//! is safe to remove, then remove it.

use kube::api::DeleteParams;
use kube::Client;
use portside_core::archive::{plan_items, ArchivePlanItem};
use portside_core::manifest::{apply_order, ObjectRef};

use crate::manifests::{api_for_kind, get_value, list_values, related, CopyResult};
use crate::Result;

const WORKLOAD_KINDS: [&str; 5] = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];

/// Related objects of a workload, each with the other workloads that still
/// use it, so the archive dialog only offers to remove what nothing else needs.
pub async fn plan(client: &Client, kind: &str, namespace: &str, name: &str) -> Result<Vec<ArchivePlanItem>> {
    let target = get_value(client, kind, Some(namespace), name).await?;
    let rel = related(client, kind, namespace, name).await?;
    let (services, ingresses) =
        futures::join!(list_values(client, "Service", namespace), list_values(client, "Ingress", namespace));
    let mut workloads = Vec::new();
    for k in WORKLOAD_KINDS {
        workloads.extend(list_values(client, k, namespace).await);
    }
    Ok(plan_items(&target, rel, &workloads, &services, &ingresses))
}

/// Delete objects from one namespace, users before what they use (Ingresses
/// and autoscalers, then the workload, then Services, config and accounts).
/// Something already gone counts as removed. Every item gets a result.
pub async fn remove_objects(client: &Client, namespace: &str, items: &[ObjectRef]) -> Vec<CopyResult> {
    let mut ordered: Vec<&ObjectRef> = items.iter().collect();
    ordered.sort_by_key(|r| std::cmp::Reverse(apply_order(&r.kind)));
    let mut out = Vec::with_capacity(items.len());
    for item in ordered {
        let result = async {
            let (api, _) = api_for_kind(client, &item.kind, Some(namespace)).await?;
            match api.delete(&item.name, &DeleteParams::background()).await {
                Ok(_) => Ok("removed"),
                Err(kube::Error::Api(s)) if s.code == 404 => Ok("already gone"),
                Err(e) => Err(crate::KubeError::from(e)),
            }
        }
        .await;
        out.push(match result {
            Ok(outcome) => CopyResult { kind: item.kind.clone(), name: item.name.clone(), outcome: outcome.into(), message: None },
            Err(e) => CopyResult { kind: item.kind.clone(), name: item.name.clone(), outcome: "error".into(), message: Some(e.to_string()) },
        });
    }
    out
}
