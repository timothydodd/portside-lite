//! Flatten raw Kubernetes objects into the UI snapshot DTOs.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ContainerStatus, Event, Node, PersistentVolumeClaim, Pod, PodSpec,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, Time};

use crate::models::*;
use crate::quantity::{parse_cpu, parse_memory};

/// Everything fetched from the API server in one poll.
#[derive(Debug, Default, Clone)]
pub struct ClusterObjects {
    pub server_version: Option<String>,
    pub nodes: Vec<Node>,
    pub pods: Vec<Pod>,
    pub deployments: Vec<Deployment>,
    pub statefulsets: Vec<StatefulSet>,
    pub daemonsets: Vec<DaemonSet>,
    pub jobs: Vec<Job>,
    pub cronjobs: Vec<CronJob>,
    pub pvcs: Vec<PersistentVolumeClaim>,
    /// Warning events only.
    pub events: Vec<Event>,
}

/// Live usage from metrics.k8s.io, in cores and bytes.
#[derive(Debug, Default, Clone)]
pub struct UsageMetrics {
    pub available: bool,
    pub nodes: HashMap<String, (f64, f64)>,
    /// Keyed by `namespace/name`.
    pub pods: HashMap<String, (f64, f64)>,
}

/// Cap on events kept in a snapshot.
const MAX_EVENTS: usize = 300;

pub fn build_snapshot(
    cluster_id: &str,
    objs: &ClusterObjects,
    usage: &UsageMetrics,
    now_ms: i64,
) -> ClusterSnapshot {
    let pods: Vec<PodInfo> = objs.pods.iter().map(|p| pod_info(p, usage)).collect();

    let mut per_node: HashMap<&str, (usize, f64, f64)> = HashMap::new();
    for p in &pods {
        if p.phase == "Succeeded" || p.phase == "Failed" {
            continue;
        }
        if let Some(n) = &p.node {
            let e = per_node.entry(n.as_str()).or_default();
            e.0 += 1;
            e.1 += p.cpu_requests;
            e.2 += p.mem_requests;
        }
    }

    let nodes: Vec<NodeInfo> = objs
        .nodes
        .iter()
        .map(|n| {
            let mut info = node_info(n, usage);
            if let Some((count, cpu, mem)) = per_node.get(info.name.as_str()) {
                info.pod_count = *count;
                info.cpu_requests = *cpu;
                info.mem_requests = *mem;
            }
            info
        })
        .collect();

    let mut workloads = Vec::new();
    workloads.extend(objs.deployments.iter().map(deployment_info));
    workloads.extend(objs.statefulsets.iter().map(statefulset_info));
    workloads.extend(objs.daemonsets.iter().map(daemonset_info));
    workloads.extend(objs.jobs.iter().map(job_info));
    workloads.extend(objs.cronjobs.iter().map(cronjob_info));

    let volumes: Vec<VolumeClaimInfo> = objs.pvcs.iter().map(pvc_info).collect();

    let mut events: Vec<EventInfo> = objs.events.iter().map(event_info).collect();
    events.sort_by(|a, b| b.last_ms.cmp(&a.last_ms));
    events.truncate(MAX_EVENTS);

    let mut namespaces = BTreeSet::new();
    for p in &pods {
        namespaces.insert(p.namespace.clone());
    }
    for w in &workloads {
        namespaces.insert(w.namespace.clone());
    }

    let totals = ClusterTotals {
        nodes: nodes.len(),
        nodes_ready: nodes.iter().filter(|n| n.ready).count(),
        pods: pods.len(),
        pods_running: pods.iter().filter(|p| p.phase == "Running").count(),
        pods_pending: pods.iter().filter(|p| p.phase == "Pending").count(),
        pods_failed: pods.iter().filter(|p| p.phase == "Failed").count(),
        cpu_allocatable: nodes.iter().map(|n| n.cpu_allocatable).sum(),
        mem_allocatable: nodes.iter().map(|n| n.mem_allocatable).sum(),
        cpu_usage: usage
            .available
            .then(|| nodes.iter().filter_map(|n| n.cpu_usage).sum()),
        mem_usage: usage
            .available
            .then(|| nodes.iter().filter_map(|n| n.mem_usage).sum()),
        cpu_requests: nodes.iter().map(|n| n.cpu_requests).sum(),
        mem_requests: nodes.iter().map(|n| n.mem_requests).sum(),
    };

    ClusterSnapshot {
        collected_at_ms: now_ms,
        cluster_id: cluster_id.to_string(),
        server_version: objs.server_version.clone(),
        metrics_available: usage.available,
        totals,
        nodes,
        pods,
        workloads,
        volumes,
        events,
        issues: Vec::new(),
        namespaces: namespaces.into_iter().collect(),
    }
}

