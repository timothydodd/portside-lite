//! Flatten raw Kubernetes objects into the UI snapshot DTOs.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use k8s_openapi::api::apps::v1::{DaemonSet, Deployment, StatefulSet};
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::{
    ConfigMap, ContainerStatus, Event, Node, PersistentVolumeClaim, Pod, PodSpec, Secret, Service, ServiceAccount,
};
use k8s_openapi::api::rbac::v1::{ClusterRoleBinding, RoleBinding};
use k8s_openapi::api::autoscaling::v2::HorizontalPodAutoscaler;
use k8s_openapi::api::core::v1::PersistentVolume;
use k8s_openapi::api::networking::v1::Ingress;
use k8s_openapi::api::storage::v1::StorageClass;
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
    pub services: Vec<Service>,
    pub configmaps: Vec<ConfigMap>,
    pub secrets: Vec<Secret>,
    pub ingresses: Vec<Ingress>,
    pub hpas: Vec<HorizontalPodAutoscaler>,
    pub pvs: Vec<PersistentVolume>,
    pub storage_classes: Vec<StorageClass>,
    pub service_accounts: Vec<ServiceAccount>,
    pub role_bindings: Vec<RoleBinding>,
    pub cluster_role_bindings: Vec<ClusterRoleBinding>,
    /// Optional kinds whose list failed for a passing reason (not "forbidden"
    /// or "not served"): their vectors are empty because we don't know, not
    /// because there are none.
    pub unknown: Vec<&'static str>,
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

    let mut workloads: Vec<WorkloadInfo> = Vec::new();
    workloads.extend(objs.deployments.iter().map(deployment_info));
    workloads.extend(objs.statefulsets.iter().map(statefulset_info));
    workloads.extend(objs.daemonsets.iter().map(daemonset_info));
    workloads.extend(objs.jobs.iter().map(job_info));
    workloads.extend(objs.cronjobs.iter().map(cronjob_info));

    let volumes: Vec<VolumeClaimInfo> = objs.pvcs.iter().map(|p| pvc_info(p, objs)).collect();
    for v in &volumes {
        for user in &v.used_by {
            let Some((kind, name)) = user.split_once('/') else { continue };
            if let Some(w) = workloads.iter_mut().find(|w| w.kind == kind && w.name == name && w.namespace == v.namespace) {
                w.claims.push(v.name.clone());
            }
        }
    }
    let services = services_info(objs);
    let ingresses = objs.ingresses.iter().map(|i| ingress_info(i, &services)).collect();
    let autoscalers = objs.hpas.iter().map(autoscaler_info).collect();
    let persistent_volumes: Vec<PersistentVolumeInfo> = objs.pvs.iter().map(pv_info).collect();
    let storage_classes = objs
        .storage_classes
        .iter()
        .map(|c| storage_class_info(c, &persistent_volumes, &volumes))
        .collect();
    let configs = configs_info(objs);
    let service_accounts = service_accounts_info(objs);
    let registries = registries_info(objs);
    let backups = backups_info(objs);

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
        services,
        ingresses,
        autoscalers,
        persistent_volumes,
        storage_classes,
        configs,
        service_accounts,
        registries,
        backups,
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
    let mut unlimited = false;
    for c in &spec.containers {
        let limit = c.resources.as_ref().map_or(0.0, |r| q_mem(&r.limits, "memory"));
        unlimited |= limit <= 0.0;
        mem_lim += limit;
        if let Some(r) = &c.resources {
            cpu += q_cpu(&r.requests, "cpu");
            mem += q_mem(&r.requests, "memory");
        }
    }
    // Init containers run one at a time before the others, so the scheduler
    // reserves whichever is larger: the biggest of them, or the regular sum.
    for c in spec.init_containers.iter().flatten() {
        if let Some(r) = &c.resources {
            cpu = cpu.max(q_cpu(&r.requests, "cpu"));
            mem = mem.max(q_mem(&r.requests, "memory"));
        }
    }
    // Usage is per pod, so a limit only means something when every container
    // has one; a sidecar without a limit makes the sum of the others misleading.
    (cpu, mem, if unlimited { 0.0 } else { mem_lim })
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

/// The backup that scaled it to 0, while it's at 0.
fn backup_offline(meta: &ObjectMeta, desired: i32) -> Option<String> {
    if desired != 0 {
        return None;
    }
    meta.annotations.as_ref()?.get(crate::backup::OFFLINE_ANNOTATION).cloned()
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
        backup_offline: backup_offline(&d.metadata, spec.replicas.unwrap_or(1)),
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
        backup_offline: backup_offline(&s.metadata, spec.replicas.unwrap_or(1)),
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
        last_successful_ms: ms(&st.last_successful_time),
        created_ms: ms(&c.metadata.creation_timestamp),
        ..Default::default()
    }
}

fn pvc_info(p: &PersistentVolumeClaim, objs: &ClusterObjects) -> VolumeClaimInfo {
    let st = p.status.clone().unwrap_or_default();
    let ns = ns_of(&p.metadata);
    let name = name_of(&p.metadata);
    let users = crate::files::claim_users(&ns, &name, objs);
    VolumeClaimInfo {
        namespace: ns.clone(),
        name: name.clone(),
        phase: st.phase.unwrap_or_else(|| "Unknown".into()),
        storage_class: p.spec.as_ref().and_then(|s| s.storage_class_name.clone()),
        capacity: st
            .capacity
            .as_ref()
            .and_then(|c| c.get("storage"))
            .map(|q| q.0.clone()),
        volume_name: p.spec.as_ref().and_then(|s| s.volume_name.clone()),
        created_ms: ms(&p.metadata.creation_timestamp),
        access_modes: p.spec.as_ref().and_then(|s| s.access_modes.clone()).unwrap_or_default(),
        mounted_by: users.pods.into_iter().map(|(pod, _)| pod).collect(),
        used_by: users.workloads,
        write_blockers: users.blockers,
    }
}

