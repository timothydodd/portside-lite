//! One poll of cluster state + live usage.

use std::collections::HashMap;

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{Event, Node, PersistentVolumeClaim, Pod};
use kube::api::{Api, ListParams, ObjectList};
use kube::Client;
use portside_core::quantity::{parse_cpu, parse_memory};
use portside_core::summarize::{ClusterObjects, UsageMetrics};
use serde::Deserialize;

use crate::Result;

async fn list<K>(client: &Client, lp: &ListParams) -> Result<Vec<K>>
where
    K: kube::Resource<Scope = k8s_openapi::NamespaceResourceScope>
        + Clone
        + serde::de::DeserializeOwned
        + std::fmt::Debug,
    K::DynamicType: Default,
{
    let list: ObjectList<K> = Api::<K>::all(client.clone()).list(lp).await?;
    Ok(list.items)
}

pub async fn fetch_objects(client: &Client) -> Result<ClusterObjects> {
    let all = ListParams::default();
    let warnings = ListParams::default().fields("type=Warning");
    let (version, nodes, pods, deployments, statefulsets, daemonsets, jobs, cronjobs, pvcs, events) = futures::try_join!(
        async { Ok::<_, crate::KubeError>(client.apiserver_version().await.ok().map(|v| v.git_version)) },
        async { Ok(Api::<Node>::all(client.clone()).list(&all).await?.items) },
        list::<Pod>(client, &all),
        list::<Deployment>(client, &all),
        list::<StatefulSet>(client, &all),
        list::<DaemonSet>(client, &all),
        list::<Job>(client, &all),
        list::<CronJob>(client, &all),
        list::<PersistentVolumeClaim>(client, &all),
        list::<Event>(client, &warnings),
    )?;
    Ok(ClusterObjects {
        server_version: version,
        nodes,
        pods,
        deployments,
        statefulsets,
        daemonsets,
        jobs,
        cronjobs,
        pvcs,
        events,
    })
}

#[derive(Deserialize)]
struct MetricsList<T> {
    items: Vec<T>,
}

#[derive(Deserialize)]
struct NodeMetrics {
    metadata: MetaName,
    usage: HashMap<String, String>,
}

#[derive(Deserialize)]
struct PodMetrics {
    metadata: MetaName,
    containers: Vec<ContainerMetrics>,
}

#[derive(Deserialize)]
struct ContainerMetrics {
    usage: HashMap<String, String>,
}

#[derive(Deserialize)]
struct MetaName {
    name: String,
    #[serde(default)]
    namespace: Option<String>,
}

async fn get_json<T: serde::de::DeserializeOwned>(client: &Client, path: &str) -> Result<T> {
    let req = http::Request::get(path)
        .body(Vec::new())
        .map_err(|e| crate::KubeError::Other(e.to_string()))?;
    Ok(client.request::<T>(req).await?)
}

/// Usage from metrics.k8s.io. k3s ships metrics-server by default; if it's
/// missing or unhealthy this returns `available: false` rather than failing
/// the whole poll.
pub async fn fetch_usage(client: &Client) -> UsageMetrics {
    let nodes = get_json::<MetricsList<NodeMetrics>>(client, "/apis/metrics.k8s.io/v1beta1/nodes");
    let pods = get_json::<MetricsList<PodMetrics>>(client, "/apis/metrics.k8s.io/v1beta1/pods");
    let (nodes, pods) = futures::join!(nodes, pods);
    let (Ok(nodes), Ok(pods)) = (nodes, pods) else {
        return UsageMetrics::default();
    };
    let usage = |m: &HashMap<String, String>| {
        (
            m.get("cpu").map(|s| parse_cpu(s)).unwrap_or(0.0),
            m.get("memory").map(|s| parse_memory(s)).unwrap_or(0.0),
        )
    };
    UsageMetrics {
        available: true,
        nodes: nodes
            .items
            .iter()
            .map(|n| (n.metadata.name.clone(), usage(&n.usage)))
            .collect(),
        pods: pods
            .items
            .iter()
            .map(|p| {
                let (cpu, mem) = p.containers.iter().map(|c| usage(&c.usage)).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
                (
                    format!("{}/{}", p.metadata.namespace.as_deref().unwrap_or_default(), p.metadata.name),
                    (cpu, mem),
                )
            })
            .collect(),
    }
}
