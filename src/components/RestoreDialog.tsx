import { useEffect, useState } from "react";
import { ArchiveRestore, FlaskConical, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtDateTime } from "../lib/format";
import type { ImportResult } from "../lib/types";
import { useArchivesStore } from "../stores/archives";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { connectionSummary } from "./ClusterSwitcher";
import { ImportResults } from "./ImportDialog";
import { Modal, Spinner } from "./ui";

/** Deploy an archived workload again, to its own cluster or any other. */
export default function RestoreDialog() {
  const { restoring: archive, closeRestore } = useNavStore();
  const settings = useClusterStore((s) => s.settings);
  const loadArchives = useArchivesStore((s) => s.load);
  const [targetId, setTargetId] = useState("");
  const [namespace, setNamespace] = useState("");
  const [busy, setBusy] = useState<null | "dry" | "restore">(null);
  const [results, setResults] = useState<{ dryRun: boolean; rows: ImportResult[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!archive || !settings) return;
    // Back where it came from when that profile still exists.
    const origin = settings.connections.find((p) => p.id === archive.profileId);
    setTargetId(origin?.id ?? settings.activeConnectionId ?? "");
    setNamespace("");
    setResults(null);
    setError(null);
  }, [archive]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!archive || !settings) return null;
  const target = settings.connections.find((p) => p.id === targetId);
  const ns = namespace.trim() || archive.namespace;

  const run = async (dryRun: boolean) => {
    if (!dryRun) {
      const ok = await confirmDestructive(
        `Restore ${archive.kind} ${archive.name} (${archive.objects.length} object${archive.objects.length === 1 ? "" : "s"}) into "${target?.name}", namespace ${ns}?\n\n` +
          `Objects that already exist there are updated to the archived version.`,
        "Restore archive",
      );
      if (!ok) return;
    }
    setBusy(dryRun ? "dry" : "restore");
    setError(null);
    setResults(null);
    try {
      const rows = await ipc.restoreArchive(archive.id, targetId, namespace.trim() || null, dryRun);
      setResults({ dryRun, rows });
      if (!dryRun) {
        void loadArchives();
        const failed = rows.filter((r) => r.outcome === "error").length;
        if (failed) toast.error(`${failed} of ${rows.length} objects failed to restore`);
        else toast.success(`Restored ${archive.name} to ${target?.name}`);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal title={`Restore ${archive.kind} ${archive.name}`} onClose={closeRestore} wide>
      <div className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-content-muted">
          Archived {fmtDateTime(archive.archivedMs)} from <b className="text-content-secondary">{archive.connectionName}</b>
          {archive.replicas != null && `, starts with ${archive.replicas} replica${archive.replicas === 1 ? "" : "s"}`}. Applies:{" "}
          {archive.objects.map((o) => `${o.kind} ${o.name}`).join(", ")}. The archive is kept afterwards.
        </p>

        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Target cluster</span>
            <select className="field w-full" value={targetId} onChange={(e) => setTargetId(e.target.value)}>
              {settings.connections.map((p) => (
                <option key={p.id} value={p.id}>
                  {p.name}
                  {p.id === archive.profileId ? " (archived from)" : p.id === settings.activeConnectionId ? " (current)" : ""} — {connectionSummary(p.connection)}
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="field-label">Namespace</span>
            <input className="field w-full" value={namespace} placeholder={archive.namespace} onChange={(e) => setNamespace(e.target.value)} />
            <span className="mt-1 block text-[11px] text-content-muted">Created if missing.</span>
          </label>
        </div>

        {error && (
          <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <ShieldAlert size={14} className="mt-px shrink-0" />
            <span className="mono whitespace-pre-wrap break-words">{error}</span>
          </div>
        )}
        {results && <ImportResults dryRun={results.dryRun} rows={results.rows} />}

        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={closeRestore}>
            Close
          </button>
          <button className="btn-ghost" disabled={!!busy || !target} onClick={() => void run(true)}>
            {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
          </button>
          <button className="btn-primary" disabled={!!busy || !target} onClick={() => void run(false)}>
            {busy === "restore" ? <Spinner size={14} /> : <ArchiveRestore size={14} />} Restore
          </button>
        </div>
      </div>
    </Modal>
  );
}
