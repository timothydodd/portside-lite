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

fn detect_workloads(snap: &ClusterSnapshot, out: &mut Vec<Issue>) {
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
                if let Some(msg) = &w.condition_message {
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
        let (sev, rule, detail, hint) = match v.phase.as_str() {
            "Pending" => (
                Severity::Warning,
                "pvc-pending",
                "Claim isn't bound to a volume.",
                "Check the StorageClass exists and its provisioner (local-path on k3s) is running.",
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
}

fn detect_events(snap: &ClusterSnapshot, now: i64, out: &mut Vec<Issue>) {
    // Group by (object, reason) so a flapping probe is one issue, not fifty.
    let mut groups: HashMap<(String, String, String, String), (i32, &EventInfo)> = HashMap::new();
    for e in &snap.events {
        if e.last_ms.map(|t| now - t > EVENT_WINDOW_MS).unwrap_or(true) {
            continue;
        }
        if !EVENT_REASONS.iter().any(|(r, _)| *r == e.reason) {
            continue;
        }
        let key = (
            e.object_kind.clone(),
            e.namespace.clone(),
            e.object_name.clone(),
            e.reason.clone(),
        );
        let entry = groups.entry(key).or_insert((0, e));
        entry.0 += e.count;
        if e.last_ms > entry.1.last_ms {
            entry.1 = e;
        }
    }
    for ((kind, ns, name, reason), (count, e)) in groups {
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
            since_ms: e.first_ms,
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
    detect_workloads(snap, &mut out);
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
        let snap = ClusterSnapshot { events: vec![ev(3), ev(4)], ..Default::default() };
        let issues = detect(&snap, &Settings::default(), NOW);
        assert_eq!(issues.len(), 1);
        assert!(issues[0].detail.contains("7×"));
    }
}
