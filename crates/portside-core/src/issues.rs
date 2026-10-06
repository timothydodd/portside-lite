//! Problem detection. Each rule looks at the flattened snapshot and emits
//! zero or more [`Issue`]s with a stable key, so the store can track when a
//! problem first appeared and when it resolved.

use std::collections::HashMap;

use crate::models::*;
use crate::settings::Settings;

/// Grace period before a Pending / not-ready pod counts as a problem.
const STARTUP_GRACE_MS: i64 = 5 * 60 * 1000;
/// Terminating longer than this is "stuck".
const STUCK_TERMINATING_MS: i64 = 5 * 60 * 1000;
/// OOMKills older than this are history, not a live problem.
const RECENT_OOM_MS: i64 = 60 * 60 * 1000;
/// Warning events older than this are ignored for issue purposes.
const EVENT_WINDOW_MS: i64 = 30 * 60 * 1000;

/// Waiting reasons that mean the container will not start without help.
const FATAL_WAITING: &[(&str, &str)] = &[
    ("CrashLoopBackOff", "The container keeps crashing on start. Check the previous container's logs for the crash reason."),
    ("ImagePullBackOff", "The image can't be pulled. Check the image name/tag, registry reachability and imagePullSecrets."),
    ("ErrImagePull", "The image can't be pulled. Check the image name/tag, registry reachability and imagePullSecrets."),
    ("InvalidImageName", "The image reference is malformed. Fix the image field in the workload spec."),
    ("CreateContainerConfigError", "A referenced ConfigMap or Secret (or a key in it) is missing."),
    ("CreateContainerError", "The runtime couldn't create the container. The message usually names the cause."),
    ("RunContainerError", "The runtime couldn't start the container, often a bad command/entrypoint or mount."),
];

/// Warning event reasons worth raising on their own — they describe problems
/// that aren't visible from object status alone.
const EVENT_REASONS: &[(&str, &str)] = &[
    ("FailedMount", "A volume can't be mounted. Check the PVC/Secret/ConfigMap it references and the storage provisioner."),
    ("FailedAttachVolume", "The volume can't attach to the node. Check the storage provisioner and whether another node holds it."),
    ("Unhealthy", "Liveness/readiness probes are failing. Check the probe endpoint and the app's startup time."),
    ("FailedCreatePodSandBox", "The pod sandbox can't be created, usually a CNI/networking problem on the node."),
    ("FailedCreate", "The controller can't create pods. Often quota, admission or a bad pod template."),
    ("NodeNotReady", "A node stopped reporting ready."),
    ("Evicted", "A pod was evicted, usually from node resource pressure."),
    ("OOMKilling", "The kernel OOM killer fired on the node."),
    ("FreeDiskSpaceFailed", "The kubelet couldn't free disk space on the node."),
    ("ImageGCFailed", "Image garbage collection failed on the node — disk may be filling."),
];

fn fmt_pct(v: f64) -> String {
    format!("{:.0}%", v)
}

fn pod_issue(
    rule: &str,
    sub: Option<&str>,
    severity: Severity,
    p: &PodInfo,
    title: String,
    detail: String,
    hint: Option<&str>,
    since_ms: Option<i64>,
    actions: Vec<ActionKind>,
) -> Issue {
    let key = match sub {
        Some(s) => format!("{rule}:Pod/{}/{}/{s}", p.namespace, p.name),
        None => format!("{rule}:Pod/{}/{}", p.namespace, p.name),
    };
    Issue {
        key,
        severity,
        category: "pod".into(),
        rule: rule.into(),
        kind: "Pod".into(),
        namespace: Some(p.namespace.clone()),
        name: p.name.clone(),
        title,
        detail,
        hint: hint.map(str::to_string),
        since_ms,
        actions,
        first_seen_ms: None,
    }
}

