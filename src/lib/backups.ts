import { defaultMaxAgeHours, localTimeZone } from "./schedule";
import type { BackupInfo, BackupSpec, PodInfo, VolumeClaimInfo } from "./types";

export const BACKUP_PREFIX = "portside-backup-";
/** Mirrors `backup::DEFAULT_IMAGE`. */
export const DEFAULT_BACKUP_IMAGE = "curlimages/curl:8.16.0";
/** Mirrors `backup::MAX_NAME_LEN`. */
export const MAX_BACKUP_NAME = 52 - BACKUP_PREFIX.length;
/** Mirrors `backup::STOPPABLE`: workloads a backup can scale to 0. */
const STOPPABLE = ["Deployment", "StatefulSet"];

/** A claim that holds backups itself. */
export const isBackupClaim = (name: string) => name.startsWith(BACKUP_PREFIX);

/** The backup of this claim, if there is one. */
export function backupForClaim(backups: BackupInfo[], v: Pick<VolumeClaimInfo, "namespace" | "name">): BackupInfo | undefined {
  return backups.find((b) => b.namespace === v.namespace && b.spec?.claim === v.name);
}

/** Which of the claim's users a backup can stop, and which it can't (DaemonSets, CronJobs, bare pods). */
export function claimUsers(v: VolumeClaimInfo): { stoppable: string[]; other: string[] } {
  const stoppable = v.usedBy.filter((u) => STOPPABLE.includes(u.split("/")[0]));
  const other = v.usedBy.filter((u) => !stoppable.includes(u));
  return { stoppable, other };
}

/** A new backup for a claim: daily at 02:00 local time, 7 kept, offline when anything can be stopped. */
export function defaultSpec(v: VolumeClaimInfo): BackupSpec {
  const { stoppable } = claimUsers(v);
  const schedule = "0 2 * * *";
  return {
    namespace: v.namespace,
    name: v.name.slice(0, MAX_BACKUP_NAME).replace(/-+$/, ""),
    claim: v.name,
    schedule,
    timeZone: localTimeZone(),
    keep: 7,
    maxAgeHours: defaultMaxAgeHours(schedule),
    claimSize: v.capacity ?? "10Gi",
    storageClass: null,
    offline: stoppable.length > 0,
    workloads: stoppable,
    image: DEFAULT_BACKUP_IMAGE,
  };
}

/** The backup name from an issue's object: its CronJob, or one of its Jobs (`<cronjob>-<n>`). */
export function backupNameOf(kind: string, name: string): string {
  const cron = kind === "Job" ? name.replace(/-\d+$/, "") : name;
  return cron.startsWith(BACKUP_PREFIX) ? cron.slice(BACKUP_PREFIX.length) : cron;
}

export type BackupTone = "good" | "warning" | "critical" | "muted";

/** One word for the table, matching the backend's `backup-stale` rule. */
export function backupState(b: BackupInfo, now = Date.now()): { label: string; tone: BackupTone } {
  if (b.running) return { label: "Running", tone: "good" };
  if (b.suspended) return { label: "Paused", tone: "muted" };
  if (b.lastFailure && (b.lastSuccessfulMs == null || (b.lastFailureMs ?? 0) > b.lastSuccessfulMs)) return { label: "Failed", tone: "critical" };
  const maxAge = b.spec?.maxAgeHours ?? 0;
  const since = b.lastSuccessfulMs ?? b.createdMs;
  if (maxAge > 0 && since != null && now - since > maxAge * 3_600_000) return { label: "Overdue", tone: "warning" };
  if (b.lastSuccessfulMs == null) return { label: "Not run yet", tone: "muted" };
  return { label: "OK", tone: "good" };
}

/** The newest pod of the backup's runs (for "Logs of the last run"). */
export function lastRunPod(b: BackupInfo, pods: PodInfo[]): PodInfo | undefined {
  // Its runs are Jobs named `<cronjob>-<number>`; "db" mustn't match backup "db-2"'s runs.
  const run = new RegExp(`^${b.cronJob}-\\d+$`);
  return pods
    .filter((p) => p.namespace === b.namespace && p.ownerKind === "Job" && run.test(p.ownerName ?? ""))
    .sort((x, y) => (y.createdMs ?? 0) - (x.createdMs ?? 0))[0];
}