fn ingress_info(ing: &Ingress, services: &[ServiceInfo]) -> IngressInfo {
    let ns = ns_of(&ing.metadata);
    let spec = ing.spec.clone().unwrap_or_default();
    let route = |host: String, path: String, b: &k8s_openapi::api::networking::v1::IngressBackend| {
        let svc = b.service.as_ref();
        IngressRoute {
            host,
            path,
            service: svc.map(|s| s.name.clone()),
            port: svc.and_then(|s| s.port.as_ref()).and_then(|p| p.number.map(|n| n.to_string()).or_else(|| p.name.clone())),
            resource: b.resource.as_ref().map(|r| format!("{}/{}", r.kind, r.name)),
            service_found: svc.is_some_and(|s| services.iter().any(|x| x.namespace == ns && x.name == s.name)),
        }
    };
    let mut routes = Vec::new();
    if let Some(b) = &spec.default_backend {
        routes.push(route("(default)".into(), "/".into(), b));
    }
    for rule in spec.rules.iter().flatten() {
        let host = rule.host.clone().unwrap_or_else(|| "*".into());
        for p in rule.http.iter().flat_map(|h| h.paths.iter()) {
            routes.push(route(host.clone(), p.path.clone().unwrap_or_else(|| "/".into()), &p.backend));
        }
    }
    IngressInfo {
        class: spec.ingress_class_name.clone().or_else(|| {
            ing.metadata.annotations.as_ref().and_then(|a| a.get("kubernetes.io/ingress.class")).cloned()
        }),
        routes,
        tls_hosts: spec.tls.iter().flatten().flat_map(|t| t.hosts.clone().unwrap_or_default()).collect(),
        address: ing
            .status
            .as_ref()
            .and_then(|s| s.load_balancer.as_ref())
            .and_then(|lb| lb.ingress.as_ref())
            .map(|v| v.iter().filter_map(|i| i.ip.clone().or_else(|| i.hostname.clone())).collect())
            .unwrap_or_default(),
        created_ms: ms(&ing.metadata.creation_timestamp),
        name: name_of(&ing.metadata),
        namespace: ns,
    }
}

fn autoscaler_info(h: &HorizontalPodAutoscaler) -> AutoscalerInfo {
    use k8s_openapi::api::autoscaling::v2::{MetricTarget, MetricValueStatus};
    let spec = &h.spec;
    let st = h.status.clone().unwrap_or_default();
    let target = |t: &MetricTarget| {
        t.average_utilization
            .map(|u| format!("{u}%"))
            .or_else(|| t.average_value.as_ref().map(|q| q.0.clone()))
            .or_else(|| t.value.as_ref().map(|q| q.0.clone()))
            .unwrap_or_else(|| "?".into())
    };
    let current = |v: Option<&MetricValueStatus>| {
        v.and_then(|v| {
            v.average_utilization
                .map(|u| format!("{u}%"))
                .or_else(|| v.average_value.as_ref().map(|q| q.0.clone()))
                .or_else(|| v.value.as_ref().map(|q| q.0.clone()))
        })
        .unwrap_or_else(|| "?".into())
    };
    let statuses = st.current_metrics.clone().unwrap_or_default();
    let metrics = spec
        .metrics
        .iter()
        .flatten()
        .map(|m| match m.type_.as_str() {
            "Resource" => {
                let r = m.resource.as_ref();
                let name = r.map(|r| r.name.clone()).unwrap_or_default();
                let now = statuses.iter().filter_map(|s| s.resource.as_ref()).find(|s| s.name == name).map(|s| &s.current);
                format!("{name} {} / {}", current(now), r.map(|r| target(&r.target)).unwrap_or_default())
            }
            "ContainerResource" => {
                let r = m.container_resource.as_ref();
                let name = r.map(|r| format!("{} ({})", r.name, r.container)).unwrap_or_default();
                format!("{name} / {}", r.map(|r| target(&r.target)).unwrap_or_default())
            }
            "Pods" => m.pods.as_ref().map(|p| format!("{} / {}", p.metric.name, target(&p.target))).unwrap_or_default(),
            "Object" => m.object.as_ref().map(|o| format!("{} / {}", o.metric.name, target(&o.target))).unwrap_or_default(),
            "External" => m.external.as_ref().map(|e| format!("{} / {}", e.metric.name, target(&e.target))).unwrap_or_default(),
            other => other.to_string(),
        })
        .collect();
    let conditions = st.conditions.clone().unwrap_or_default();
    let problem = conditions
        .iter()
        .find(|c| (c.type_ == "AbleToScale" || c.type_ == "ScalingActive") && c.status == "False")
        .map(|c| c.message.clone().or_else(|| c.reason.clone()).unwrap_or_else(|| format!("{} is False", c.type_)));
    let at_max = conditions
        .iter()
        .any(|c| c.type_ == "ScalingLimited" && c.status == "True" && c.reason.as_deref() == Some("TooManyReplicas"));
    AutoscalerInfo {
        namespace: ns_of(&h.metadata),
        name: name_of(&h.metadata),
        target_kind: spec.scale_target_ref.kind.clone(),
        target_name: spec.scale_target_ref.name.clone(),
        min_replicas: spec.min_replicas.unwrap_or(1),
        max_replicas: spec.max_replicas,
        current_replicas: st.current_replicas.unwrap_or(0),
        desired_replicas: st.desired_replicas,
        metrics,
        problem,
        at_max,
        last_scale_ms: ms(&st.last_scale_time),
        created_ms: ms(&h.metadata.creation_timestamp),
    }
}