fn ms(t: &Option<Time>) -> Option<i64> {
    t.as_ref().map(|t| t.0.as_millisecond())
}

fn ns_of(meta: &ObjectMeta) -> String {
    meta.namespace.clone().unwrap_or_default()
}

fn name_of(meta: &ObjectMeta) -> String {
    meta.name.clone().unwrap_or_default()
}

fn q_cpu(map: &Option<BTreeMap<String, Quantity>>, key: &str) -> f64 {
    map.as_ref()
        .and_then(|m| m.get(key))
        .map(|q| parse_cpu(&q.0))
        .unwrap_or(0.0)
}

fn q_mem(map: &Option<BTreeMap<String, Quantity>>, key: &str) -> f64 {
    map.as_ref()
        .and_then(|m| m.get(key))
        .map(|q| parse_memory(&q.0))
        .unwrap_or(0.0)
}

fn node_info(n: &Node, usage: &UsageMetrics) -> NodeInfo {
    let name = name_of(&n.metadata);
    let status = n.status.clone().unwrap_or_default();
    let conditions: Vec<ConditionInfo> = status
        .conditions
        .unwrap_or_default()
        .into_iter()
        .map(|c| ConditionInfo {
            type_: c.type_,
            status: c.status,
            reason: c.reason,
            message: c.message,
            last_transition_ms: ms(&c.last_transition_time),
        })
        .collect();
    let ready = conditions
        .iter()
        .any(|c| c.type_ == "Ready" && c.status == "True");

    let mut roles: Vec<String> = n
        .metadata
        .labels
        .as_ref()
        .map(|l| {
            l.keys()
                .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    roles.sort();

    let info = status.node_info.unwrap_or_default();
    let internal_ip = status.addresses.and_then(|a| {
        a.into_iter()
            .find(|a| a.type_ == "InternalIP")
            .map(|a| a.address)
    });
    let (cpu_usage, mem_usage) = match usage.nodes.get(&name) {
        Some((c, m)) => (Some(*c), Some(*m)),
        None => (None, None),
    };
    let pod_capacity = status
        .allocatable
        .as_ref()
        .and_then(|m| m.get("pods"))
        .and_then(|q| q.0.parse().ok())
        .unwrap_or(0);

    NodeInfo {
        ready,
        unschedulable: n.spec.as_ref().and_then(|s| s.unschedulable).unwrap_or(false),
        roles,
        kubelet_version: info.kubelet_version,
        os_image: info.os_image,
        kernel_version: info.kernel_version,
        internal_ip,
        cpu_capacity: q_cpu(&status.capacity, "cpu"),
        cpu_allocatable: q_cpu(&status.allocatable, "cpu"),
        mem_capacity: q_mem(&status.capacity, "memory"),
        mem_allocatable: q_mem(&status.allocatable, "memory"),
        cpu_usage,
        mem_usage,
        cpu_requests: 0.0,
        mem_requests: 0.0,
        pod_count: 0,
        pod_capacity,
        conditions,
        created_ms: ms(&n.metadata.creation_timestamp),
        name,
    }
}

fn container_info(cs: &ContainerStatus, init: bool) -> ContainerInfo {
    let mut info = ContainerInfo {
        name: cs.name.clone(),
        image: cs.image.clone(),
        init,
        ready: cs.ready,
        restarts: cs.restart_count,
        state: "unknown".into(),
        ..Default::default()
    };
    if let Some(state) = &cs.state {
        if let Some(r) = &state.running {
            info.state = "running".into();
            info.started_ms = ms(&r.started_at);
        } else if let Some(w) = &state.waiting {
            info.state = "waiting".into();
            info.reason = w.reason.clone();
            info.message = w.message.clone();
        } else if let Some(t) = &state.terminated {
            info.state = "terminated".into();
            info.reason = t.reason.clone().or_else(|| Some(format!("ExitCode:{}", t.exit_code)));
            info.message = t.message.clone();
        }
    }
    if let Some(t) = cs.last_state.as_ref().and_then(|s| s.terminated.as_ref()) {
        info.last_terminated_reason = t.reason.clone();
        info.last_terminated_exit_code = Some(t.exit_code);
        info.last_terminated_ms = ms(&t.finished_at);
    }
    info
}

fn sum_requests(spec: &Option<PodSpec>) -> (f64, f64, f64) {
    let Some(spec) = spec else { return (0.0, 0.0, 0.0) };
    let mut cpu = 0.0;
    let mut mem = 0.0;
    let mut mem_lim = 0.0;
    for c in &spec.containers {
        if let Some(r) = &c.resources {
            cpu += q_cpu(&r.requests, "cpu");
            mem += q_mem(&r.requests, "memory");
            mem_lim += q_mem(&r.limits, "memory");
        }
    }
    (cpu, mem, mem_lim)
}

pub fn pod_info(p: &Pod, usage: &UsageMetrics) -> PodInfo {
    let namespace = ns_of(&p.metadata);
    let name = name_of(&p.metadata);
    let status = p.status.clone().unwrap_or_default();
    let phase = status.phase.clone().unwrap_or_else(|| "Unknown".into());

    let init: Vec<ContainerInfo> = status
        .init_container_statuses
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|c| container_info(c, true))
        .collect();
    let main: Vec<ContainerInfo> = status
        .container_statuses
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|c| container_info(c, false))
        .collect();

    // kubectl-style status column.
    let mut display = status.reason.clone().unwrap_or_else(|| phase.clone());
    let mut initializing = false;
    for (i, c) in init.iter().enumerate() {
        match c.state.as_str() {
            "terminated" if c.reason.as_deref() == Some("Completed") => continue,
            "terminated" => {
                display = format!("Init:{}", c.reason.clone().unwrap_or_default());
                initializing = true;
            }
            "waiting" if c.reason.as_deref().is_some_and(|r| r != "PodInitializing") => {
                display = format!("Init:{}", c.reason.clone().unwrap_or_default());
                initializing = true;
            }
            _ => {
                display = format!("Init:{}/{}", i, init.len());
                initializing = true;
            }
        }
        break;
    }
    if !initializing {
        for c in main.iter().rev() {
            match c.state.as_str() {
                "waiting" if c.reason.is_some() => display = c.reason.clone().unwrap(),
                "terminated" if c.reason.is_some() => display = c.reason.clone().unwrap(),
                _ => {}
            }
        }
        if main.iter().any(|c| c.state == "running") && phase == "Running" {
            // A running pod whose other container is backing off still shows the
            // back-off reason; only reset when nothing is waiting/terminated.
            if !main.iter().any(|c| c.state != "running") {
                display = "Running".into();
            }
        }
    }
    let deleting_since_ms = ms(&p.metadata.deletion_timestamp);
    if deleting_since_ms.is_some() {
        display = "Terminating".into();
    }

    let owner = p
        .metadata
        .owner_references
        .as_ref()
        .and_then(|o| o.iter().find(|r| r.controller == Some(true)).or(o.first()));
    let (owner_kind, owner_name) = match owner {
        Some(o) if o.kind == "ReplicaSet" => {
            // Deployment name = ReplicaSet name minus the pod-template-hash.
            let hash = p
                .metadata
                .labels
                .as_ref()
                .and_then(|l| l.get("pod-template-hash"));
            match hash.and_then(|h| o.name.strip_suffix(&format!("-{h}"))) {
                Some(dep) => (Some("Deployment".into()), Some(dep.to_string())),
                None => (Some(o.kind.clone()), Some(o.name.clone())),
            }
        }
        Some(o) => (Some(o.kind.clone()), Some(o.name.clone())),
        None => (None, None),
    };

    let (cpu_requests, mem_requests, mem_limits) = sum_requests(&p.spec);
    let key = format!("{namespace}/{name}");
    let (cpu_usage, mem_usage) = match usage.pods.get(&key) {
        Some((c, m)) => (Some(*c), Some(*m)),
        None => (None, None),
    };

    let conditions = status
        .conditions
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|c| ConditionInfo {
            type_: c.type_,
            status: c.status,
            reason: c.reason,
            message: c.message,
            last_transition_ms: ms(&c.last_transition_time),
        })
        .collect();

    let last_restart_ms = main.iter().filter_map(|c| c.last_terminated_ms).max();
    let mut containers = init;
    containers.extend(main);

    PodInfo {
        ready_containers: containers.iter().filter(|c| !c.init && c.ready).count(),
        total_containers: containers.iter().filter(|c| !c.init).count(),
        restarts: containers.iter().filter(|c| !c.init).map(|c| c.restarts).sum(),
        namespace,
        name,
        uid: p.metadata.uid.clone().unwrap_or_default(),
        node: p.spec.as_ref().and_then(|s| s.node_name.clone()),
        phase,
        status: display,
        last_restart_ms,
        created_ms: ms(&p.metadata.creation_timestamp),
        deleting_since_ms,
        owner_kind,
        owner_name,
        pod_ip: status.pod_ip.clone(),
        qos_class: status.qos_class.clone(),
        cpu_usage,
        mem_usage,
        cpu_requests,
        mem_requests,
        mem_limits,
        containers,
        conditions,
    }
}

