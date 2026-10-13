import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, CheckCircle2, FlaskConical, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage } from "../lib/format";
import type { CopyOutcome, FileProgress, ObjectRef, RelatedRef } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { connectionSummary } from "./ClusterSwitcher";
import { TransferBar } from "./FileBrowser";
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
  const [results, setResults] = useState<{ dryRun: boolean; outcome: CopyOutcome } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [withData, setWithData] = useState(false);
  const [transfer, setTransfer] = useState<FileProgress | null>(null);
  const transferId = useRef<string | null>(null);

  useEffect(() => {
    const un = listen<FileProgress>("files:progress", (e) => {
      if (e.payload.transferId === transferId.current) setTransfer(e.payload);
    });
    // Closing the dialog mid-copy stops the file transfer (the objects already applied stay).
    return () => {
      void un.then((f) => f());
      if (transferId.current) void ipc.cancelTransfer(transferId.current);
    };
  }, []);

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
    setWithData(false);
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
  const claimsPicked = useMemo(() => extras.filter((r) => r.kind === "PersistentVolumeClaim").map((r) => r.name), [extras]);

  if (!target || !settings) return null;

  const run = async (dryRun: boolean) => {
    const dataClaims = withData ? claimsPicked : [];
    if (!dryRun) {
      const what = [target.name, ...extras.map((e) => e.name)].join(", ");
      const files = dataClaims.length
        ? `\n\nThe files on ${dataClaims.join(", ")} are written over the target's. If a copy of ${target.name} already runs there it is stopped while they're written and started again after.`
        : "";
      const ok = await confirmDestructive(
        `Copy ${what} to "${targetProfile?.name}" in namespace ${targetNs.trim()}?\n\nExisting objects with the same names there are updated.${files}`,
        "Copy to cluster",
      );
      if (!ok) return;
    }
    setBusy(dryRun ? "dry" : "copy");
    setError(null);
    setResults(null);
    setTransfer(null);
    const id = crypto.randomUUID();
    transferId.current = id;
    try {
      const outcome = await ipc.copyToCluster({
        kind: target.kind,
        namespace: target.namespace,
        name: target.name,
        extras,
        targetConnectionId: targetId,
        targetNamespace: targetNs,
        dataClaims,
        transferId: id,
        dryRun,
      });
      setResults({ dryRun, outcome });
      if (!dryRun) {
        const rows = outcome.results;
        const failed = rows.filter((r) => r.outcome === "error").length;
        const copied = rows.filter((r) => r.outcome === "created" || r.outcome === "updated" || r.outcome === "copied").length;
        if (failed) toast.error(`${failed} of ${rows.length} objects failed to copy`);
        else if (outcome.warnings.length) toast.info(`Copied ${copied} object${copied > 1 ? "s" : ""}; ${outcome.warnings.length} thing${outcome.warnings.length > 1 ? "s" : ""} to sort out on ${targetProfile?.name}`);
        else toast.success(`Copied ${copied} object${copied > 1 ? "s" : ""} to ${targetProfile?.name}`);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      transferId.current = null;
      setTransfer(null);
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
    <Modal title={`Copy ${target.kind} ${target.name}`} onClose={closeCopy} wide explicitClose={busy === "copy"}>
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
          <p className="mb-1 text-[11px] text-content-muted">
            Secrets, volume claims and cluster-wide permissions are off by default. A dry run says what the copy would still be
            missing on the target.
          </p>
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

        {claimsPicked.length > 0 && (
          <section className="flex flex-col gap-1">
            <label className="flex cursor-pointer items-center gap-2 text-xs">
              <input type="checkbox" className="accent-brand" disabled={!!busy} checked={withData} onChange={(e) => setWithData(e.target.checked)} />
              <span className="text-content">Also copy the files on {claimsPicked.length > 1 ? "these volumes" : "this volume"}</span>
            </label>
            <p className="pl-5 text-[11px] text-content-muted">
              Streamed through this PC with short-lived helper pods on both sides, after the claim is created and before the
              workload starts. Files with the same names on the target are replaced, others kept. If the source app is running,
              stop it first for a consistent copy (databases especially).
            </p>
          </section>
        )}

        {busy === "copy" && (
          <div className="flex items-center gap-2 text-xs text-content-muted">
            {transfer ? <TransferBar transfer={transfer} /> : <><Spinner size={12} /> Copying…</>}
          </div>
        )}

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
              {results.outcome.results.map((r) => (
                <div key={refKey(r)} className="flex items-start gap-2 text-xs">
                  {r.outcome === "error" ? (
                    <ShieldAlert size={14} className="mt-px shrink-0 text-critical" />
                  ) : (
                    <CheckCircle2 size={14} className="mt-px shrink-0 text-good" />
                  )}
                  <span className="w-44 shrink-0 truncate text-content-muted" title={r.kind}>{r.kind}</span>
                  <span className="mono text-content">{r.name}</span>
                  <span className="ml-auto min-w-0 text-right text-content-secondary">
                    {r.outcome === "error"
                      ? r.message
                      : `${results.dryRun ? "would be " : ""}${r.outcome}${r.message ? ` — ${r.message}` : ""}`}
                  </span>
                </div>
              ))}
            </div>
            {results.outcome.warnings.length > 0 && (
              <div className="tint-warning mt-2 flex flex-col gap-1 rounded-md px-3 py-2 text-xs">
                <div className="flex items-center gap-2 font-medium">
                  <AlertTriangle size={14} className="shrink-0" />
                  {results.dryRun ? "The target would still be missing" : "The target is still missing"}
                </div>
                <ul className="ml-5 list-disc">
                  {results.outcome.warnings.map((w) => (
                    <li key={w}>{w}</li>
                  ))}
                </ul>
              </div>
            )}
          </section>
        )}

        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={closeCopy}>
            {busy === "copy" ? "Stop" : "Close"}
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