fn pv_info(pv: &PersistentVolume) -> PersistentVolumeInfo {
    let spec = pv.spec.clone().unwrap_or_default();
    let st = pv.status.clone().unwrap_or_default();
    let source = if let Some(h) = &spec.host_path {
        Some(format!("hostPath {}", h.path))
    } else if let Some(l) = &spec.local {
        Some(format!("local {}", l.path))
    } else if let Some(n) = &spec.nfs {
        Some(format!("nfs {}:{}", n.server, n.path))
    } else if let Some(c) = &spec.csi {
        Some(format!("csi {}", c.driver))
    } else {
        None
    };
    // local-path / local volumes pin to one node through a hostname term.
    let node = spec
        .node_affinity
        .as_ref()
        .and_then(|a| a.required.as_ref())
        .and_then(|r| r.node_selector_terms.first())
        .and_then(|t| t.match_expressions.as_ref())
        .and_then(|es| es.iter().find(|e| e.key == "kubernetes.io/hostname"))
        .and_then(|e| e.values.as_ref())
        .and_then(|v| v.first().cloned());
    PersistentVolumeInfo {
        name: name_of(&pv.metadata),
        phase: st.phase.unwrap_or_else(|| "Unknown".into()),
        capacity: spec.capacity.as_ref().and_then(|c| c.get("storage")).map(|q| q.0.clone()),
        storage_class: spec.storage_class_name.clone().filter(|c| !c.is_empty()),
        reclaim_policy: spec.persistent_volume_reclaim_policy.clone(),
        access_modes: spec.access_modes.clone().unwrap_or_default(),
        claim: spec
            .claim_ref
            .as_ref()
            .and_then(|c| Some(format!("{}/{}", c.namespace.as_deref()?, c.name.as_deref()?))),
        source,
        node,
        message: st.message.filter(|m| !m.is_empty()),
        phase_since_ms: ms(&st.last_phase_transition_time),
        created_ms: ms(&pv.metadata.creation_timestamp),
    }
}

fn storage_class_info(c: &StorageClass, pvs: &[PersistentVolumeInfo], claims: &[VolumeClaimInfo]) -> StorageClassInfo {
    let name = name_of(&c.metadata);
    let annotations = c.metadata.annotations.as_ref();
    let is_default = ["storageclass.kubernetes.io/is-default-class", "storageclass.beta.kubernetes.io/is-default-class"]
        .iter()
        .any(|k| annotations.and_then(|a| a.get(*k)).is_some_and(|v| v == "true"));
    StorageClassInfo {
        provisioner: c.provisioner.clone(),
        reclaim_policy: c.reclaim_policy.clone(),
        binding_mode: c.volume_binding_mode.clone(),
        allow_expansion: c.allow_volume_expansion.unwrap_or(false),
        is_default,
        volumes: pvs.iter().filter(|p| p.storage_class.as_deref() == Some(name.as_str())).count(),
        claims: claims.iter().filter(|p| p.storage_class.as_deref() == Some(name.as_str())).count(),
        created_ms: ms(&c.metadata.creation_timestamp),
        name,
    }
}

fn pod_is_ready(p: &Pod) -> bool {
    p.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|cs| cs.iter().any(|c| c.type_ == "Ready" && c.status == "True"))
}

/// Services with live endpoint health computed from pod labels + readiness,
/// and the Ingress rules that route to each.
fn services_info(objs: &ClusterObjects) -> Vec<ServiceInfo> {
    // Ingress routes per (namespace, service).
    let mut routes: HashMap<(String, String), Vec<String>> = HashMap::new();
    for ing in &objs.ingresses {
        let ns = ns_of(&ing.metadata);
        let ing_name = name_of(&ing.metadata);
        let Some(spec) = &ing.spec else { continue };
        if let Some(svc) = spec.default_backend.as_ref().and_then(|b| b.service.as_ref()) {
            routes.entry((ns.clone(), svc.name.clone())).or_default().push(format!("* (default, {ing_name})"));
        }
        for rule in spec.rules.iter().flatten() {
            let host = rule.host.clone().unwrap_or_else(|| "*".into());
            for path in rule.http.iter().flat_map(|h| h.paths.iter()) {
                if let Some(svc) = &path.backend.service {
                    let p = path.path.clone().unwrap_or_else(|| "/".into());
                    routes.entry((ns.clone(), svc.name.clone())).or_default().push(format!("{host}{p} ({ing_name})"));
                }
            }
        }
    }

    // Pod labels of workloads that have no pods on purpose: scaled to 0, or
    // CronJobs (pods only while a run is in progress).
    type Labels = BTreeMap<String, String>;
    let mut idle_templates: Vec<(String, Labels)> = Vec::new();
    let labels_of = |m: Option<&ObjectMeta>| m.and_then(|m| m.labels.clone()).unwrap_or_default();
    for d in &objs.deployments {
        if d.spec.as_ref().is_some_and(|s| s.replicas == Some(0)) {
            idle_templates.push((ns_of(&d.metadata), labels_of(d.spec.as_ref().and_then(|s| s.template.metadata.as_ref()))));
        }
    }
    for st in &objs.statefulsets {
        if st.spec.as_ref().is_some_and(|s| s.replicas == Some(0)) {
            idle_templates.push((ns_of(&st.metadata), labels_of(st.spec.as_ref().and_then(|s| s.template.metadata.as_ref()))));
        }
    }
    for c in &objs.cronjobs {
        let template = c.spec.job_template.spec.as_ref().and_then(|j| j.template.metadata.as_ref());
        idle_templates.push((ns_of(&c.metadata), labels_of(template)));
    }

    objs.services
        .iter()
        .map(|s| {
            let spec = s.spec.clone().unwrap_or_default();
            let ns = ns_of(&s.metadata);
            let name = name_of(&s.metadata);
            let selector: BTreeMap<String, String> = spec.selector.clone().unwrap_or_default();
            let matched: Vec<&Pod> = if selector.is_empty() {
                Vec::new()
            } else {
                objs.pods
                    .iter()
                    .filter(|p| p.metadata.namespace.as_deref() == Some(ns.as_str()))
                    .filter(|p| p.metadata.deletion_timestamp.is_none())
                    .filter(|p| p.status.as_ref().and_then(|st| st.phase.as_deref()) == Some("Running"))
                    .filter(|p| {
                        let labels = p.metadata.labels.as_ref();
                        selector.iter().all(|(k, v)| labels.and_then(|l| l.get(k)) == Some(v))
                    })
                    .collect()
            };
            let mut external: Vec<String> = s
                .status
                .as_ref()
                .and_then(|st| st.load_balancer.as_ref())
                .and_then(|lb| lb.ingress.as_ref())
                .map(|ing| ing.iter().filter_map(|i| i.ip.clone().or_else(|| i.hostname.clone())).collect())
                .unwrap_or_default();
            external.extend(spec.external_ips.clone().unwrap_or_default());
            if let Some(en) = &spec.external_name {
                external.push(en.clone());
            }
            ServiceInfo {
                type_: spec.type_.clone().unwrap_or_else(|| "ClusterIP".into()),
                cluster_ip: spec.cluster_ip.clone().filter(|ip| ip != "None" && !ip.is_empty()),
                external,
                ports: spec
                    .ports
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| ServicePort {
                        name: p.name,
                        port: p.port,
                        target_port: p.target_port.map(|t| match t {
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(i) => i.to_string(),
                            k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s) => s,
                        }),
                        node_port: p.node_port,
                        protocol: p.protocol.unwrap_or_else(|| "TCP".into()),
                    })
                    .collect(),
                idle: matched.is_empty() && idle_templates.iter().any(|(tns, labels)| {
                    *tns == ns && !selector.is_empty() && selector.iter().all(|(k, v)| labels.get(k) == Some(v))
                }),
                pods_ready: matched.iter().filter(|p| pod_is_ready(p)).count(),
                pods_matched: matched.len(),
                pod_names: matched.iter().map(|p| name_of(&p.metadata)).collect(),
                routes: routes.remove(&(ns.clone(), name.clone())).unwrap_or_default(),
                created_ms: ms(&s.metadata.creation_timestamp),
                selector,
                namespace: ns,
                name,
            }
        })
        .collect()
}