fn detect_pods(snap: &ClusterSnapshot, settings: &Settings, now: i64, out: &mut Vec<Issue>) {
    for p in &snap.pods {
        let age = p.created_ms.map(|c| now - c).unwrap_or(i64::MAX);

        if let Some(since) = p.deleting_since_ms {
            if now - since > STUCK_TERMINATING_MS {
                out.push(pod_issue(
                    "stuck-terminating",
                    None,
                    Severity::Warning,
                    p,
                    format!("{} stuck terminating", p.name),
                    "Pod has been terminating for more than 5 minutes.".into(),
                    Some("Usually a finalizer or an unreachable node. Check the node, then force-delete if safe."),
                    Some(since),
                    vec![ActionKind::DeletePod],
                ));
            }
            continue;
        }

        let mut container_problem = false;
        for c in &p.containers {
            if c.state == "waiting" {
                if let Some(reason) = &c.reason {
                    if let Some((_, hint)) = FATAL_WAITING.iter().find(|(r, _)| r == reason) {
                        container_problem = true;
                        let mut actions = vec![ActionKind::ViewLogs];
                        if reason == "CrashLoopBackOff" {
                            actions.insert(0, ActionKind::ViewPreviousLogs);
                        }
                        actions.push(ActionKind::DeletePod);
                        let detail = match (&c.message, c.last_terminated_exit_code) {
                            (Some(m), _) => m.clone(),
                            (None, Some(code)) => format!(
                                "Container '{}' last exited with code {} ({}); {} restarts.",
                                c.name,
                                code,
                                c.last_terminated_reason.as_deref().unwrap_or("Error"),
                                c.restarts
                            ),
                            _ => format!("Container '{}' is waiting: {}.", c.name, reason),
                        };
                        out.push(pod_issue(
                            &reason.to_ascii_lowercase(),
                            Some(&c.name),
                            Severity::Critical,
                            p,
                            format!("{reason}: {}", p.name),
                            detail,
                            Some(hint),
                            c.last_terminated_ms,
                            actions,
                        ));
                    }
                }
            }

            if c.last_terminated_reason.as_deref() == Some("OOMKilled")
                && c.last_terminated_ms.is_some_and(|t| now - t < RECENT_OOM_MS)
            {
                container_problem = true;
                let limit = if p.mem_limits > 0.0 {
                    format!(" (pod memory limit {:.0} MiB)", p.mem_limits / 1048576.0)
                } else {
                    String::new()
                };
                out.push(pod_issue(
                    "oom-killed",
                    Some(&c.name),
                    Severity::Warning,
                    p,
                    format!("OOMKilled: {}", p.name),
                    format!("Container '{}' was killed for exceeding its memory{limit}.", c.name),
                    Some("Raise the memory limit or find the leak. Check memory history for this pod in Analytics."),
                    c.last_terminated_ms,
                    vec![ActionKind::ViewPreviousLogs, ActionKind::ViewLogs],
                ));
            }
        }

        if p.phase == "Pending" && age > STARTUP_GRACE_MS && !container_problem {
            let unschedulable = p
                .conditions
                .iter()
                .find(|c| c.type_ == "PodScheduled" && c.status == "False");
            match unschedulable {
                Some(c) => out.push(pod_issue(
                    "unschedulable",
                    None,
                    Severity::Critical,
                    p,
                    format!("Unschedulable: {}", p.name),
                    c.message.clone().unwrap_or_else(|| "No node can run this pod.".into()),
                    Some("Not enough free CPU/memory, or node selectors/taints/affinity exclude every node."),
                    c.last_transition_ms.or(p.created_ms),
                    vec![ActionKind::DeletePod],
                )),
                None => out.push(pod_issue(
                    "pending",
                    None,
                    Severity::Warning,
                    p,
                    format!("Pending: {}", p.name),
                    format!("Pod has been Pending for {} min.", age / 60000),
                    Some("Check its events — often waiting on a volume or image pull."),
                    p.created_ms,
                    vec![ActionKind::DeletePod],
                )),
            }
        }

        if p.phase == "Failed" && p.owner_kind.as_deref() != Some("Job") {
            let evicted = p.status == "Evicted";
            out.push(pod_issue(
                if evicted { "evicted" } else { "failed" },
                None,
                Severity::Warning,
                p,
                format!("{}: {}", if evicted { "Evicted" } else { "Failed" }, p.name),
                format!("Pod is in phase Failed (status {}).", p.status),
                Some(if evicted {
                    "The node ran short on memory or disk. Check node pressure; delete the pod to clean up."
                } else {
                    "Check the pod's containers and events for the failure reason."
                }),
                p.created_ms,
                vec![ActionKind::ViewLogs, ActionKind::DeletePod],
            ));
        }

        if p.phase == "Running"
            && !container_problem
            && p.ready_containers < p.total_containers
            && age > STARTUP_GRACE_MS
        {
            let since = p
                .conditions
                .iter()
                .find(|c| c.type_ == "Ready")
                .and_then(|c| c.last_transition_ms);
            if since.map(|s| now - s > STARTUP_GRACE_MS).unwrap_or(true) {
                out.push(pod_issue(
                    "not-ready",
                    None,
                    Severity::Warning,
                    p,
                    format!("Not ready: {}", p.name),
                    format!(
                        "{}/{} containers ready.",
                        p.ready_containers, p.total_containers
                    ),
                    Some("A readiness probe is failing, so the pod receives no Service traffic."),
                    since,
                    vec![ActionKind::ViewLogs, ActionKind::DeletePod],
                ));
            }
        }

        if !container_problem
            && p.restarts >= settings.restart_warning_threshold
            && p.phase == "Running"
        {
            out.push(pod_issue(
                "restarts",
                None,
                Severity::Info,
                p,
                format!("{} restarts: {}", p.restarts, p.name),
                "Pod is running now but has restarted repeatedly.".into(),
                Some("Look at the previous container's logs around the last restart."),
                p.last_restart_ms,
                vec![ActionKind::ViewPreviousLogs, ActionKind::ViewLogs],
            ));
        }

        if let (Some(mem), true) = (p.mem_usage, p.mem_limits > 0.0) {
            let pct = mem / p.mem_limits * 100.0;
            if pct >= settings.high_usage_percent {
                out.push(pod_issue(
                    "near-memory-limit",
                    None,
                    Severity::Warning,
                    p,
                    format!("Near memory limit: {}", p.name),
                    format!("Using {} of its memory limit; it will be OOMKilled at 100%.", fmt_pct(pct)),
                    Some("Raise the limit or reduce usage before it gets killed."),
                    None,
                    vec![ActionKind::ViewLogs],
                ));
            }
        }
    }
}

fn node_issue(rule: &str, severity: Severity, n: &NodeInfo, title: String, detail: String, hint: &str, since: Option<i64>, actions: Vec<ActionKind>) -> Issue {
    Issue {
        key: format!("{rule}:Node/{}", n.name),
        severity,
        category: "node".into(),
        rule: rule.into(),
        kind: "Node".into(),
        namespace: None,
        name: n.name.clone(),
        title,
        detail,
        hint: Some(hint.into()),
        since_ms: since,
        actions,
        first_seen_ms: None,
    }
}