fn images(spec: &Option<PodSpec>) -> Vec<String> {
    spec.as_ref()
        .map(|s| s.containers.iter().filter_map(|c| c.image.clone()).collect())
        .unwrap_or_default()
}

/// Remembered replica count, only while the workload is actually at 0 (a
/// manual scale-up makes a leftover annotation meaningless).
fn disabled_replicas(meta: &ObjectMeta, desired: i32) -> Option<i32> {
    if desired != 0 {
        return None;
    }
    meta.annotations
        .as_ref()?
        .get(DISABLED_REPLICAS_ANNOTATION)?
        .parse()
        .ok()
}

fn deployment_info(d: &Deployment) -> WorkloadInfo {
    let spec = d.spec.clone().unwrap_or_default();
    let st = d.status.clone().unwrap_or_default();
    let condition_message = st.conditions.as_ref().and_then(|cs| {
        cs.iter()
            .find(|c| {
                (c.type_ == "Progressing" && c.status == "False")
                    || (c.type_ == "ReplicaFailure" && c.status == "True")
            })
            .and_then(|c| c.message.clone())
    });
    WorkloadInfo {
        kind: "Deployment".into(),
        namespace: ns_of(&d.metadata),
        name: name_of(&d.metadata),
        desired: spec.replicas.unwrap_or(1),
        ready: st.ready_replicas.unwrap_or(0),
        available: st.available_replicas.unwrap_or(0),
        updated: st.updated_replicas.unwrap_or(0),
        images: images(&spec.template.spec),
        paused: spec.paused.unwrap_or(false),
        disabled_replicas: disabled_replicas(&d.metadata, spec.replicas.unwrap_or(1)),
        condition_message,
        created_ms: ms(&d.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn statefulset_info(s: &StatefulSet) -> WorkloadInfo {
    let spec = s.spec.clone().unwrap_or_default();
    let st = s.status.clone().unwrap_or_default();
    WorkloadInfo {
        kind: "StatefulSet".into(),
        namespace: ns_of(&s.metadata),
        name: name_of(&s.metadata),
        desired: spec.replicas.unwrap_or(1),
        ready: st.ready_replicas.unwrap_or(0),
        available: st.available_replicas.unwrap_or(0),
        updated: st.updated_replicas.unwrap_or(0),
        images: images(&spec.template.spec),
        disabled_replicas: disabled_replicas(&s.metadata, spec.replicas.unwrap_or(1)),
        created_ms: ms(&s.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn daemonset_info(d: &DaemonSet) -> WorkloadInfo {
    let spec = d.spec.clone().unwrap_or_default();
    let st = d.status.clone().unwrap_or_default();
    WorkloadInfo {
        kind: "DaemonSet".into(),
        namespace: ns_of(&d.metadata),
        name: name_of(&d.metadata),
        desired: st.desired_number_scheduled,
        ready: st.number_ready,
        available: st.number_available.unwrap_or(0),
        updated: st.updated_number_scheduled.unwrap_or(0),
        images: images(&spec.template.spec),
        created_ms: ms(&d.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn job_info(j: &Job) -> WorkloadInfo {
    let spec = j.spec.clone().unwrap_or_default();
    let st = j.status.clone().unwrap_or_default();
    let condition_message = st.conditions.as_ref().and_then(|cs| {
        cs.iter()
            .find(|c| c.type_ == "Failed" && c.status == "True")
            .map(|c| {
                c.message
                    .clone()
                    .or_else(|| c.reason.clone())
                    .unwrap_or_else(|| "Job failed".into())
            })
    });
    WorkloadInfo {
        kind: "Job".into(),
        namespace: ns_of(&j.metadata),
        name: name_of(&j.metadata),
        desired: spec.completions.unwrap_or(1),
        ready: st.succeeded.unwrap_or(0),
        available: st.active.unwrap_or(0),
        failed: st.failed.unwrap_or(0),
        images: images(&spec.template.spec),
        paused: spec.suspend.unwrap_or(false),
        condition_message,
        created_ms: ms(&j.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn cronjob_info(c: &CronJob) -> WorkloadInfo {
    let spec = &c.spec;
    let st = c.status.clone().unwrap_or_default();
    WorkloadInfo {
        kind: "CronJob".into(),
        namespace: ns_of(&c.metadata),
        name: name_of(&c.metadata),
        available: st.active.map(|a| a.len() as i32).unwrap_or(0),
        images: spec
            .job_template
            .spec
            .as_ref()
            .map(|s| images(&s.template.spec))
            .unwrap_or_default(),
        paused: spec.suspend.unwrap_or(false),
        schedule: Some(spec.schedule.clone()),
        last_schedule_ms: ms(&st.last_schedule_time),
        created_ms: ms(&c.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn pvc_info(p: &PersistentVolumeClaim) -> VolumeClaimInfo {
    let st = p.status.clone().unwrap_or_default();
    VolumeClaimInfo {
        namespace: ns_of(&p.metadata),
        name: name_of(&p.metadata),
        phase: st.phase.unwrap_or_else(|| "Unknown".into()),
        storage_class: p.spec.as_ref().and_then(|s| s.storage_class_name.clone()),
        capacity: st
            .capacity
            .as_ref()
            .and_then(|c| c.get("storage"))
            .map(|q| q.0.clone()),
        volume_name: p.spec.as_ref().and_then(|s| s.volume_name.clone()),
        created_ms: ms(&p.metadata.creation_timestamp),
    }
}

fn event_info(e: &Event) -> EventInfo {
    let event_time = e.event_time.as_ref().map(|t| t.0.as_millisecond());
    let series_last = e
        .series
        .as_ref()
        .and_then(|s| s.last_observed_time.as_ref())
        .map(|t| t.0.as_millisecond());
    EventInfo {
        namespace: e
            .involved_object
            .namespace
            .clone()
            .or_else(|| e.metadata.namespace.clone())
            .unwrap_or_default(),
        object_kind: e.involved_object.kind.clone().unwrap_or_default(),
        object_name: e.involved_object.name.clone().unwrap_or_default(),
        reason: e.reason.clone().unwrap_or_default(),
        message: e.message.clone().unwrap_or_default().trim().to_string(),
        type_: e.type_.clone().unwrap_or_default(),
        count: e
            .count
            .or_else(|| e.series.as_ref().and_then(|s| s.count))
            .unwrap_or(1),
        first_ms: ms(&e.first_timestamp).or(event_time),
        last_ms: series_last
            .or_else(|| ms(&e.last_timestamp))
            .or(event_time)
            .or_else(|| ms(&e.metadata.creation_timestamp)),
        source: e
            .source
            .as_ref()
            .and_then(|s| s.component.clone())
            .or_else(|| e.reporting_component.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deployment(replicas: i32, annotation: Option<&str>) -> Deployment {
        let mut v = serde_json::json!({
            "metadata": { "name": "web", "namespace": "apps" },
            "spec": { "replicas": replicas, "selector": {}, "template": { "spec": { "containers": [] } } }
        });
        if let Some(a) = annotation {
            v["metadata"]["annotations"] = serde_json::json!({ DISABLED_REPLICAS_ANNOTATION: a });
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn disabled_only_while_at_zero() {
        assert_eq!(deployment_info(&deployment(0, Some("3"))).disabled_replicas, Some(3));
        assert_eq!(deployment_info(&deployment(2, Some("3"))).disabled_replicas, None, "manually scaled back up");
        assert_eq!(deployment_info(&deployment(0, None)).disabled_replicas, None, "plain scale-to-zero");
        assert_eq!(deployment_info(&deployment(0, Some("junk"))).disabled_replicas, None);
    }
}