/// ConfigMaps and Secrets (key names and sizes only) with the workloads that
/// reference each.
fn configs_info(objs: &ClusterObjects) -> Vec<ConfigInfo> {
    // (namespace, kind, name) → ["Deployment/web", …]
    let mut used: HashMap<(String, String, String), Vec<String>> = HashMap::new();
    let mut note = |kind: &str, meta: &ObjectMeta, value: serde_json::Result<serde_json::Value>| {
        let Ok(v) = value else { return };
        let ns = ns_of(meta);
        let who = format!("{kind}/{}", name_of(meta));
        for r in crate::manifest::references(&v) {
            if r.kind == "ConfigMap" || r.kind == "Secret" {
                used.entry((ns.clone(), r.kind, r.name)).or_default().push(who.clone());
            }
        }
    };
    for d in &objs.deployments {
        note("Deployment", &d.metadata, serde_json::to_value(d));
    }
    for s in &objs.statefulsets {
        note("StatefulSet", &s.metadata, serde_json::to_value(s));
    }
    for d in &objs.daemonsets {
        note("DaemonSet", &d.metadata, serde_json::to_value(d));
    }
    for c in &objs.cronjobs {
        note("CronJob", &c.metadata, serde_json::to_value(c));
    }
    for j in objs.jobs.iter().filter(|j| unowned(&j.metadata)) {
        note("Job", &j.metadata, serde_json::to_value(j));
    }
    // Pull secrets handed out by a ServiceAccount are used by its pods.
    for sa in &objs.service_accounts {
        for ips in sa.image_pull_secrets.iter().flatten() {
            used.entry((ns_of(&sa.metadata), "Secret".into(), ips.name.clone()))
                .or_default()
                .push(format!("ServiceAccount/{}", name_of(&sa.metadata)));
        }
    }

    let mut out = Vec::new();
    for c in &objs.configmaps {
        let mut keys: Vec<String> = c.data.iter().flat_map(|d| d.keys().cloned()).collect();
        keys.extend(c.binary_data.iter().flat_map(|d| d.keys().cloned()));
        keys.sort();
        let size = c.data.iter().flat_map(|d| d.values()).map(String::len).sum::<usize>()
            + c.binary_data.iter().flat_map(|d| d.values()).map(|b| b.0.len()).sum::<usize>();
        let (ns, name) = (ns_of(&c.metadata), name_of(&c.metadata));
        out.push(ConfigInfo {
            kind: "ConfigMap".into(),
            used_by: used.remove(&(ns.clone(), "ConfigMap".into(), name.clone())).unwrap_or_default(),
            secret_type: None,
            keys,
            size_bytes: size,
            immutable: c.immutable.unwrap_or(false),
            created_ms: ms(&c.metadata.creation_timestamp),
            namespace: ns,
            name,
        });
    }
    for s in &objs.secrets {
        let mut keys: Vec<String> = s.data.iter().flat_map(|d| d.keys().cloned()).collect();
        keys.extend(s.string_data.iter().flat_map(|d| d.keys().cloned()));
        keys.sort();
        keys.dedup();
        let size = s.data.iter().flat_map(|d| d.values()).map(|b| b.0.len()).sum::<usize>();
        let (ns, name) = (ns_of(&s.metadata), name_of(&s.metadata));
        out.push(ConfigInfo {
            kind: "Secret".into(),
            used_by: used.remove(&(ns.clone(), "Secret".into(), name.clone())).unwrap_or_default(),
            secret_type: s.type_.clone(),
            keys,
            size_bytes: size,
            immutable: s.immutable.unwrap_or(false),
            created_ms: ms(&s.metadata.creation_timestamp),
            namespace: ns,
            name,
        });
    }
    out
}

fn unowned(meta: &ObjectMeta) -> bool {
    meta.owner_references.as_ref().is_none_or(|o| o.is_empty())
}