fn detect_nodes(snap: &ClusterSnapshot, settings: &Settings, out: &mut Vec<Issue>) {
    for n in &snap.nodes {
        if !n.ready {
            let c = n.conditions.iter().find(|c| c.type_ == "Ready");
            out.push(node_issue(
                "node-not-ready",
                Severity::Critical,
                n,
                format!("Node not ready: {}", n.name),
                c.and_then(|c| c.message.clone())
                    .unwrap_or_else(|| "The kubelet isn't reporting ready.".into()),
                "Check the k3s service on the node (systemctl status k3s / k3s-agent), network and disk.",
                c.and_then(|c| c.last_transition_ms),
                vec![],
            ));
        }
        for (cond, sev, hint) in [
            ("MemoryPressure", Severity::Critical, "The node is low on memory and will start evicting pods."),
            ("DiskPressure", Severity::Critical, "The node is low on disk and will evict pods / refuse images. Prune images and logs."),
            ("PIDPressure", Severity::Warning, "The node is running out of process IDs."),
            ("NetworkUnavailable", Severity::Critical, "The node's network isn't configured. Check flannel/CNI."),
        ] {
            if let Some(c) = n.conditions.iter().find(|c| c.type_ == cond && c.status == "True") {
                out.push(node_issue(
                    &cond.to_ascii_lowercase(),
                    sev,
                    n,
                    format!("{cond}: {}", n.name),
                    c.message.clone().unwrap_or_default(),
                    hint,
                    c.last_transition_ms,
                    vec![],
                ));
            }
        }
        if n.unschedulable {
            out.push(node_issue(
                "cordoned",
                Severity::Info,
                n,
                format!("Cordoned: {}", n.name),
                "No new pods will be scheduled on this node.".into(),
                "Uncordon it once maintenance is done.",
                None,
                vec![ActionKind::Uncordon],
            ));
        }
        if let Some(cpu) = n.cpu_usage {
            if n.cpu_allocatable > 0.0 {
                let pct = cpu / n.cpu_allocatable * 100.0;
                if pct >= settings.high_usage_percent {
                    out.push(node_issue(
                        "node-high-cpu",
                        Severity::Warning,
                        n,
                        format!("High CPU: {}", n.name),
                        format!("CPU at {} of allocatable.", fmt_pct(pct)),
                        "Find the top consumers on the Pods page (sort by CPU).",
                        None,
                        vec![],
                    ));
                }
            }
        }
        if let Some(mem) = n.mem_usage {
            if n.mem_allocatable > 0.0 {
                let pct = mem / n.mem_allocatable * 100.0;
                if pct >= settings.high_usage_percent {
                    out.push(node_issue(
                        "node-high-memory",
                        Severity::Warning,
                        n,
                        format!("High memory: {}", n.name),
                        format!("Memory at {} of allocatable.", fmt_pct(pct)),
                        "Find the top consumers on the Pods page (sort by memory). Evictions start under pressure.",
                        None,
                        vec![],
                    ));
                }
            }
        }
        if n.cpu_allocatable > 0.0 && n.cpu_requests / n.cpu_allocatable > 1.0 {
            // Requests can't exceed allocatable for scheduled pods, but static
            // pods and races can push it over; flag as capacity planning info.
            out.push(node_issue(
                "node-overcommitted",
                Severity::Info,
                n,
                format!("CPU requests over capacity: {}", n.name),
                format!("Requests total {:.2} of {:.2} cores.", n.cpu_requests, n.cpu_allocatable),
                "Lower requests or add capacity.",
                None,
                vec![],
            ));
        }
    }
}

fn workload_issue(rule: &str, severity: Severity, w: &WorkloadInfo, title: String, detail: String, hint: &str, actions: Vec<ActionKind>) -> Issue {
    Issue {
        key: format!("{rule}:{}/{}/{}", w.kind, w.namespace, w.name),
        severity,
        category: "workload".into(),
        rule: rule.into(),
        kind: w.kind.clone(),
        namespace: Some(w.namespace.clone()),
        name: w.name.clone(),
        title,
        detail,
        hint: Some(hint.into()),
        since_ms: None,
        actions,
        first_seen_ms: None,
    }
}

/// A pod that's young and coming up without trouble (no restarts, nothing
/// stuck on an image or config error).
fn starting_cleanly(p: &PodInfo, now: i64) -> bool {
    p.created_ms.is_some_and(|c| now - c < STARTUP_GRACE_MS)
        && p.deleting_since_ms.is_none()
        && p.containers.iter().all(|c| {
            c.restarts == 0
                && (c.state != "waiting" || matches!(c.reason.as_deref(), None | Some("ContainerCreating" | "PodInitializing")))
                && c.state != "terminated"
        })
}

/// Not ready yet for an ordinary reason: it was just created, or its pods
/// were just replaced (rollout, restart, a deleted pod) and are starting
/// cleanly. Pods in real trouble raise their own issues straight away.
fn settling(w: &WorkloadInfo, snap: &ClusterSnapshot, now: i64) -> bool {
    if w.created_ms.is_some_and(|c| now - c < STARTUP_GRACE_MS) {
        return true;
    }
    let mut pods = snap
        .pods
        .iter()
        .filter(|p| p.namespace == w.namespace && p.owner_kind.as_deref() == Some(w.kind.as_str()) && p.owner_name.as_deref() == Some(w.name.as_str()))
        .filter(|p| p.ready_containers < p.total_containers || p.total_containers == 0)
        .peekable();
    pods.peek().is_some() && pods.all(|p| starting_cleanly(p, now))
}

