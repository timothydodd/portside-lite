import type { PodInfo, WorkloadInfo } from "./types";

export type Tone = "good" | "warning" | "critical" | "muted";

export function workloadHealth(w: WorkloadInfo): { label: string; tone: Tone } {
  if (w.kind === "CronJob") return w.paused ? { label: "Suspended", tone: "muted" } : { label: "Scheduled", tone: "good" };
  if (w.kind === "Job") {
    if (w.conditionMessage) return { label: "Failed", tone: "critical" };
    if (w.ready >= w.desired) return { label: "Complete", tone: "muted" };
    return { label: "Running", tone: "good" };
  }
  if (w.desired === 0)
    return { label: w.disabledReplicas != null ? `Scaled to 0 (was ${w.disabledReplicas})` : "Scaled to 0", tone: "muted" };
  if (w.ready === 0) return { label: "Down", tone: "critical" };
  if (w.ready < w.desired) return { label: "Degraded", tone: "warning" };
  if (w.conditionMessage) return { label: "Rollout stalled", tone: "warning" };
  return { label: "Healthy", tone: "good" };
}

/**
 * Whether a pod's controller (as the snapshot reports it, ReplicaSets folded
 * into their Deployment) is this workload. CronJob pods belong to a Job named
 * `<cronjob>-<timestamp>`. Mirrors `archive::owner_matches` in portside-core.
 */
export function ownedBy(ownerKind: string | null, ownerName: string | null, kind: string, name: string): boolean {
  if (!ownerKind || !ownerName) return false;
  if (ownerKind === kind && ownerName === name) return true;
  return kind === "CronJob" && ownerKind === "Job" && ownerName.startsWith(`${name}-`) && /^\d+$/.test(ownerName.slice(name.length + 1));
}

export function podsOf(pods: PodInfo[], t: { kind: string; namespace: string; name: string }): PodInfo[] {
  return pods.filter((p) => p.namespace === t.namespace && ownedBy(p.ownerKind, p.ownerName, t.kind, t.name));
}
