import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, DatabaseBackup, FolderOpen, HardDrive, Laptop, Pause, Pencil, Play, RefreshCw, ScrollText, Settings2, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { backupForClaim, backupState, isBackupClaim, lastRunPod } from "../lib/backups";
import { errorMessage, fmtAgo, fmtBytes } from "../lib/format";
import { describeCron } from "../lib/schedule";
import type { BackupInfo, BackupJob, ClusterSnapshot, JobRecord, LocalArchives, LocalBackupStatus, ProfileBackupStatus, VolumeClaimInfo } from "../lib/types";
import SnapshotGate from "../components/SnapshotGate";
import { ActionMenu, EmptyState, Modal, NamespaceSelect, PageHeader, SearchInput, Spinner, StatusPill } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";

export async function runBackupNow(namespace: string, name: string) {
  try {
    const job = await ipc.runBackupNow(namespace, name);
    toast.success(`Backup ${name} started (${job})`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

async function setPaused(b: BackupInfo, paused: boolean) {
  try {
    await ipc.setBackupPaused(b.namespace, b.name, paused);
    toast.success(paused ? `Paused ${b.name}: no runs until it's resumed` : `Resumed ${b.name}`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

async function removeBackup(b: BackupInfo) {
  const ok = await confirmDestructive(
    `Stop backing up ${b.namespace}/${b.spec?.claim ?? b.name}?\n\nIts schedule and run history are removed. The backups already made stay on its storage (${b.cronJob}) until you delete that too, which is the next question.`,
    "Remove backup",
  );
  if (!ok) return;
  const deleteStorage = await confirmDestructive(
    `Also delete ${b.cronJob} and every backup on it?\n\nThis can't be undone. Choose No to keep the files; you can browse or delete them later from Storage.`,
    "Delete backup storage",
  );
  try {
    await ipc.removeBackup(b.namespace, b.name, deleteStorage);
    toast.success(deleteStorage ? `Removed ${b.name} and its backups` : `Removed ${b.name}; its backups are kept in ${b.cronJob}`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

/** What this PC has copied and backed up, refreshed while the page is open. */
function useLocalBackups() {
  const [status, setStatus] = useState<LocalBackupStatus | null>(null);
  const [archives, setArchives] = useState<LocalArchives[]>([]);
  useEffect(() => {
    let live = true;
    const load = () => {
      ipc.localBackupStatus().then((s) => live && setStatus(s)).catch(() => {});
      ipc.localBackupArchives().then((a) => live && setArchives(a)).catch(() => {});
    };
    load();
    const t = window.setInterval(load, 5000);
    return () => {
      live = false;
      window.clearInterval(t);
    };
  }, []);
  return { status, archives };
}

export default function BackupsPage() {
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [picking, setPicking] = useState(false);
  const local = useLocalBackups();

  return (
    <SnapshotGate>
      {(s) => {
        const rows = s.backups.filter(
          (b) => (!namespace || b.namespace === namespace) && (!search || `${b.name} ${b.spec?.claim ?? ""}`.toLowerCase().includes(search.toLowerCase())),
        );
        return (
          <div className="flex h-full flex-col">
            <PageHeader title="Backups" subtitle="Scheduled copies of volumes, made inside the cluster">
              <button className="btn-primary" onClick={() => setPicking(true)}>
                <DatabaseBackup size={14} /> Back up a volume…
              </button>
              <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />
              <SearchInput value={search} onChange={setSearch} className="w-56" />
            </PageHeader>
            <LocalPanel status={local.status} />
            {s.backups.length === 0 ? (
              <EmptyState icon={<DatabaseBackup size={28} />} title="No volume backups yet">
                Pick a volume with <b>Back up a volume…</b>, or from its menu on the Storage page. You choose how often it runs, how many copies to keep,
                and whether the app is stopped while it's copied. Runs happen in the cluster, so they don't need Portside to be open.
              </EmptyState>
            ) : (
              <BackupTable rows={rows} snapshot={s} archives={local.archives} />
            )}
            {picking && <VolumePicker snapshot={s} onClose={() => setPicking(false)} />}
          </div>
        );
      }}
    </SnapshotGate>
  );
}

function JobLine({ label, enabled, schedule, rec, state }: { label: string; enabled: boolean; schedule: string; rec: JobRecord; state: "running" | "queued" | null }) {
  const failed = rec.error && (rec.lastSuccessMs == null || (rec.lastAttemptMs ?? 0) > rec.lastSuccessMs);
  return (
    <div className="flex items-start gap-2 text-xs">
      <span className="w-28 shrink-0 text-content-muted">{label}</span>
      {!enabled ? (
        <span className="text-content-muted">off</span>
      ) : (
        <span className="min-w-0">
          {rec.lastSuccessMs ? (
            <span className="text-content-secondary">
              {fmtAgo(rec.lastSuccessMs)}
              {rec.summary && ` · ${rec.summary}`}
            </span>
          ) : (
            <span className="text-content-muted">not yet</span>
          )}
          {failed && (
            <span className="mt-0.5 flex items-start gap-1 text-critical" title={rec.error ?? undefined}>
              <AlertTriangle size={12} className="mt-px shrink-0" />
              <span className="line-clamp-2">{rec.error}</span>
            </span>
          )}
          <span className="block text-[11px] text-content-muted">
            {describeCron(schedule)}
            {state && ` · ${state === "running" ? "running now" : "queued"}`}
          </span>
        </span>
      )}
    </div>
  );
}

function jobState(p: ProfileBackupStatus, job: BackupJob) {
  return p.running === job ? "running" : p.queued.includes(job) ? "queued" : null;
}

/** Backups this PC makes for every connection (set up under Settings). */
function LocalPanel({ status }: { status: LocalBackupStatus | null }) {
  const settings = useClusterStore((s) => s.settings);
  const go = useNavStore((s) => s.go);
  if (!settings || !status) return null;
  const any = settings.backupPull || settings.manifestBackup;
  const runNow = async () => {
    try {
      await ipc.runLocalBackups([...(settings.backupPull ? (["pull"] as const) : []), ...(settings.manifestBackup ? (["manifests"] as const) : [])]);
      toast.success(status.activity ? "Queued after the current run" : "Started");
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };
  return (
    <div className="border-b border-border-light px-6 py-4">
      <div className="mb-3 flex items-center gap-2">
        <Laptop size={15} className="text-content-muted" />
        <h2 className="card-title">On this PC</h2>
        {status.activity && (
          <span className="flex items-center gap-1.5 text-xs text-content-secondary">
            <Spinner size={12} /> {status.activity}
            {status.queued > 0 && <span className="text-content-muted">· {status.queued} more queued</span>}
          </span>
        )}
        <span className="ml-auto flex gap-2">
          {any && (
            <button className="btn-chip" onClick={() => void runNow()}>
              <RefreshCw size={12} /> Run now
            </button>
          )}
          <button className="btn-chip" title={status.folder} onClick={() => void ipc.openBackupFolder(null).catch((e) => toast.error(errorMessage(e)))}>
            <FolderOpen size={12} /> Open folder
          </button>
          <button className="btn-chip" onClick={() => go("settings")}>
            <Settings2 size={12} /> Settings
          </button>
        </span>
      </div>
      {!any ? (
        <p className="text-xs text-content-muted">Copying volume backups and manifest backups to this PC are both off. Turn them on under Settings → Backups to this PC.</p>
      ) : (
        <div className="grid grid-cols-[repeat(auto-fill,minmax(340px,1fr))] gap-3">
          {status.profiles.map((p) => (
            <div key={p.profileId} className="card flex flex-col gap-2 px-3 py-2.5">
              <div className="flex items-center gap-2">
                <span className="text-sm font-medium text-content">{p.name}</span>
                <button
                  className="btn-quiet ml-auto !px-1 !py-0"
                  title={`Open ${p.folder}`}
                  aria-label={`Open ${p.name}'s backup folder`}
                  onClick={() => void ipc.openBackupFolder(p.profileId).catch((e) => toast.error(errorMessage(e)))}
                >
                  <FolderOpen size={13} />
                </button>
              </div>
              <JobLine label="Volume copies" enabled={settings.backupPull} schedule={settings.backupPullSchedule} rec={p.pull} state={jobState(p, "pull")} />
              <JobLine label="Manifests" enabled={settings.manifestBackup} schedule={settings.manifestBackupSchedule} rec={p.manifests} state={jobState(p, "manifests")} />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function BackupTable({ rows, snapshot, archives }: { rows: BackupInfo[]; snapshot: ClusterSnapshot; archives: LocalArchives[] }) {
  const { openBackupSetup, openFiles, openPod } = useNavStore();
  const pullOn = useClusterStore((s) => s.settings?.backupPull ?? false);
  const sorted = useMemo(() => [...rows].sort((a, b) => a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name)), [rows]);
  if (sorted.length === 0) {
    return <EmptyState title="No backups match">Nothing matches the namespace and search filter.</EmptyState>;
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Backup</th>
            <th>Namespace</th>
            <th>Status</th>
            <th>Schedule</th>
            <th className="text-right">Kept</th>
            <th className="text-right">Last good backup</th>
            <th className="text-right">Storage</th>
            <th className="text-right">On this PC</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {sorted.map((b) => {
            const state = backupState(b);
            const pod = lastRunPod(b, snapshot.pods);
            const here = archives.find((a) => a.namespace === b.namespace && a.name === b.name);
            return (
              <tr key={`${b.namespace}/${b.name}`}>
                <td>
                  <div className="font-medium text-content">{b.spec?.claim ?? b.name}</div>
                  <div className="text-[11px] text-content-muted" title={b.spec?.workloads.join("\n")}>
                    {!b.spec ? "settings unreadable" : b.spec.offline ? `offline: stops ${b.spec.workloads.join(", ")}` : "copied while running"}
                  </div>
                </td>
                <td className="text-content-secondary">{b.namespace}</td>
                <td>
                  <StatusPill status={state.label} tone={state.tone} />
                  {state.label === "Failed" && b.lastFailure && (
                    <div className="max-w-xs truncate text-[11px] text-content-muted" title={b.lastFailure}>
                      {b.lastFailure}
                    </div>
                  )}
                </td>
                <td>
                  <div title={b.schedule}>{describeCron(b.schedule)}</div>
                  <div className="text-[11px] text-content-muted">{b.timeZone ?? "cluster time"}</div>
                </td>
                <td className="text-right tabular-nums">{b.spec?.keep ?? "?"}</td>
                <td className="text-right text-xs text-content-muted">{b.lastSuccessfulMs ? fmtAgo(b.lastSuccessfulMs) : "never"}</td>
                <td className="text-right text-xs text-content-muted">{b.claimCapacity ?? b.claimPhase ?? "missing"}</td>
                <td className="text-right text-xs text-content-muted" title={here ? fmtBytes(here.bytes) : undefined}>
                  {here?.count ? `${here.count} · newest ${fmtAgo(here.newestMs)}` : pullOn ? "none yet" : "copying is off"}
                </td>
                <td className="whitespace-nowrap text-right">
                  <ActionMenu
                    label={`Actions for ${b.name}`}
                    items={[
                      { label: "Run now", icon: <Play size={14} />, disabled: b.running, onSelect: () => void runBackupNow(b.namespace, b.name) },
                      b.spec && { label: "Change…", icon: <Pencil size={14} />, title: "Schedule, how many to keep, offline or not", onSelect: () => openBackupSetup({ spec: b.spec! }) },
                      {
                        label: "Browse backups",
                        icon: <FolderOpen size={14} />,
                        title: "Open its storage to see or download the archives",
                        disabled: b.claimPhase !== "Bound",
                        onSelect: () => openFiles(b.namespace, b.cronJob),
                      },
                      pod && { label: "Logs of the last run", icon: <ScrollText size={14} />, onSelect: () => openPod(pod.namespace, pod.name, "logs") },
                      b.suspended
                        ? { label: "Resume schedule", icon: <Play size={14} />, onSelect: () => void setPaused(b, false) }
                        : { label: "Pause schedule", icon: <Pause size={14} />, onSelect: () => void setPaused(b, true) },
                      { label: "Remove…", icon: <Trash2 size={14} />, danger: true, onSelect: () => void removeBackup(b) },
                    ]}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** Volumes to back up; ones already backed up are marked and open their backup to change. */
function VolumePicker({ snapshot, onClose }: { snapshot: ClusterSnapshot; onClose: () => void }) {
  const openBackupSetup = useNavStore((s) => s.openBackupSetup);
  const candidates = useMemo(
    () => snapshot.volumes.filter((v) => !isBackupClaim(v.name)).sort((a, b) => a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name)),
    [snapshot.volumes],
  );
  const pick = (v: VolumeClaimInfo) => {
    onClose();
    openBackupSetup({ claim: { namespace: v.namespace, name: v.name } });
  };
  return (
    <Modal title="Back up a volume" onClose={onClose} wide>
      {candidates.length === 0 ? (
        <p className="text-sm text-content-secondary">This cluster has no PersistentVolumeClaims.</p>
      ) : (
        <div className="flex max-h-[60vh] flex-col gap-1 overflow-auto">
          {candidates.map((v) => {
            const existing = backupForClaim(snapshot.backups, v);
            return (
              <button
                key={`${v.namespace}/${v.name}`}
                className="flex items-center gap-3 rounded-md px-3 py-2 text-left text-sm hover:bg-muted"
                onClick={() => pick(v)}
              >
                <HardDrive size={16} className="shrink-0 text-content-muted" />
                <span className="min-w-0 flex-1">
                  <span className="font-medium text-content">{v.name}</span>
                  <span className="ml-2 text-xs text-content-muted">
                    {v.namespace} · {v.capacity ?? v.phase}
                    {v.usedBy.length > 0 && ` · used by ${v.usedBy.join(", ")}`}
                  </span>
                </span>
                {existing && <span className="text-xs text-content-muted">backed up · change</span>}
              </button>
            );
          })}
        </div>
      )}
      <div className="mt-4 flex justify-end">
        <button className="btn-ghost" onClick={onClose}>
          Close
        </button>
      </div>
    </Modal>
  );
}