/// A failed run of a CronJob that has since had a successful one.
fn superseded(job: &WorkloadInfo, snap: &ClusterSnapshot) -> bool {
    let Some((parent, _)) = job.name.rsplit_once('-') else { return false };
    let sibling = |w: &&WorkloadInfo| w.kind == "Job" && w.namespace == job.namespace && w.name.rsplit_once('-').is_some_and(|(p, _)| p == parent);
    snap.workloads.iter().any(|w| w.kind == "CronJob" && w.namespace == job.namespace && w.name == parent)
        && snap.workloads.iter().filter(sibling).any(|w| w.created_ms > job.created_ms && w.condition_message.is_none() && w.ready >= w.desired.max(1))
}

fn detect_workloads(snap: &ClusterSnapshot, now: i64, out: &mut Vec<Issue>) {
    for w in &snap.workloads {
        match w.kind.as_str() {
            "Deployment" | "StatefulSet" | "DaemonSet" => {
                if w.desired == 0 || w.ready >= w.desired {
                    if let Some(msg) = &w.condition_message {
                        out.push(workload_issue(
                            "rollout-stalled",
                            Severity::Warning,
                            w,
                            format!("Rollout problem: {}", w.name),
                            msg.clone(),
                            "The latest rollout isn't progressing. Check the new pods' status and events.",
                            vec![ActionKind::RolloutRestart],
                        ));
                    }
                    continue;
                }
                if settling(w, snap, now) {
                    continue;
                }
                let severity = if w.ready == 0 { Severity::Critical } else { Severity::Warning };
                let mut detail = format!("{}/{} ready.", w.ready, w.desired);
                if let Some(m) = &w.condition_message {
                    detail.push(' ');
                    detail.push_str(m);
                }
                let mut actions = vec![ActionKind::RolloutRestart];
                if w.kind != "DaemonSet" {
                    actions.push(ActionKind::Scale);
                }
                out.push(workload_issue(
                    if w.ready == 0 { "workload-down" } else { "workload-degraded" },
                    severity,
                    w,
                    format!(
                        "{} {}: {}",
                        w.kind,
                        if w.ready == 0 { "down" } else { "degraded" },
                        w.name
                    ),
                    detail,
                    "Its pods are listed under Problems too — fix those first.",
                    actions,
                ));
            }
            "Job" => {
                if let Some(msg) = w.condition_message.as_ref().filter(|_| !superseded(w, snap)) {
                    out.push(workload_issue(
                        "job-failed",
                        Severity::Warning,
                        w,
                        format!("Job failed: {}", w.name),
                        format!("{msg} ({} failed pods).", w.failed),
                        "Check the failed pods' logs. Delete the Job once handled so it stops showing.",
                        vec![],
                    ));
                }
            }
            _ => {}
        }
    }

    for v in &snap.volumes {
        // Only say a class is missing when we could list classes at all.
        let missing_class = v
            .storage_class
            .as_deref()
            .filter(|c| !snap.storage_classes.is_empty() && !snap.storage_classes.iter().any(|sc| sc.name == *c));
        let pending_detail = match missing_class {
            Some(c) => format!("Claim isn't bound to a volume. StorageClass \"{c}\" doesn't exist."),
            None => "Claim isn't bound to a volume.".to_string(),
        };
        let (sev, rule, detail, hint) = match v.phase.as_str() {
            "Pending" => (
                Severity::Warning,
                "pvc-pending",
                pending_detail.as_str(),
                if missing_class.is_some() {
                    "Point the claim at a class that exists (see Storage → Storage classes), or create the class."
                } else {
                    "Check the StorageClass exists and its provisioner (local-path on k3s) is running."
                },
            ),
            "Lost" => (
                Severity::Critical,
                "pvc-lost",
                "The bound PersistentVolume no longer exists.",
                "Data may be gone. Restore the PV or recreate the claim.",
            ),
            _ => continue,
        };
        out.push(Issue {
            key: format!("{rule}:PersistentVolumeClaim/{}/{}", v.namespace, v.name),
            severity: sev,
            category: "storage".into(),
            rule: rule.into(),
            kind: "PersistentVolumeClaim".into(),
            namespace: Some(v.namespace.clone()),
            name: v.name.clone(),
            title: format!("PVC {}: {}", v.phase, v.name),
            detail: detail.into(),
            hint: Some(hint.into()),
            since_ms: v.created_ms,
            actions: vec![],
            first_seen_ms: None,
        });
    }

    for pv in &snap.persistent_volumes {
        let released_for = pv.phase_since_ms.map(|t| now - t);
        let (sev, rule, title, detail, hint) = match (pv.phase.as_str(), pv.reclaim_policy.as_deref()) {
            ("Failed", _) => (
                Severity::Warning,
                "pv-failed",
                format!("PV failed: {}", pv.name),
                pv.message.clone().unwrap_or_else(|| "Reclaiming the volume failed.".into()),
                "The provisioner couldn't clean it up. Check its logs (local-path-provisioner on k3s), then remove the data and the PV by hand.",
            ),
            // Delete policy: the provisioner should remove it within moments.
            ("Released", Some("Delete")) if released_for.is_some_and(|t| t > STARTUP_GRACE_MS) => (
                Severity::Warning,
                "pv-released-stuck",
                format!("PV not cleaned up: {}", pv.name),
                format!("Its claim {} is gone but the volume wasn't deleted.", pv.claim.as_deref().unwrap_or("?")),
                "Check the provisioner's logs (local-path-provisioner on k3s).",
            ),
            ("Released", Some("Retain")) => (
                Severity::Info,
                "pv-released",
                format!("PV kept after its claim was deleted: {}", pv.name),
                format!(
                    "{} from {} is still on disk{}.",
                    pv.capacity.as_deref().unwrap_or("Its data"),
                    pv.claim.as_deref().unwrap_or("a deleted claim"),
                    pv.node.as_deref().map(|n| format!(" on {n}")).unwrap_or_default()
                ),
                "Reclaim policy is Retain, so nothing removes it. Delete the PV (and its data) once you're sure it isn't needed.",
            ),
            _ => continue,
        };
        out.push(Issue {
            key: format!("{rule}:PersistentVolume/{}", pv.name),
            severity: sev,
            category: "storage".into(),
            rule: rule.into(),
            kind: "PersistentVolume".into(),
            namespace: None,
            name: pv.name.clone(),
            title,
            detail,
            hint: Some(hint.into()),
            since_ms: pv.phase_since_ms,
            actions: vec![],
            first_seen_ms: None,
        });
    }

    for h in &snap.autoscalers {
        let (sev, rule, title, detail, hint) = if let Some(problem) = &h.problem {
            // Metrics aren't there for a new HPA or freshly started pods yet.
            if h.created_ms.is_some_and(|c| now - c < STARTUP_GRACE_MS) {
                continue;
            }
            (
                Severity::Warning,
                "hpa-cannot-scale",
                format!("Autoscaler can't scale: {}", h.name),
                problem.clone(),
                "Usually missing metrics: check metrics-server is running and the target's containers set resource requests.",
            )
        } else if h.at_max {
            (
                Severity::Info,
                "hpa-at-max",
                format!("Autoscaler at its maximum: {}", h.name),
                format!("{}/{} wants more than {} replicas ({}).", h.target_kind, h.target_name, h.max_replicas, h.metrics.join(", ")),
                "Load is above what max replicas allows. Raise maxReplicas or give the pods more resources.",
            )
        } else {
            continue;
        };
        out.push(Issue {
            key: format!("{rule}:HorizontalPodAutoscaler/{}/{}", h.namespace, h.name),
            severity: sev,
            category: "workload".into(),
            rule: rule.into(),
            kind: "HorizontalPodAutoscaler".into(),
            namespace: Some(h.namespace.clone()),
            name: h.name.clone(),
            title,
            detail,
            hint: Some(hint.into()),
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        });
    }
}