/// Every top-level thing that runs pods, as (namespace, "Kind/name", pod
/// spec): workloads, Jobs no CronJob owns, and bare Pods.
fn pod_specs(objs: &ClusterObjects) -> Vec<(String, String, &PodSpec)> {
    fn add<'a>(out: &mut Vec<(String, String, &'a PodSpec)>, kind: &str, meta: &ObjectMeta, spec: Option<&'a PodSpec>) {
        if let Some(spec) = spec {
            out.push((ns_of(meta), format!("{kind}/{}", name_of(meta)), spec));
        }
    }
    let mut out = Vec::new();
    for d in &objs.deployments {
        add(&mut out, "Deployment", &d.metadata, d.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
    }
    for s in &objs.statefulsets {
        add(&mut out, "StatefulSet", &s.metadata, s.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
    }
    for d in &objs.daemonsets {
        add(&mut out, "DaemonSet", &d.metadata, d.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
    }
    for c in &objs.cronjobs {
        add(&mut out, "CronJob", &c.metadata, c.spec.job_template.spec.as_ref().and_then(|j| j.template.spec.as_ref()));
    }
    for j in objs.jobs.iter().filter(|j| unowned(&j.metadata)) {
        add(&mut out, "Job", &j.metadata, j.spec.as_ref().and_then(|s| s.template.spec.as_ref()));
    }
    for p in objs.pods.iter().filter(|p| unowned(&p.metadata)) {
        add(&mut out, "Pod", &p.metadata, p.spec.as_ref());
    }
    out
}

fn service_account_of(spec: &PodSpec) -> String {
    spec.service_account_name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "default".into())
}

/// ServiceAccounts with their pull secrets, the roles bound to them and the
/// workloads running as them.
fn service_accounts_info(objs: &ClusterObjects) -> Vec<ServiceAccountInfo> {
    let mut roles: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut bind = |binding_ns: Option<&str>, subjects: Option<&Vec<k8s_openapi::api::rbac::v1::Subject>>, role: String| {
        for s in subjects.into_iter().flatten().filter(|s| s.kind == "ServiceAccount") {
            // A RoleBinding subject without a namespace means the binding's own.
            if let Some(ns) = s.namespace.as_deref().filter(|n| !n.is_empty()).or(binding_ns) {
                roles.entry((ns.to_string(), s.name.clone())).or_default().push(role.clone());
            }
        }
    };
    for rb in &objs.role_bindings {
        let ns = ns_of(&rb.metadata);
        let role = format!("{}/{} (RoleBinding {})", rb.role_ref.kind, rb.role_ref.name, name_of(&rb.metadata));
        bind(Some(&ns), rb.subjects.as_ref(), role);
    }
    for crb in &objs.cluster_role_bindings {
        let role = format!("{}/{} (ClusterRoleBinding {}, all namespaces)", crb.role_ref.kind, crb.role_ref.name, name_of(&crb.metadata));
        bind(None, crb.subjects.as_ref(), role);
    }

    let mut used: HashMap<(String, String), Vec<String>> = HashMap::new();
    for (ns, who, spec) in pod_specs(objs) {
        used.entry((ns, service_account_of(spec))).or_default().push(who);
    }
    let secrets: BTreeSet<(String, String)> = objs.secrets.iter().map(|s| (ns_of(&s.metadata), name_of(&s.metadata))).collect();
    // Without Secret access (or before it loads) nothing can be called missing.
    let can_check = !objs.secrets.is_empty() && !objs.unknown.contains(&"Secret");

    let mut out: Vec<ServiceAccountInfo> = objs
        .service_accounts
        .iter()
        .map(|sa| {
            let (ns, name) = (ns_of(&sa.metadata), name_of(&sa.metadata));
            let pull: Vec<String> = sa.image_pull_secrets.iter().flatten().map(|r| r.name.clone()).filter(|n| !n.is_empty()).collect();
            let key = (ns.clone(), name.clone());
            let mut roles = roles.remove(&key).unwrap_or_default();
            roles.sort();
            let mut used_by = used.remove(&key).unwrap_or_default();
            used_by.sort();
            ServiceAccountInfo {
                missing_pull_secrets: if can_check {
                    pull.iter().filter(|p| !secrets.contains(&(ns.clone(), (*p).clone()))).cloned().collect()
                } else {
                    Vec::new()
                },
                image_pull_secrets: pull,
                automount_token: sa.automount_service_account_token,
                roles,
                used_by,
                created_ms: ms(&sa.metadata.creation_timestamp),
                namespace: ns,
                name,
            }
        })
        .collect();
    out.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    out
}

const PULL_ERRORS: [&str; 3] = ["ImagePullBackOff", "ErrImagePull", "InvalidImageName"];

/// Backup CronJobs with their latest outcome, from their Jobs (each run is a
/// Job owned by the CronJob) and their claim.
fn backups_info(objs: &ClusterObjects) -> Vec<BackupInfo> {
    use crate::backup;
    let mut out: Vec<BackupInfo> = objs
        .cronjobs
        .iter()
        .filter(|c| backup::is_backup(&c.metadata))
        .map(|c| {
            let namespace = ns_of(&c.metadata);
            let cron_job = name_of(&c.metadata);
            let st = c.status.clone().unwrap_or_default();
            let owned = |j: &&Job| {
                ns_of(&j.metadata) == namespace
                    && j.metadata.owner_references.as_ref().is_some_and(|o| o.iter().any(|r| r.kind == "CronJob" && r.name == cron_job))
            };
            // The newest finished run decides whether the backup is failing.
            let last = objs
                .jobs
                .iter()
                .filter(owned)
                .filter(|j| {
                    j.status.as_ref().and_then(|s| s.conditions.as_ref()).is_some_and(|cs| {
                        cs.iter().any(|c| (c.type_ == "Complete" || c.type_ == "Failed") && c.status == "True")
                    })
                })
                .max_by_key(|j| ms(&j.metadata.creation_timestamp))
                .map(job_info);
            let (last_failure, last_failure_ms) = match last {
                Some(j) if j.condition_message.is_some() => (j.condition_message, j.created_ms),
                _ => (None, None),
            };
            let claim = objs
                .pvcs
                .iter()
                .find(|p| ns_of(&p.metadata) == namespace && name_of(&p.metadata) == cron_job);
            BackupInfo {
                name: backup::backup_name(&cron_job).unwrap_or(&cron_job).to_string(),
                spec: backup::spec_of(c),
                schedule: c.spec.schedule.clone(),
                time_zone: c.spec.time_zone.clone(),
                suspended: c.spec.suspend.unwrap_or(false),
                running: st.active.is_some_and(|a| !a.is_empty()),
                last_schedule_ms: ms(&st.last_schedule_time),
                last_successful_ms: ms(&st.last_successful_time),
                last_failure,
                last_failure_ms,
                claim_phase: claim.map(|p| p.status.as_ref().and_then(|s| s.phase.clone()).unwrap_or_else(|| "Pending".into())),
                claim_capacity: claim.and_then(|p| p.status.as_ref()?.capacity.as_ref()?.get("storage").map(|q| q.0.clone())),
                created_ms: ms(&c.metadata.creation_timestamp),
                namespace,
                cron_job,
            }
        })
        .collect();
    out.sort_by(|a, b| a.namespace.cmp(&b.namespace).then_with(|| a.name.cmp(&b.name)));
    out
}

/// Registries rolled up from the images in use and the pull secrets in the
/// cluster: who pulls from each, which credentials cover it, and where
/// pulls are failing.
fn registries_info(objs: &ClusterObjects) -> Vec<RegistryInfo> {
    use crate::registry::{credential_hosts, split_image, DOCKER_CFG, DOCKER_CONFIG_JSON};

    // (namespace, secret) → hosts it covers.
    let mut secret_hosts: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut reg: BTreeMap<String, RegistryInfo> = BTreeMap::new();
    let mut sa_pull: HashMap<(String, String), Vec<String>> = HashMap::new();
    for sa in &objs.service_accounts {
        let pulls: Vec<String> = sa.image_pull_secrets.iter().flatten().map(|r| r.name.clone()).collect();
        sa_pull.insert((ns_of(&sa.metadata), name_of(&sa.metadata)), pulls);
    }

    for s in &objs.secrets {
        let t = s.type_.as_deref().unwrap_or_default();
        let key = match t {
            DOCKER_CONFIG_JSON => ".dockerconfigjson",
            DOCKER_CFG => ".dockercfg",
            _ => continue,
        };
        let Some(payload) = s.data.as_ref().and_then(|d| d.get(key)) else { continue };
        let (ns, name) = (ns_of(&s.metadata), name_of(&s.metadata));
        let mut via: Vec<String> = sa_pull
            .iter()
            .filter(|((sns, _), pulls)| *sns == ns && pulls.contains(&name))
            .map(|((_, sa), _)| sa.clone())
            .collect();
        via.sort();
        for (host, username) in credential_hosts(t, &payload.0) {
            secret_hosts.entry((ns.clone(), name.clone())).or_default().push(host.clone());
            reg.entry(host.clone()).or_insert_with(|| RegistryInfo { host, ..Default::default() }).credentials.push(RegistryCredential {
                namespace: ns.clone(),
                secret: name.clone(),
                username,
                service_accounts: via.clone(),
            });
        }
    }

    for (ns, who, spec) in pod_specs(objs) {
        let mut pulls: Vec<String> = spec.image_pull_secrets.iter().flatten().map(|r| r.name.clone()).collect();
        pulls.extend(sa_pull.get(&(ns.clone(), service_account_of(spec))).cloned().unwrap_or_default());
        let covered: BTreeSet<&String> = pulls.iter().filter_map(|p| secret_hosts.get(&(ns.clone(), p.clone()))).flatten().collect();
        let containers = spec.containers.iter().chain(spec.init_containers.iter().flatten());
        for c in containers {
            let Some(image) = c.image.as_deref().filter(|i| !i.is_empty()) else { continue };
            let (host, rest) = split_image(image);
            let r = reg.entry(host.clone()).or_insert_with(|| RegistryInfo { host: host.clone(), ..Default::default() });
            r.images.push(rest);
            r.used_by.push(format!("{ns}/{who}"));
            if !covered.contains(&host) {
                r.without_credentials.push(format!("{ns}/{who}"));
            }
        }
    }

    for p in &objs.pods {
        let Some(status) = &p.status else { continue };
        let statuses = status.container_statuses.iter().flatten().chain(status.init_container_statuses.iter().flatten());
        for cs in statuses {
            let waiting = cs.state.as_ref().and_then(|s| s.waiting.as_ref()).and_then(|w| w.reason.as_deref());
            if !waiting.is_some_and(|r| PULL_ERRORS.contains(&r)) {
                continue;
            }
            let (host, _) = split_image(&cs.image);
            let r = reg.entry(host.clone()).or_insert_with(|| RegistryInfo { host, ..Default::default() });
            r.pull_errors.push(format!("{}/{} ({}): {}", ns_of(&p.metadata), name_of(&p.metadata), cs.name, cs.image));
        }
    }

    reg.into_values()
        .map(|mut r| {
            for v in [&mut r.images, &mut r.used_by, &mut r.without_credentials, &mut r.pull_errors] {
                v.sort();
                v.dedup();
            }
            r.credentials.sort_by(|a, b| (&a.namespace, &a.secret).cmp(&(&b.namespace, &b.secret)));
            r
        })
        .collect()
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

    #[test]
    fn service_endpoints_from_pod_labels_and_readiness() {
        let pod = |name: &str, app: &str, ready: bool| -> Pod {
            serde_json::from_value(serde_json::json!({
                "metadata": { "name": name, "namespace": "apps", "labels": { "app": app } },
                "spec": { "containers": [] },
                "status": { "phase": "Running", "conditions": [{ "type": "Ready", "status": if ready { "True" } else { "False" } }] }
            }))
            .unwrap()
        };
        let svc: Service = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web", "namespace": "apps" },
            "spec": { "selector": { "app": "web" }, "ports": [{ "port": 80, "targetPort": 8080 }] }
        }))
        .unwrap();
        let ing: Ingress = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web-ing", "namespace": "apps" },
            "spec": { "rules": [{ "host": "shop.lan", "http": { "paths": [{ "path": "/", "pathType": "Prefix",
                      "backend": { "service": { "name": "web", "port": { "number": 80 } } } }] } }] }
        }))
        .unwrap();
        let objs = ClusterObjects {
            pods: vec![pod("web-1", "web", true), pod("web-2", "web", false), pod("db-1", "db", true)],
            services: vec![svc],
            ingresses: vec![ing],
            ..Default::default()
        };
        let snap = build_snapshot("c", &objs, &UsageMetrics::default(), 0);
        let s = &snap.services[0];
        assert_eq!((s.pods_matched, s.pods_ready), (2, 1));
        assert_eq!(s.ports[0].target_port.as_deref(), Some("8080"));
        assert_eq!(s.routes, vec!["shop.lan/ (web-ing)"]);
    }

    #[test]
    fn ingresses_autoscalers_and_storage() {
        let ing: Ingress = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web", "namespace": "apps" },
            "spec": {
                "ingressClassName": "traefik",
                "tls": [{ "hosts": ["shop.lan"] }],
                "rules": [{ "host": "shop.lan", "http": { "paths": [
                    { "path": "/", "pathType": "Prefix", "backend": { "service": { "name": "web", "port": { "number": 80 } } } },
                    { "path": "/api", "pathType": "Prefix", "backend": { "service": { "name": "api", "port": { "name": "http" } } } }
                ] } }]
            },
            "status": { "loadBalancer": { "ingress": [{ "ip": "192.168.1.240" }] } }
        }))
        .unwrap();
        let svc: Service = serde_json::from_value(serde_json::json!({ "metadata": { "name": "web", "namespace": "apps" }, "spec": {} })).unwrap();
        let hpa: HorizontalPodAutoscaler = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web", "namespace": "apps" },
            "spec": { "scaleTargetRef": { "kind": "Deployment", "name": "web" }, "minReplicas": 2, "maxReplicas": 5,
                      "metrics": [{ "type": "Resource", "resource": { "name": "cpu", "target": { "type": "Utilization", "averageUtilization": 80 } } }] },
            "status": { "currentReplicas": 5, "desiredReplicas": 5,
                        "currentMetrics": [{ "type": "Resource", "resource": { "name": "cpu", "current": { "averageUtilization": 95 } } }],
                        "conditions": [
                            { "type": "AbleToScale", "status": "True" },
                            { "type": "ScalingActive", "status": "True" },
                            { "type": "ScalingLimited", "status": "True", "reason": "TooManyReplicas" }
                        ] }
        }))
        .unwrap();
        let pv: PersistentVolume = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "pvc-1" },
            "spec": { "capacity": { "storage": "5Gi" }, "storageClassName": "local-path", "persistentVolumeReclaimPolicy": "Retain",
                      "hostPath": { "path": "/var/lib/rancher/k3s/storage/pvc-1" },
                      "claimRef": { "namespace": "apps", "name": "data" },
                      "nodeAffinity": { "required": { "nodeSelectorTerms": [{ "matchExpressions": [{ "key": "kubernetes.io/hostname", "operator": "In", "values": ["node-a"] }] }] } } },
            "status": { "phase": "Released" }
        }))
        .unwrap();
        let sc: StorageClass = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "local-path", "annotations": { "storageclass.kubernetes.io/is-default-class": "true" } },
            "provisioner": "rancher.io/local-path", "volumeBindingMode": "WaitForFirstConsumer"
        }))
        .unwrap();
        let objs = ClusterObjects { services: vec![svc], ingresses: vec![ing], hpas: vec![hpa], pvs: vec![pv], storage_classes: vec![sc], ..Default::default() };
        let snap = build_snapshot("c", &objs, &UsageMetrics::default(), 0);

        let i = &snap.ingresses[0];
        assert_eq!(i.class.as_deref(), Some("traefik"));
        assert_eq!((i.tls_hosts.clone(), i.address.clone()), (vec!["shop.lan".to_string()], vec!["192.168.1.240".to_string()]));
        let found: Vec<(&str, Option<&str>, bool)> = i.routes.iter().map(|r| (r.path.as_str(), r.port.as_deref(), r.service_found)).collect();
        assert_eq!(found, [("/", Some("80"), true), ("/api", Some("http"), false)]);

        let h = &snap.autoscalers[0];
        assert_eq!((h.min_replicas, h.max_replicas, h.current_replicas), (2, 5, 5));
        assert_eq!(h.metrics, ["cpu 95% / 80%"]);
        assert!(h.at_max && h.problem.is_none());

        let v = &snap.persistent_volumes[0];
        assert_eq!(v.claim.as_deref(), Some("apps/data"));
        assert_eq!(v.node.as_deref(), Some("node-a"));
        assert_eq!(v.source.as_deref(), Some("hostPath /var/lib/rancher/k3s/storage/pvc-1"));

        let c = &snap.storage_classes[0];
        assert!(c.is_default);
        assert_eq!(c.volumes, 1);
    }

    #[test]
    fn service_accounts_and_registries() {
        use base64::Engine as _;
        use k8s_openapi::api::core::v1::{Secret, ServiceAccount};
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
        let config = serde_json::json!({ "auths": { "ghcr.io": { "auth": b64("alice:tok") } } }).to_string();
        let secret: Secret = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "ghcr", "namespace": "apps" }, "type": "kubernetes.io/dockerconfigjson",
            "data": { ".dockerconfigjson": b64(&config) }
        }))
        .unwrap();
        let sa: ServiceAccount = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web-sa", "namespace": "apps" }, "imagePullSecrets": [{ "name": "ghcr" }, { "name": "gone" }]
        }))
        .unwrap();
        let rb: RoleBinding = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web-rb", "namespace": "apps" },
            "roleRef": { "apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "reader" },
            "subjects": [{ "kind": "ServiceAccount", "name": "web-sa" }]
        }))
        .unwrap();
        let deploy = |name: &str, sa: Option<&str>, image: &str| -> Deployment {
            serde_json::from_value(serde_json::json!({
                "metadata": { "name": name, "namespace": "apps" },
                "spec": { "selector": {}, "template": { "spec": { "serviceAccountName": sa, "containers": [{ "name": "c", "image": image }] } } }
            }))
            .unwrap()
        };
        let stuck: Pod = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "api-1", "namespace": "apps", "ownerReferences": [{ "apiVersion": "apps/v1", "kind": "ReplicaSet", "name": "api-x", "uid": "u" }] },
            "spec": { "containers": [{ "name": "app", "image": "ghcr.io/org/api:2" }] },
            "status": { "phase": "Pending", "containerStatuses": [{
                "name": "app", "image": "ghcr.io/org/api:2", "imageID": "", "ready": false, "restartCount": 0,
                "state": { "waiting": { "reason": "ImagePullBackOff" } } }] }
        }))
        .unwrap();
        let objs = ClusterObjects {
            pods: vec![stuck],
            deployments: vec![deploy("web", Some("web-sa"), "ghcr.io/org/web:1"), deploy("api", None, "ghcr.io/org/api:2"), deploy("cache", None, "redis:7")],
            secrets: vec![secret],
            service_accounts: vec![sa],
            role_bindings: vec![rb],
            ..Default::default()
        };
        let snap = build_snapshot("c", &objs, &UsageMetrics::default(), 0);

        let sa = &snap.service_accounts[0];
        assert_eq!(sa.image_pull_secrets, ["ghcr", "gone"]);
        assert_eq!(sa.missing_pull_secrets, ["gone"]);
        assert_eq!(sa.roles, ["Role/reader (RoleBinding web-rb)"]);
        assert_eq!(sa.used_by, ["Deployment/web"]);

        let hosts: Vec<&str> = snap.registries.iter().map(|r| r.host.as_str()).collect();
        assert_eq!(hosts, ["docker.io", "ghcr.io"]);
        let ghcr = &snap.registries[1];
        assert_eq!(ghcr.images, ["org/api:2", "org/web:1"]);
        assert_eq!(ghcr.credentials.len(), 1);
        assert_eq!((ghcr.credentials[0].username.as_deref(), ghcr.credentials[0].service_accounts.clone()), (Some("alice"), vec!["web-sa".to_string()]));
        assert_eq!(ghcr.without_credentials, ["apps/Deployment/api"], "web gets ghcr through its account");
        assert_eq!(ghcr.pull_errors, ["apps/api-1 (app): ghcr.io/org/api:2"]);

        let secret = snap.configs.iter().find(|c| c.name == "ghcr").unwrap();
        assert_eq!(secret.used_by, ["ServiceAccount/web-sa"]);
        assert!(crate::issues::detect(&snap, &Default::default(), 0).iter().any(|i| i.key == "missing-pull-secret:ServiceAccount/apps/web-sa"));
    }

    #[test]
    fn backups_take_their_state_from_the_newest_finished_run() {
        use crate::backup::{backup_claim, backup_cronjob, sample_spec};
        let spec = sample_spec();
        let mut cron = backup_cronjob(&spec);
        cron.metadata.creation_timestamp = Some(Time("2026-10-01T00:00:00Z".parse().unwrap()));
        cron.status = Some(serde_json::from_value(serde_json::json!({ "lastSuccessfulTime": "2026-10-09T02:01:00Z" })).unwrap());
        let job = |name: &str, created: &str, outcome: &str, owner: &str| -> Job {
            serde_json::from_value(serde_json::json!({
                "metadata": {
                    "name": name, "namespace": "shop", "creationTimestamp": created,
                    "ownerReferences": [{ "apiVersion": "batch/v1", "kind": "CronJob", "name": owner, "uid": "u" }]
                },
                "status": { "conditions": [{ "type": outcome, "status": "True", "message": format!("{outcome} run") }] }
            }))
            .unwrap()
        };
        let mut claim = backup_claim(&spec);
        claim.status = Some(serde_json::from_value(serde_json::json!({ "phase": "Bound", "capacity": { "storage": "1Gi" } })).unwrap());
        let web: Deployment = serde_json::from_value(serde_json::json!({
            "metadata": { "name": "web", "namespace": "shop", "annotations": { crate::backup::OFFLINE_ANNOTATION: "uploads", DISABLED_REPLICAS_ANNOTATION: "2" } },
            "spec": { "replicas": 0, "selector": {}, "template": { "spec": { "containers": [] } } }
        }))
        .unwrap();
        let objs = ClusterObjects {
            cronjobs: vec![cron],
            pvcs: vec![claim],
            deployments: vec![web],
            jobs: vec![
                job("portside-backup-uploads-1", "2026-10-09T02:00:00Z", "Complete", "portside-backup-uploads"),
                job("portside-backup-uploads-2", "2026-10-10T02:00:00Z", "Failed", "portside-backup-uploads"),
                // Another CronJob's newer run doesn't count.
                job("other-3", "2026-10-11T02:00:00Z", "Complete", "other"),
            ],
            ..Default::default()
        };
        let snap = build_snapshot("c", &objs, &UsageMetrics::default(), 0);
        let b = &snap.backups[0];
        assert_eq!((b.name.as_str(), b.cron_job.as_str()), ("uploads", "portside-backup-uploads"));
        assert_eq!(b.spec.as_ref(), Some(&spec));
        assert_eq!(b.last_failure.as_deref(), Some("Failed run"));
        assert!(b.last_failure_ms > b.last_successful_ms, "the failure is newer than the last success");
        assert_eq!((b.claim_phase.as_deref(), b.claim_capacity.as_deref()), (Some("Bound"), Some("1Gi")));
        let web = snap.workloads.iter().find(|w| w.name == "web").unwrap();
        assert_eq!((web.backup_offline.as_deref(), web.disabled_replicas), (Some("uploads"), Some(2)));
    }
}
