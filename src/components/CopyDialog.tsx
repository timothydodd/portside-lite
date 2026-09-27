import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, CheckCircle2, FlaskConical, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage } from "../lib/format";
import type { CopyResult, ObjectRef, RelatedRef } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { connectionSummary } from "./ClusterSwitcher";
import { Modal, Spinner } from "./ui";

const refKey = (r: ObjectRef) => `${r.kind}/${r.name}`;

/** Copy a workload (and chosen dependencies) to any saved cluster/namespace. */
export default function CopyDialog() {
  const { copy: target, closeCopy } = useNavStore();
  const settings = useClusterStore((s) => s.settings);
  const namespaces = useClusterStore((s) => s.snapshot?.namespaces);
  const activeId = settings?.activeConnectionId ?? null;

  const [targetId, setTargetId] = useState<string>("");
  const [targetNs, setTargetNs] = useState("");
  const [refs, setRefs] = useState<RelatedRef[] | null>(null);
  const [refsError, setRefsError] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState<null | "dry" | "copy">(null);
  const [results, setResults] = useState<{ dryRun: boolean; rows: CopyResult[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!target) return;
    // Default to another cluster when one exists; same namespace name.
    const other = settings?.connections.find((p) => p.id !== activeId);
    setTargetId(other?.id ?? activeId ?? "");
    setTargetNs(target.namespace);
    setResults(null);
    setError(null);
    setRefs(null);
    setRefsError(null);
    ipc
      .relatedObjects(target.kind, target.namespace, target.name)
      .then((r) => {
        setRefs(r);
        // Safe related objects on by default; Secrets and PVCs are opt-in.
        setPicked(new Set(r.filter((x) => x.exists && x.defaultSelected).map(refKey)));
      })
      .catch((e) => setRefsError(errorMessage(e)));
  }, [target]); // eslint-disable-line react-hooks/exhaustive-deps

  const sameSpot = targetId === activeId && targetNs.trim() === target?.namespace;
  const targetProfile = settings?.connections.find((p) => p.id === targetId);
  const extras = useMemo(() => (refs ?? []).filter((r) => picked.has(refKey(r))).map(({ kind, name }) => ({ kind, name })), [refs, picked]);

  if (!target || !settings) return null;

  const run = async (dryRun: boolean) => {
    if (!dryRun) {
      const what = [target.name, ...extras.map((e) => e.name)].join(", ");
      const ok = await confirmDestructive(
        `Copy ${what} to "${targetProfile?.name}" in namespace ${targetNs.trim()}?\n\nExisting objects with the same names there are updated.`,
        "Copy to cluster",
      );
      if (!ok) return;
    }
    setBusy(dryRun ? "dry" : "copy");
    setError(null);
    setResults(null);
    try {
      const rows = await ipc.copyToCluster(target.kind, target.namespace, target.name, extras, targetId, targetNs, dryRun);
      setResults({ dryRun, rows });
      if (!dryRun) {
        const failed = rows.filter((r) => r.outcome === "error").length;
        if (failed) toast.error(`${failed} of ${rows.length} objects failed to copy`);
        else toast.success(`Copied ${rows.length} object${rows.length > 1 ? "s" : ""} to ${targetProfile?.name}`);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const toggle = (r: RelatedRef) =>
    setPicked((s) => {
      const n = new Set(s);
      if (n.has(refKey(r))) n.delete(refKey(r));
      else n.add(refKey(r));
      return n;
    });

  return (
    <Modal title={`Copy ${target.kind} ${target.name}`} onClose={closeCopy} wide>
      <div className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-content-muted">
          From <b className="text-content-secondary">{settings.connections.find((p) => p.id === activeId)?.name}</b> ·{" "}
          {target.namespace}. Server fields (status, uid, resourceVersion…) are stripped; the copy is created, or updated if it
          already exists.
        </p>

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Target cluster</span>
            <select className="field w-full" value={targetId} onChange={(e) => setTargetId(e.target.value)}>
              {settings.connections.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                  {p.id === activeId ? " (current)" : ""} — {connectionSummary(p.connection)}
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="field-label">Target namespace</span>
            <input className="field w-full" list="copy-ns" value={targetNs} onChange={(e) => setTargetNs(e.target.value)} />
            {targetId === activeId && (
              <datalist id="copy-ns">
                {namespaces?.map((n) => <option key={n} value={n} />)}
              </datalist>
            )}
            <span className="mt-1 block text-[11px] text-content-muted">Created if it doesn't exist.</span>
          </label>
        </div>

        {sameSpot && (
          <div className="tint-warning flex items-center gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} /> That's where it already lives. Pick another cluster or namespace to clone it.
          </div>
        )}

        <section>
          <h3 className="field-label">Bring along</h3>
          {refsError ? (
            <p className="text-xs text-critical">{refsError}</p>
          ) : refs == null ? (
            <Spinner />
          ) : refs.length === 0 ? (
            <p className="text-xs text-content-muted">Nothing related found: no config, Services, Ingresses or autoscalers point at it.</p>
          ) : (
            <div className="flex flex-col gap-1 rounded-md bg-muted p-2">
              {refs.map((r) => (
                <label key={refKey(r)} className={`flex items-center gap-2 text-xs ${r.exists ? "cursor-pointer" : ""}`}>
                  <input
                    type="checkbox"
                    className="accent-brand"
                    disabled={!r.exists}
                    checked={picked.has(refKey(r))}
                    onChange={() => toggle(r)}
                  />
                  <span className="w-44 shrink-0 truncate text-content-muted" title={r.kind}>{r.kind}</span>
                  <span className="mono text-content">{r.name}</span>
                  <span className="ml-auto min-w-0 truncate text-[11px] text-content-muted" title={r.reason}>
                    {r.exists ? r.reason : "not found in source"}
                  </span>
                </label>
              ))}
            </div>
          )}
        </section>

        {error && (
          <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <ShieldAlert size={14} className="mt-px shrink-0" />
            <span className="mono whitespace-pre-wrap break-words">{error}</span>
          </div>
        )}

        {results && (
          <section>
            <h3 className="field-label">{results.dryRun ? "Dry run: what would happen" : "Result"}</h3>
            <div className="flex flex-col gap-1">
              {results.rows.map((r) => (
                <div key={refKey(r)} className="flex items-start gap-2 text-xs">
                  {r.outcome === "error" ? (
                    <ShieldAlert size={14} className="mt-px shrink-0 text-critical" />
                  ) : (
                    <CheckCircle2 size={14} className="mt-px shrink-0 text-good" />
                  )}
                  <span className="w-44 shrink-0 truncate text-content-muted" title={r.kind}>{r.kind}</span>
                  <span className="mono text-content">{r.name}</span>
                  <span className="ml-auto text-content-secondary">
                    {r.outcome === "error" ? r.message : results.dryRun ? `would be ${r.outcome}` : r.outcome}
                  </span>
                </div>
              ))}
            </div>
          </section>
        )}

        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={closeCopy}>
            Close
          </button>
          <button className="btn-ghost" disabled={!!busy || sameSpot || !targetNs.trim()} onClick={() => void run(true)}>
            {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
          </button>
          <button className="btn-primary" disabled={!!busy || sameSpot || !targetNs.trim()} onClick={() => void run(false)}>
            {busy === "copy" ? <Spinner size={14} /> : null} Copy
          </button>
        </div>
      </div>
    </Modal>
  );
}