/// Ingress rules that send traffic to a Service that doesn't exist.
fn detect_ingresses(snap: &ClusterSnapshot, out: &mut Vec<Issue>) {
    for ing in &snap.ingresses {
        let mut missing: Vec<&str> = ing.routes.iter().filter(|r| !r.service_found).filter_map(|r| r.service.as_deref()).collect();
        missing.sort();
        missing.dedup();
        if missing.is_empty() {
            continue;
        }
        out.push(Issue {
            key: format!("ingress-missing-backend:Ingress/{}/{}", ing.namespace, ing.name),
            severity: Severity::Warning,
            category: "network".into(),
            rule: "ingress-missing-backend".into(),
            kind: "Ingress".into(),
            namespace: Some(ing.namespace.clone()),
            name: ing.name.clone(),
            title: format!("Ingress routes to a missing Service: {}", ing.name),
            detail: format!("No Service named {} in {}, so those routes return errors.", missing.join(", "), ing.namespace),
            hint: Some("Fix the backend service name in the Ingress, or deploy the Service it expects.".into()),
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        });
    }
}

/// Services whose selector matches no running pod, or none that is Ready:
/// traffic to them fails even though the Service itself looks fine.
fn detect_services(snap: &ClusterSnapshot, out: &mut Vec<Issue>) {
    for s in &snap.services {
        if s.type_ == "ExternalName" || s.selector.is_empty() || s.idle {
            continue;
        }
        let (rule, detail, hint) = if s.pods_matched == 0 {
            (
                "service-no-pods",
                "Its selector matches no running pod, so requests have nowhere to go.".to_string(),
                "Compare the Service's selector with the pod labels: usually a typo, or the workload behind it is gone.",
            )
        } else if s.pods_ready == 0 {
            (
                "service-no-ready-endpoints",
                format!("Selects {} pod(s) but none are Ready, so it has no endpoints.", s.pods_matched),
                "Fix the backing pods first (readiness probe, crash); see their problems.",
            )
        } else {
            continue;
        };
        out.push(Issue {
            key: format!("{rule}:Service/{}/{}", s.namespace, s.name),
            severity: Severity::Warning,
            category: "network".into(),
            rule: rule.into(),
            kind: "Service".into(),
            namespace: Some(s.namespace.clone()),
            name: s.name.clone(),
            title: format!("No endpoints: Service {}", s.name),
            detail,
            hint: Some(hint.into()),
            since_ms: None,
            actions: vec![],
            first_seen_ms: None,
        });
    }
}

/// A readiness/startup probe that failed only while the pod's containers were
/// starting (scale-up, rollout): the probe ran before the app was listening.
/// If it keeps failing past the grace period, later events fall outside the
/// window and the issue shows up. Liveness failures always count.
fn startup_probe_noise(e: &EventInfo, snap: &ClusterSnapshot) -> bool {
    if e.reason != "Unhealthy" || e.object_kind != "Pod" {
        return false;
    }
    if !(e.message.starts_with("Readiness probe") || e.message.starts_with("Startup probe")) {
        return false;
    }
    let Some(p) = snap.pods.iter().find(|p| p.namespace == e.namespace && p.name == e.object_name) else { return false };
    // A restarted container probes from scratch, so measure from the latest start.
    let started = p.containers.iter().filter(|c| !c.init).filter_map(|c| c.started_ms).max().or(p.created_ms);
    match (started, e.last_ms) {
        (Some(s), Some(last)) => last - s <= STARTUP_GRACE_MS,
        _ => false,
    }
}

