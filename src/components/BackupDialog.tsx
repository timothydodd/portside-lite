import { useMemo, useState } from "react";
import { AlertTriangle, CheckCircle2, DatabaseBackup, FlaskConical } from "lucide-react";
import * as ipc from "../lib/ipc";
import { errorMessage } from "../lib/format";
import { BACKUP_PREFIX, backupForClaim, claimUsers, defaultSpec } from "../lib/backups";
import { coverage, defaultMaxAgeHours, describeCron, timeZones } from "../lib/schedule";
import type { BackupSpec, VolumeClaimInfo } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import SchedulePicker from "./SchedulePicker";
import { Modal, Spinner } from "./ui";

const LOCAL_PATH = "rancher.io/local-path";

/** Set up a scheduled backup of a volume, or change an existing one. */
export default function BackupDialog() {
  const setup = useNavStore((s) => s.backupSetup);
  const close = useNavStore((s) => s.closeBackupSetup);
  const volumes = useClusterStore((s) => s.snapshot?.volumes);
  const backups = useClusterStore((s) => s.snapshot?.backups);
  if (!setup) return null;

  const ref = "spec" in setup ? { namespace: setup.spec.namespace, name: setup.spec.claim } : setup.claim;
  const volume = volumes?.find((v) => v.namespace === ref.namespace && v.name === ref.name);
  // From a claim that's already backed up: change that backup instead of making a second.
  const existing = "claim" in setup && backups ? backupForClaim(backups, ref) : undefined;
  const spec = "spec" in setup ? setup.spec : existing?.spec ?? (volume ? defaultSpec(volume) : null);
  const editing = "spec" in setup || !!existing?.spec;

  return (
    <Modal title={editing ? `Change backup of ${ref.name}` : `Back up ${ref.name}`} onClose={close} wide explicitClose>
      {spec ? (
        <BackupForm key={`${ref.namespace}/${ref.name}`} initial={spec} volume={volume} editing={editing} onClose={close} />
      ) : (
        <div className="flex flex-col gap-3 text-sm">
          <div className="tint-critical rounded-md px-3 py-2 text-xs">
            Volume {ref.namespace}/{ref.name} isn't in the cluster any more.
          </div>
          <div className="flex justify-end">
            <button className="btn-ghost" onClick={close}>
              Close
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

function BackupForm({ initial, volume, editing, onClose }: { initial: BackupSpec; volume?: VolumeClaimInfo; editing: boolean; onClose: () => void }) {
  const [spec, setSpec] = useState<BackupSpec>(initial);
  const [maxAgeTouched, setMaxAgeTouched] = useState(editing);
  const [busy, setBusy] = useState<null | "dry" | "save">(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const storageClasses = useClusterStore((s) => s.snapshot?.storageClasses);
  const zones = useMemo(timeZones, []);
  const set = (patch: Partial<BackupSpec>) => setSpec((s) => ({ ...s, ...patch }));

  const users = volume ? claimUsers(volume) : { stoppable: spec.workloads, other: [] };
  // Workloads saved in the spec that no longer use the volume still show, so they can be unticked.
  const candidates = [...new Set([...users.stoppable, ...spec.workloads])];
  const provisioner = storageClasses?.find((c) => c.name === volume?.storageClass)?.provisioner;
  const singleNode = volume?.accessModes.includes("ReadWriteOnce") && provisioner != null && provisioner !== LOCAL_PATH;

  const toggleWorkload = (w: string, on: boolean) =>
    set({ workloads: on ? [...spec.workloads.filter((x) => x !== w), w] : spec.workloads.filter((x) => x !== w) });
  const onSchedule = (schedule: string) => set(maxAgeTouched ? { schedule } : { schedule, maxAgeHours: defaultMaxAgeHours(schedule) });

  const run = async (dryRun: boolean) => {
    setBusy(dryRun ? "dry" : "save");
    setResult(null);
    const final: BackupSpec = { ...spec, timeZone: spec.timeZone?.trim() || null, workloads: spec.offline ? spec.workloads : [] };
    try {
      const done = await ipc.saveBackup(final, dryRun);
      if (dryRun) {
        setResult({ ok: true, text: done.join("; ") });
      } else {
        toast.success(`Backup ${final.name}: ${done.join("; ")}`);
        onClose();
      }
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const ready = spec.name && spec.schedule.trim() && spec.keep >= 1 && spec.image && (!spec.offline || spec.workloads.length > 0);

  return (
    <div className="flex flex-col gap-4 text-sm">
      <p className="text-xs text-content-muted">
        A job in the cluster copies the volume into a dated <span className="mono">.tar.gz</span> on this schedule and keeps the newest copies on a
        backup volume next to it. It runs whether or not Portside is open.
      </p>

      <section className="flex flex-col gap-2">
        <div className="card-title">Take the app offline</div>
        <label className={`flex items-start gap-2 ${candidates.length ? "" : "opacity-50"}`}>
          <input
            type="checkbox"
            className="accent-brand mt-0.5"
            checked={spec.offline}
            disabled={candidates.length === 0}
            onChange={(e) => set({ offline: e.target.checked, workloads: e.target.checked && spec.workloads.length === 0 ? candidates : spec.workloads })}
          />
          <span>
            Stop what uses the volume while it's copied
            <span className="block text-xs text-content-muted">
              Scaled to 0 before the copy and back to its size right after, so nothing is half-written. The app is down for the copy.
            </span>
          </span>
        </label>
        {spec.offline && candidates.length > 0 && (
          <div className="ml-6 flex flex-col gap-1">
            {candidates.map((w) => (
              <label key={w} className="flex items-center gap-2 text-xs">
                <input type="checkbox" className="accent-brand" checked={spec.workloads.includes(w)} onChange={(e) => toggleWorkload(w, e.target.checked)} />
                <span className="mono">{w}</span>
                {!users.stoppable.includes(w) && <span className="text-content-muted">(doesn't use it any more)</span>}
              </label>
            ))}
          </div>
        )}
        {candidates.length === 0 && <p className="text-xs text-content-muted">No Deployment or StatefulSet uses this volume, so there's nothing to stop.</p>}
        {!spec.offline && candidates.length > 0 && (
          <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} className="mt-px shrink-0" />
            Copied while the app runs: a file being written during the copy, a database especially, can be saved half-written.
          </div>
        )}
        {users.other.length > 0 && (
          <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} className="mt-px shrink-0" />
            {users.other.join(", ")} also {users.other.length === 1 ? "uses" : "use"} this volume and can't be stopped by a backup, so{" "}
            {users.other.length === 1 ? "it keeps" : "they keep"} running during the copy.
          </div>
        )}
        {spec.offline && singleNode && (
          <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} className="mt-px shrink-0" />
            This volume's storage ({provisioner}) attaches to one node at a time. The backup job has to start on the app's node, which isn't guaranteed
            while the app is running. Backups are tested with k3s local-path.
          </div>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <div className="card-title">When</div>
        <SchedulePicker cron={spec.schedule} onChange={onSchedule} />
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Time zone</span>
            <input className="field w-full" list="backup-tz" value={spec.timeZone ?? ""} placeholder="Cluster's (UTC on k3s)" onChange={(e) => set({ timeZone: e.target.value })} />
            <datalist id="backup-tz">
              {zones.map((z) => (
                <option key={z} value={z} />
              ))}
            </datalist>
          </label>
          <p className="self-end pb-1.5 text-xs text-content-muted">
            {describeCron(spec.schedule)}
            {spec.timeZone ? ` (${spec.timeZone})` : " (cluster time)"}
          </p>
        </div>
      </section>

      <section className="flex flex-col gap-2">
        <div className="card-title">How many to keep</div>
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Backups kept</span>
            <input
              className="field w-full"
              type="number"
              min={1}
              max={1000}
              value={spec.keep}
              onChange={(e) => set({ keep: Math.max(0, Math.floor(Number(e.target.value) || 0)) })}
            />
            <span className="mt-1 block text-[11px] text-content-muted">
              Covers {coverage(spec.schedule, Math.max(spec.keep, 1))}. Older ones are deleted after each run.
            </span>
          </label>
          <label className="block">
            <span className="field-label">Warn when the newest is older than (hours)</span>
            <input
              className="field w-full"
              type="number"
              min={0}
              value={spec.maxAgeHours}
              onChange={(e) => {
                setMaxAgeTouched(true);
                set({ maxAgeHours: Math.max(0, Math.floor(Number(e.target.value) || 0)) });
              }}
            />
            <span className="mt-1 block text-[11px] text-content-muted">Shows on Problems and alerts. 0 = never.</span>
          </label>
        </div>
      </section>

      <details>
        <summary className="cursor-pointer text-xs font-medium text-content-secondary">Name, storage and image</summary>
        <div className="mt-3 grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Backup name</span>
            <input className="field mono w-full" value={spec.name} disabled={editing} onChange={(e) => set({ name: e.target.value.toLowerCase() })} />
            <span className="mt-1 block text-[11px] text-content-muted">
              {spec.namespace} · <span className="mono">{BACKUP_PREFIX + (spec.name || "…")}</span>
            </span>
          </label>
          <label className="block">
            <span className="field-label">Backup storage size</span>
            <input className="field w-full" value={spec.claimSize} disabled={editing} onChange={(e) => set({ claimSize: e.target.value.trim() })} />
            <span className="mt-1 block text-[11px] text-content-muted">
              {editing
                ? "Set when the storage was created."
                : `${volume?.capacity ? `The volume is ${volume.capacity}. ` : ""}local-path doesn't enforce sizes: make sure the node has room for ${Math.max(spec.keep, 1)} copies.`}
            </span>
          </label>
          <label className="block">
            <span className="field-label">Storage class</span>
            <select className="field w-full" value={spec.storageClass ?? ""} disabled={editing} onChange={(e) => set({ storageClass: e.target.value || null })}>
              <option value="">Cluster default</option>
              {(storageClasses ?? []).map((c) => (
                <option key={c.name} value={c.name}>
                  {c.name}
                  {c.isDefault ? " (default)" : ""}
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="field-label">Image</span>
            <input className="field mono w-full" value={spec.image} onChange={(e) => set({ image: e.target.value.trim() })} />
            <span className="mt-1 block text-[11px] text-content-muted">Needs sh, tar, gzip and curl.</span>
          </label>
        </div>
      </details>

      {result && (
        <div className={`${result.ok ? "tint-good" : "tint-critical"} flex items-start gap-2 rounded-md px-3 py-2 text-xs`}>
          {result.ok && <CheckCircle2 size={14} className="mt-px shrink-0" />}
          {result.text}
        </div>
      )}
      <div className="flex justify-end gap-2">
        <button className="btn-ghost" onClick={onClose}>
          Cancel
        </button>
        <button className="btn-ghost" disabled={!!busy || !ready} onClick={() => void run(true)}>
          {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
        </button>
        <button className="btn-primary" disabled={!!busy || !ready} onClick={() => void run(false)}>
          {busy === "save" ? <Spinner size={14} /> : <DatabaseBackup size={14} />} {editing ? "Save" : "Set up backup"}
        </button>
      </div>
    </div>
  );
}
