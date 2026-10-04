import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Archive, CheckCircle2, FolderOpen, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtDateTime } from "../lib/format";
import type { ArchiveOutcome, ArchivePlanItem, ObjectRef } from "../lib/types";
import { findArchive, useArchivesStore } from "../stores/archives";
import { activeProfile, useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { Modal, Spinner } from "./ui";

const refKey = (r: ObjectRef) => `${r.kind}/${r.name}`;

/** Removing these destroys data or breaks others, so they're never pre-ticked. */
const removeByDefault = (i: ArchivePlanItem) => i.exists && i.usedBy.length === 0 && i.kind !== "PersistentVolumeClaim";

/**
 * Archive a workload: save it and what it needs to a local folder, then take
 * it off the cluster. Restore puts it all back.
 */
export default function ArchiveDialog() {
  const { archiving: target, closeArchive, openWorkload } = useNavStore();
  const settings = useClusterStore((s) => s.settings);
  const clusterId = useClusterStore((s) => s.status?.clusterId ?? s.snapshot?.clusterId);
  const archives = useArchivesStore((s) => s.archives);
  const loadArchives = useArchivesStore((s) => s.load);

  const [plan, setPlan] = useState<ArchivePlanItem[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [keep, setKeep] = useState<Set<string>>(new Set());
  const [remove, setRemove] = useState<Set<string>>(new Set());
  const [includeLogs, setIncludeLogs] = useState(true);
  const [root, setRoot] = useState("");
  const [busy, setBusy] = useState(false);
  const [outcome, setOutcome] = useState<ArchiveOutcome | null>(null);

  useEffect(() => {
    if (!target) return;
    setPlan(null);
    setError(null);
    setOutcome(null);
    setIncludeLogs(true);
    if (archives == null) void loadArchives();
    ipc.archiveRoot().then(setRoot).catch(() => setRoot(""));
    ipc
      .archivePlan(target.kind, target.namespace, target.name)
      .then((p) => {
        setPlan(p);
        // Save everything that exists (Restore should bring back a working
        // workload); remove only what nothing else uses.
        setKeep(new Set(p.filter((i) => i.exists).map(refKey)));
        setRemove(new Set(p.filter(removeByDefault).map(refKey)));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [target]); // eslint-disable-line react-hooks/exhaustive-deps

  const secretsKept = useMemo(() => (plan ?? []).some((i) => i.sensitive && keep.has(refKey(i))), [plan, keep]);
  const sharedRemoved = useMemo(() => (plan ?? []).filter((i) => i.usedBy.length > 0 && remove.has(refKey(i))), [plan, remove]);
  const pvcRemoved = useMemo(() => (plan ?? []).some((i) => i.kind === "PersistentVolumeClaim" && remove.has(refKey(i))), [plan, remove]);
  if (!target) return null;

  const previous = findArchive(archives, clusterId, target);
  const profile = activeProfile(settings);

  const toggleKeep = (i: ArchivePlanItem) => {
    const k = refKey(i);
    const next = new Set(keep);
    if (next.has(k)) {
      next.delete(k);
      // Never remove something that isn't in the archive.
      const r = new Set(remove);
      r.delete(k);
      setRemove(r);
    } else next.add(k);
    setKeep(next);
  };
  const toggleRemove = (i: ArchivePlanItem) =>
    setRemove((s) => {
      const n = new Set(s);
      if (n.has(refKey(i))) n.delete(refKey(i));
      else n.add(refKey(i));
      return n;
    });

  const run = async () => {
    const kept = (plan ?? []).filter((i) => keep.has(refKey(i))).map(({ kind, name }) => ({ kind, name }));
    const removed = (plan ?? []).filter((i) => remove.has(refKey(i))).map(({ kind, name }) => ({ kind, name }));
    const ok = await confirmDestructive(
      `Archive ${target.kind} ${target.namespace}/${target.name}?\n\n` +
        `1. Save it${kept.length ? ` and ${kept.length} related object${kept.length === 1 ? "" : "s"}` : ""} to the archive folder` +
        `${includeLogs ? ", with its stored logs" : ""}.\n` +
        `2. Then remove it${removed.length ? ` and ${removed.length} related object${removed.length === 1 ? "" : "s"}` : ""} from "${profile?.name ?? "the cluster"}".\n\n` +
        `Nothing is removed unless the archive is written first. Restore it any time from Workloads → Archived.`,
      "Archive workload",
    );
    if (!ok) return;
    setBusy(true);
    setError(null);
    try {
      const res = await ipc.archiveWorkload(target.kind, target.namespace, target.name, kept, removed, includeLogs);
      setOutcome(res);
      void loadArchives();
      const failed = res.results.filter((r) => r.outcome === "error").length;
      if (failed) toast.error(`Archived, but ${failed} object${failed === 1 ? "" : "s"} couldn't be removed`);
      else toast.success(`Archived ${target.name} and removed it from the cluster`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={`Archive ${target.kind} ${target.name}`} onClose={closeArchive} wide explicitClose={busy || outcome != null}>
      <div className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-content-muted">
          Saves it to a local folder as re-appliable YAML (plus its stored logs), then removes it from the cluster. It shows up under{" "}
          <b className="text-content-secondary">Workloads → Archived</b>, where <b className="text-content-secondary">Restore</b> deploys it again.
        </p>

        {!outcome && (
          <section>
            <div className="mb-1 grid grid-cols-[3rem_4rem_1fr] items-end gap-2 text-[11px] text-content-muted">
              <span>Save</span>
              <span>Remove</span>
              <span />
            </div>
            {error && !plan ? (
              <p className="text-xs text-critical">{error}</p>
            ) : plan == null ? (
              <Spinner />
            ) : (
              <div className="flex max-h-64 flex-col gap-1 overflow-auto rounded-md bg-muted p-2">
                <div className="grid grid-cols-[3rem_4rem_1fr] items-center gap-2 text-xs">
                  <input type="checkbox" className="accent-brand" checked disabled title="The workload is always saved" />
                  <input type="checkbox" className="accent-brand" checked disabled title="The workload is always removed" />
                  <span className="truncate">
                    <span className="text-content-muted">{target.kind}</span> <span className="mono text-content">{target.name}</span>
                  </span>
                </div>
                {plan.map((i) => (
                  <div key={refKey(i)} className="grid grid-cols-[3rem_4rem_1fr] items-start gap-2 text-xs">
                    <input type="checkbox" className="accent-brand mt-0.5" disabled={!i.exists} checked={keep.has(refKey(i))} onChange={() => toggleKeep(i)} />
                    <input
                      type="checkbox"
                      className="accent-brand mt-0.5"
                      disabled={!i.exists || !keep.has(refKey(i))}
                      checked={remove.has(refKey(i))}
                      onChange={() => toggleRemove(i)}
                    />
                    <div className="min-w-0">
                      <div className="truncate">
                        <span className="text-content-muted">{i.kind}</span> <span className="mono text-content">{i.name}</span>
                      </div>
                      <div className="text-[11px] text-content-muted">
                        {!i.exists
                          ? "referenced but doesn't exist"
                          : i.usedBy.length
                            ? <span className="text-warning">also used by {i.usedBy.join(", ")}</span>
                            : i.reason}
                      </div>
                    </div>
                  </div>
                ))}
              </div>
            )}
          </section>
        )}

        {!outcome && (
          <label className="flex cursor-pointer items-center gap-2 text-xs">
            <input type="checkbox" className="accent-brand" checked={includeLogs} onChange={(e) => setIncludeLogs(e.target.checked)} />
            Save its stored logs too (logs.txt). They'd otherwise age out of the local store after the retention period.
          </label>
        )}

        {!outcome && (secretsKept || sharedRemoved.length > 0 || pvcRemoved || previous) && (
          <div className="flex flex-col gap-2">
            {sharedRemoved.length > 0 && (
              <Warn>
                Removing {sharedRemoved.map((i) => `${i.kind} ${i.name}`).join(", ")} will break the other workloads that use it.
              </Warn>
            )}
            {pvcRemoved && (
              <Warn>
                Removing a PersistentVolumeClaim usually deletes its volume and the data on it. The archive only holds the claim, not the data.
              </Warn>
            )}
            {secretsKept && <Warn>The archive folder will hold Secret values (base64, not encrypted). Keep it off shared drives and out of git.</Warn>}
            {previous && <Warn>Replaces the archive of it from {fmtDateTime(previous.archivedMs)}.</Warn>}
          </div>
        )}

        {root && !outcome && (
          <p className="truncate text-[11px] text-content-muted" title={root}>
            Archive folder: <span className="mono">{root}</span> (change it in Settings)
          </p>
        )}

        {error && plan && (
          <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <ShieldAlert size={14} className="mt-px shrink-0" />
            <span className="mono whitespace-pre-wrap break-words">{error}</span>
          </div>
        )}

        {outcome && (
          <section className="flex flex-col gap-2">
            <h3 className="field-label !mb-0">
              Saved {outcome.archive.objects.length} object{outcome.archive.objects.length === 1 ? "" : "s"}
              {outcome.archive.logLines > 0 && ` and ${outcome.archive.logLines.toLocaleString()} log lines`}. Removed from the cluster:
            </h3>
            <div className="flex max-h-48 flex-col gap-1 overflow-auto">
              {outcome.results.map((r) => (
                <div key={refKey(r)} className="flex items-start gap-2 text-xs">
                  {r.outcome === "error" ? (
                    <ShieldAlert size={14} className="mt-px shrink-0 text-critical" />
                  ) : (
                    <CheckCircle2 size={14} className="mt-px shrink-0 text-good" />
                  )}
                  <span className="w-40 shrink-0 truncate text-content-muted">{r.kind}</span>
                  <span className="mono w-40 shrink-0 truncate text-content" title={r.name}>{r.name}</span>
                  <span className="min-w-0 flex-1 text-content-secondary">{r.outcome === "error" ? r.message : r.outcome}</span>
                </div>
              ))}
            </div>
          </section>
        )}

        <div className="flex justify-end gap-2">
          {outcome ? (
            <>
              <button className="btn-ghost" onClick={() => void ipc.openArchiveFolder(outcome.archive.id).catch((e) => toast.error(errorMessage(e)))}>
                <FolderOpen size={14} /> Open folder
              </button>
              <button
                className="btn-ghost"
                onClick={() => {
                  closeArchive();
                  openWorkload(target);
                }}
              >
                View archive
              </button>
              <button className="btn-primary" onClick={closeArchive}>
                Done
              </button>
            </>
          ) : (
            <>
              <button className="btn-ghost" disabled={busy} onClick={closeArchive}>
                Cancel
              </button>
              <button className="btn-danger" disabled={busy || plan == null} onClick={() => void run()}>
                {busy ? <Spinner size={14} /> : <Archive size={14} />} Archive &amp; remove
              </button>
            </>
          )}
        </div>
      </div>
    </Modal>
  );
}

function Warn({ children }: { children: React.ReactNode }) {
  return (
    <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
      <AlertTriangle size={14} className="mt-px shrink-0" />
      <span>{children}</span>
    </div>
  );
}