fn detect_events(snap: &ClusterSnapshot, now: i64, out: &mut Vec<Issue>) {
    // Group by (object, reason) so a flapping probe is one issue, not fifty.
    // (count, newest event, earliest first-seen)
    let mut groups: HashMap<(String, String, String, String), (i32, &EventInfo, Option<i64>)> = HashMap::new();
    for e in &snap.events {
        if e.last_ms.map(|t| now - t > EVENT_WINDOW_MS).unwrap_or(true) {
            continue;
        }
        // A pod that's gone (replaced in a rollout, deleted) can't be fixed or opened.
        if e.object_kind == "Pod" && !snap.pods.iter().any(|p| p.namespace == e.namespace && p.name == e.object_name) {
            continue;
        }
        if !EVENT_REASONS.iter().any(|(r, _)| *r == e.reason) || startup_probe_noise(e, snap) {
            continue;
        }
        let key = (
            e.object_kind.clone(),
            e.namespace.clone(),
            e.object_name.clone(),
            e.reason.clone(),
        );
        let entry = groups.entry(key).or_insert((0, e, e.first_ms));
        entry.0 += e.count;
        if e.last_ms > entry.1.last_ms {
            entry.1 = e;
        }
        entry.2 = match (entry.2, e.first_ms) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
    }
    for ((kind, ns, name, reason), (count, e, since)) in groups {
        let hint = EVENT_REASONS.iter().find(|(r, _)| *r == reason).map(|(_, h)| *h);
        let actions = if kind == "Pod" {
            vec![ActionKind::ViewLogs, ActionKind::DeletePod]
        } else {
            vec![]
        };
        out.push(Issue {
            key: format!("event-{}:{kind}/{ns}/{name}", reason.to_ascii_lowercase()),
            severity: Severity::Warning,
            category: "event".into(),
            rule: format!("event-{}", reason.to_ascii_lowercase()),
            kind,
            namespace: (!ns.is_empty()).then_some(ns),
            name: name.clone(),
            title: format!("{reason}: {name}"),
            detail: format!("{} ({}× in the last 30 min)", e.message, count),
            hint: hint.map(str::to_string),
            since_ms: since,
            actions,
            first_seen_ms: None,
        });
    }
}

/// Run every rule over a snapshot. Issues come back sorted by severity, then
/// namespace/name, and with duplicate keys removed.
pub fn detect(snap: &ClusterSnapshot, settings: &Settings, now_ms: i64) -> Vec<Issue> {
    let mut out = Vec::new();
    detect_nodes(snap, settings, &mut out);
    detect_pods(snap, settings, now_ms, &mut out);
    detect_workloads(snap, now_ms, &mut out);
    detect_services(snap, &mut out);
    detect_ingresses(snap, &mut out);
    detect_events(snap, now_ms, &mut out);
    sort_dedupe(&mut out);
    out
}

/// Error-log volume per pod over the last hour → issues.
/// `counts` is `(namespace, pod, error_lines_last_hour)`.
pub fn log_spike_issues(counts: &[(String, String, i64)], settings: &Settings) -> Vec<Issue> {
    counts
        .iter()
        .filter(|(_, _, n)| *n >= settings.error_log_spike_per_hour)
        .map(|(ns, pod, n)| Issue {
            key: format!("log-errors:Pod/{ns}/{pod}"),
            severity: Severity::Warning,
            category: "logs".into(),
            rule: "log-errors".into(),
            kind: "Pod".into(),
            namespace: Some(ns.clone()),
            name: pod.clone(),
            title: format!("Error log spike: {pod}"),
            detail: format!("{n} error-level log lines in the last hour."),
            hint: Some("Open its logs filtered to errors to see what's failing.".into()),
            since_ms: None,
            actions: vec![ActionKind::ViewLogs],
            first_seen_ms: None,
        })
        .collect()
}

pub fn sort_dedupe(issues: &mut Vec<Issue>) {
    issues.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.namespace.cmp(&b.namespace))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.key.cmp(&b.key))
    });
    issues.dedup_by(|a, b| a.key == b.key);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod(name: &str) -> PodInfo {
        PodInfo {
            namespace: "default".into(),
            name: name.into(),
            phase: "Running".into(),
            status: "Running".into(),
            created_ms: Some(0),
            total_containers: 1,
            ready_containers: 1,
            ..Default::default()
        }
    }

    const NOW: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn crashloop_is_critical_with_previous_logs_action() {
        let mut p = pod("api");
        p.ready_containers = 0;
        p.containers.push(ContainerInfo {
            name: "app".into(),
            state: "waiting".into(),
            reason: Some("CrashLoopBackOff".into()),
            restarts: 12,
            last_terminated_exit_code: Some(1),
            ..Default::default()
        });
        let snap = ClusterSnapshot { pods: vec![p], ..Default::default() };
        let issues = detect(&snap, &Settings::default(), NOW);
        assert_eq!(issues.len(), 1, "{issues:#?}");
        assert_eq!(issues[0].severity, Severity::Critical);
        assert_eq!(issues[0].actions[0], ActionKind::ViewPreviousLogs);
        assert!(issues[0].detail.contains("exited with code 1"));
    }

    #[test]
    fn pending_within_grace_is_ignored() {
        let mut p = pod("new");
        p.phase = "Pending".into();
        p.created_ms = Some(NOW - 60_000);
        let snap = ClusterSnapshot { pods: vec![p], ..Default::default() };
        assert!(detect(&snap, &Settings::default(), NOW).is_empty());
    }

    #[test]
    fn unschedulable_uses_condition_message() {
        let mut p = pod("big");
        p.phase = "Pending".into();
        p.conditions.push(ConditionInfo {
            type_: "PodScheduled".into(),
            status: "False".into(),
            message: Some("0/1 nodes are available: 1 Insufficient memory.".into()),
            ..Default::default()
        });
        let snap = ClusterSnapshot { pods: vec![p], ..Default::default() };
        let issues = detect(&snap, &Settings::default(), NOW);
        assert_eq!(issues[0].rule, "unschedulable");
        assert!(issues[0].detail.contains("Insufficient memory"));
    }

    #[test]
    fn node_not_ready_and_pressure() {
        let n = NodeInfo {
            name: "n1".into(),
            ready: false,
            conditions: vec![ConditionInfo {
                type_: "DiskPressure".into(),
                status: "True".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let snap = ClusterSnapshot { nodes: vec![n], ..Default::default() };
        let rules: Vec<_> = detect(&snap, &Settings::default(), NOW)
            .into_iter()
            .map(|i| i.rule)
            .collect();
        assert!(rules.contains(&"node-not-ready".to_string()));
        assert!(rules.contains(&"diskpressure".to_string()));
    }

    #[test]
    fn degraded_deployment() {
        let w = WorkloadInfo {
            kind: "Deployment".into(),
            namespace: "default".into(),
            name: "web".into(),
            desired: 3,
            ready: 1,
            ..Default::default()
        };
        let snap = ClusterSnapshot { workloads: vec![w], ..Default::default() };
        let i = &detect(&snap, &Settings::default(), NOW)[0];
        assert_eq!(i.rule, "workload-degraded");
        assert_eq!(i.severity, Severity::Warning);
    }

    #[test]
    fn events_group_by_object_and_reason() {
        let ev = |count| EventInfo {
            namespace: "default".into(),
            object_kind: "Pod".into(),
            object_name: "api".into(),
            reason: "Unhealthy".into(),
            message: "Readiness probe failed".into(),
            type_: "Warning".into(),
            count,
            last_ms: Some(NOW - 1000),
            ..Default::default()
        };
        let snap = ClusterSnapshot { pods: vec![pod("api")], events: vec![ev(3), ev(4)], ..Default::default() };
        let issues = detect(&snap, &Settings::default(), NOW);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].detail.contains("7×"));
    }

    #[test]
    fn service_without_endpoints() {
        let svc = |name: &str, matched, ready| ServiceInfo {
            namespace: "apps".into(),
            name: name.into(),
            type_: "ClusterIP".into(),
            selector: [("app".to_string(), name.to_string())].into(),
            pods_matched: matched,
            pods_ready: ready,
            ..Default::default()
        };
        let snap = ClusterSnapshot {
            services: vec![svc("none", 0, 0), svc("unready", 2, 0), svc("fine", 2, 1)],
            ..Default::default()
        };
        let rules: Vec<String> = detect(&snap, &Settings::default(), NOW).into_iter().map(|i| i.rule).collect();
        assert_eq!(rules, vec!["service-no-pods", "service-no-ready-endpoints"]);

        let mut idle = svc("parked", 0, 0);
        idle.idle = true;
        let snap = ClusterSnapshot { services: vec![idle], ..Default::default() };
        assert!(detect(&snap, &Settings::default(), NOW).is_empty(), "scaled to 0 on purpose");
    }

    fn workload(kind: &str, name: &str, ready: i32, desired: i32) -> WorkloadInfo {
        WorkloadInfo { kind: kind.into(), namespace: "default".into(), name: name.into(), ready, desired, created_ms: Some(0), ..Default::default() }
    }

    fn owned(name: &str, owner: &str, age_ms: i64) -> PodInfo {
        PodInfo {
            owner_kind: Some("Deployment".into()),
            owner_name: Some(owner.into()),
            created_ms: Some(NOW - age_ms),
            ready_containers: 0,
            containers: vec![ContainerInfo { name: "app".into(), state: "waiting".into(), reason: Some("ContainerCreating".into()), ..Default::default() }],
            ..pod(name)
        }
    }

    #[test]
    fn workload_down_waits_for_a_clean_restart() {
        let rules = |pods: Vec<PodInfo>| -> Vec<String> {
            let snap = ClusterSnapshot { workloads: vec![workload("Deployment", "web", 0, 1)], pods, ..Default::default() };
            detect(&snap, &Settings::default(), NOW).into_iter().map(|i| i.rule).filter(|r| r.starts_with("workload-")).collect()
        };
        assert!(rules(vec![owned("web-1", "web", 30_000)]).is_empty(), "its pod was just replaced and is starting");
        assert_eq!(rules(vec![owned("web-1", "web", STARTUP_GRACE_MS + 1)]), ["workload-down"], "still not up after the grace period");
        assert_eq!(rules(vec![]), ["workload-down"], "no pods at all");
        let mut crashing = owned("web-1", "web", 30_000);
        crashing.containers[0].restarts = 2;
        assert_eq!(rules(vec![crashing]), ["workload-down"], "a restarting pod isn't a clean start");
    }

    #[test]
    fn failed_cron_run_clears_after_a_later_success() {
        let failed = WorkloadInfo { condition_message: Some("BackoffLimitExceeded".into()), created_ms: Some(1), ..workload("Job", "backup-100", 0, 1) };
        let later_ok = WorkloadInfo { created_ms: Some(2), ..workload("Job", "backup-200", 1, 1) };
        let cron = workload("CronJob", "backup", 0, 0);
        let rules = |workloads: Vec<WorkloadInfo>| -> Vec<String> {
            detect(&ClusterSnapshot { workloads, ..Default::default() }, &Settings::default(), NOW).into_iter().map(|i| i.rule).collect()
        };
        assert_eq!(rules(vec![cron.clone(), failed.clone()]), ["job-failed"]);
        assert!(rules(vec![cron, failed.clone(), later_ok.clone()]).is_empty());
        assert_eq!(rules(vec![failed, later_ok]), ["job-failed"], "a standalone Job stays until it's deleted");
    }

    #[test]
    fn readiness_failures_while_starting_are_ignored() {
        let started = NOW - 10 * 60 * 1000;
        let ev = |message: &str, last_ms: i64| EventInfo {
            namespace: "default".into(),
            object_kind: "Pod".into(),
            object_name: "api".into(),
            reason: "Unhealthy".into(),
            message: message.into(),
            count: 1,
            first_ms: Some(started + 2_000),
            last_ms: Some(last_ms),
            ..Default::default()
        };
        let mut p = pod("api");
        p.containers.push(ContainerInfo { name: "app".into(), state: "running".into(), started_ms: Some(started), ..Default::default() });
        let count = |e: EventInfo| detect(&ClusterSnapshot { pods: vec![p.clone()], events: vec![e], ..Default::default() }, &Settings::default(), NOW).len();
        assert_eq!(count(ev("Readiness probe failed: connection refused", started + 5_000)), 0, "app not listening yet");
        assert_eq!(count(ev("Startup probe failed: connection refused", started + 5_000)), 0);
        assert_eq!(count(ev("Readiness probe failed: timeout", started + STARTUP_GRACE_MS + 60_000)), 1, "still failing after startup");
        assert_eq!(count(ev("Liveness probe failed: connection refused", started + 5_000)), 1, "liveness failures restart the container");
    }

    #[test]
    fn events_for_pods_that_are_gone_are_dropped() {
        let ev = |name: &str| EventInfo {
            namespace: "default".into(),
            object_kind: "Pod".into(),
            object_name: name.into(),
            reason: "Unhealthy".into(),
            message: "Readiness probe failed".into(),
            count: 3,
            first_ms: Some(NOW - 120_000),
            last_ms: Some(NOW - 60_000),
            ..Default::default()
        };
        let snap = ClusterSnapshot { pods: vec![pod("here")], events: vec![ev("here"), ev("replaced")], ..Default::default() };
        let names: Vec<String> = detect(&snap, &Settings::default(), NOW).into_iter().map(|i| i.name).collect();
        assert_eq!(names, ["here"]);
    }

    #[test]
    fn ingress_hpa_and_volume_rules() {
        let ing = IngressInfo {
            namespace: "apps".into(),
            name: "web".into(),
            routes: vec![
                IngressRoute { host: "a".into(), path: "/".into(), service: Some("web".into()), service_found: true, ..Default::default() },
                IngressRoute { host: "a".into(), path: "/x".into(), service: Some("gone".into()), ..Default::default() },
            ],
            ..Default::default()
        };
        let hpa = |problem: Option<&str>, at_max, created_ms| AutoscalerInfo {
            namespace: "apps".into(),
            name: "web".into(),
            problem: problem.map(str::to_string),
            at_max,
            created_ms: Some(created_ms),
            ..Default::default()
        };
        let pv = |phase: &str, policy: &str, since| PersistentVolumeInfo {
            name: format!("pv-{phase}-{policy}"),
            phase: phase.into(),
            reclaim_policy: Some(policy.into()),
            phase_since_ms: Some(since),
            ..Default::default()
        };
        let snap = ClusterSnapshot {
            ingresses: vec![ing],
            autoscalers: vec![hpa(Some("missing metrics"), false, 0), hpa(None, true, 0)],
            persistent_volumes: vec![
                pv("Released", "Retain", 0),
                pv("Released", "Delete", NOW - 10_000),
                pv("Released", "Delete", 0),
                pv("Bound", "Delete", 0),
            ],
            storage_classes: vec![StorageClassInfo { name: "local-path".into(), ..Default::default() }],
            volumes: vec![VolumeClaimInfo { namespace: "apps".into(), name: "data".into(), phase: "Pending".into(), storage_class: Some("fast".into()), ..Default::default() }],
            ..Default::default()
        };
        let issues = detect(&snap, &Settings::default(), NOW);
        let mut rules: Vec<&str> = issues.iter().map(|i| i.rule.as_str()).collect();
        rules.sort();
        assert_eq!(rules, ["hpa-at-max", "hpa-cannot-scale", "ingress-missing-backend", "pv-released", "pv-released-stuck", "pvc-pending"]);
        assert!(issues.iter().find(|i| i.rule == "ingress-missing-backend").unwrap().detail.contains("gone"));
        assert!(issues.iter().find(|i| i.rule == "pvc-pending").unwrap().detail.contains("\"fast\" doesn't exist"));

        let young = ClusterSnapshot { autoscalers: vec![hpa(Some("missing metrics"), false, NOW - 30_000)], ..Default::default() };
        assert!(detect(&young, &Settings::default(), NOW).is_empty(), "a new HPA has no metrics yet");
    }
}
