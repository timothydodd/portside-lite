import { useEffect, useMemo, useState } from "react";
import { open as openFiles } from "@tauri-apps/plugin-dialog";
import { create } from "zustand";
import { CheckCircle2, ClipboardPaste, FileUp, FlaskConical, ShieldAlert, X } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage } from "../lib/format";
import type { ImportResult, ManifestDoc, SourceFile } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { toast } from "../stores/toast";
import { connectionSummary } from "./ClusterSwitcher";
import { Modal, Spinner } from "./ui";

export const useImportDialog = create<{ open: boolean; show: () => void; close: () => void }>((set) => ({
  open: false,
  show: () => set({ open: true }),
  close: () => set({ open: false }),
}));

/** Import one or many YAML files (or pasted YAML) into any saved cluster. */
export default function ImportDialog() {
  const { open, close } = useImportDialog();
  const settings = useClusterStore((s) => s.settings);
  const activeId = settings?.activeConnectionId ?? "";

  const [sources, setSources] = useState<SourceFile[]>([]);
  const [docs, setDocs] = useState<ManifestDoc[]>([]);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [pasting, setPasting] = useState(false);
  const [pasteText, setPasteText] = useState("");
  const [targetId, setTargetId] = useState(activeId);
  const [nsOverride, setNsOverride] = useState("");
  const [busy, setBusy] = useState<null | "dry" | "import" | "read">(null);
  const [results, setResults] = useState<{ dryRun: boolean; rows: ImportResult[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Fresh state each time the dialog opens.
  useEffect(() => {
    if (!open) return;
    setSources([]);
    setDocs([]);
    setPicked(new Set());
    setPasting(false);
    setPasteText("");
    setTargetId(activeId);
    setNsOverride("");
    setResults(null);
    setError(null);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  // Re-parse whenever the sources change; select every valid document.
  useEffect(() => {
    setResults(null);
    if (!sources.length) {
      setDocs([]);
      setPicked(new Set());
      return;
    }
    ipc
      .parseManifests(sources)
      .then((d) => {
        setDocs(d);
        setPicked(new Set(d.filter((x) => !x.error).map((x) => x.index)));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [sources]);

  const valid = useMemo(() => docs.filter((d) => !d.error), [docs]);
  const target = settings?.connections.find((p) => p.id === targetId);
  if (!open || !settings) return null;

  const addSources = (more: SourceFile[]) =>
    setSources((cur) => {
      const names = new Set(more.map((m) => m.name));
      return [...cur.filter((c) => !names.has(c.name)), ...more]; // re-adding a file replaces it
    });

  const chooseFiles = async () => {
    const chosen = await openFiles({ multiple: true, directory: false, filters: [{ name: "Kubernetes YAML", extensions: ["yaml", "yml", "json"] }] });
    const paths = Array.isArray(chosen) ? chosen : chosen ? [chosen] : [];
    if (!paths.length) return;
    setBusy("read");
    setError(null);
    try {
      addSources(await ipc.readTextFiles(paths));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const addPaste = () => {
    if (!pasteText.trim()) return;
    const n = sources.filter((s) => s.name.startsWith("pasted")).length + 1;
    addSources([{ name: n === 1 ? "pasted.yaml" : `pasted-${n}.yaml`, content: pasteText }]);
    setPasteText("");
    setPasting(false);
  };

  const run = async (dryRun: boolean) => {
    if (!dryRun) {
      const ok = await confirmDestructive(
        `Import ${picked.size} object${picked.size === 1 ? "" : "s"} into "${target?.name}"${nsOverride.trim() ? ` (namespace ${nsOverride.trim()})` : ""}?\n\nExisting objects with the same names are updated.`,
        "Import manifests",
      );
      if (!ok) return;
    }
    setBusy(dryRun ? "dry" : "import");
    setError(null);
    setResults(null);
    try {
      const rows = await ipc.importManifests(sources, [...picked], targetId, nsOverride.trim() || null, dryRun);
      setResults({ dryRun, rows });
      if (!dryRun) {
        const failed = rows.filter((r) => r.outcome === "error").length;
        if (failed) toast.error(`${failed} of ${rows.length} objects failed to import`);
        else toast.success(`Imported ${rows.length} object${rows.length === 1 ? "" : "s"} into ${target?.name}`);
      }
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const toggle = (i: number) =>
    setPicked((s) => {
      const n = new Set(s);
      if (n.has(i)) n.delete(i);
      else n.add(i);
      return n;
    });

  return (
    <Modal title="Import manifests" onClose={close} wide>
      <div className="flex flex-col gap-4 text-sm">
        <div className="flex flex-wrap items-center gap-2">
          <button className="btn-ghost" onClick={() => void chooseFiles()} disabled={!!busy}>
            {busy === "read" ? <Spinner size={14} /> : <FileUp size={14} />} Choose files…
          </button>
          <button className="btn-ghost" onClick={() => setPasting((p) => !p)}>
            <ClipboardPaste size={14} /> Paste YAML
          </button>
          <span className="text-xs text-content-muted">Multi-document files and kubectl List exports are fine.</span>
        </div>

        {pasting && (
          <div className="flex flex-col gap-2">
            <textarea
              className="field mono h-40 w-full resize-y"
              placeholder={"apiVersion: apps/v1\nkind: Deployment\n..."}
              value={pasteText}
              onChange={(e) => setPasteText(e.target.value)}
              autoFocus
            />
            <div className="flex justify-end">
              <button className="btn-primary" onClick={addPaste} disabled={!pasteText.trim()}>
                Add
              </button>
            </div>
          </div>
        )}

        {sources.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            {sources.map((s) => (
              <span key={s.name} className="inline-flex items-center gap-1 rounded bg-muted px-2 py-0.5 text-xs text-content-secondary">
                {s.name}
                <button className="text-content-muted hover:text-content" title="Remove" onClick={() => setSources((cur) => cur.filter((c) => c.name !== s.name))}>
                  <X size={12} />
                </button>
              </span>
            ))}
          </div>
        )}

        {docs.length > 0 && (
          <section>
            <div className="mb-1 flex items-center justify-between">
              <h3 className="field-label !mb-0">
                {valid.length} object{valid.length === 1 ? "" : "s"} found{docs.length > valid.length && ` · ${docs.length - valid.length} with problems`}
              </h3>
              <button
                className="text-xs text-accent hover:underline"
                onClick={() => setPicked(picked.size === valid.length ? new Set() : new Set(valid.map((d) => d.index)))}
              >
                {picked.size === valid.length ? "Select none" : "Select all"}
              </button>
            </div>
            <div className="max-h-56 overflow-auto rounded-md bg-muted p-1">
              {docs.map((d) => (
                <label key={d.index} className={`flex items-center gap-2 rounded px-2 py-1 text-xs ${d.error ? "" : "cursor-pointer hover:bg-muted"}`}>
                  <input type="checkbox" className="accent-brand" disabled={!!d.error} checked={picked.has(d.index)} onChange={() => toggle(d.index)} />
                  <span className="w-32 shrink-0 truncate text-content-muted" title={d.kind ?? ""}>{d.kind ?? "?"}</span>
                  <span className="mono min-w-0 flex-1 truncate text-content" title={d.name ?? ""}>{d.name ?? "—"}</span>
                  {d.error ? (
                    <span className="truncate text-critical" title={d.error}>{d.error}</span>
                  ) : (
                    <span className="shrink-0 text-content-muted">{nsOverride.trim() || d.namespace || "default"}</span>
                  )}
                  <span className="w-28 shrink-0 truncate text-right text-[11px] text-content-muted" title={d.source}>{d.source}</span>
                </label>
              ))}
            </div>
          </section>
        )}

        {docs.length > 0 && (
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
              <span className="field-label">Namespace</span>
              <input className="field w-full" value={nsOverride} placeholder="Keep each object's own" onChange={(e) => setNsOverride(e.target.value)} />
              <span className="mt-1 block text-[11px] text-content-muted">Objects without one go to default. Missing namespaces are created.</span>
            </label>
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
            <h3 className="field-label">{results.dryRun ? "Dry run: what would happen (in apply order)" : "Result (in apply order)"}</h3>
            <div className="flex max-h-48 flex-col gap-1 overflow-auto">
              {results.rows.map((r) => (
                <div key={r.index} className="flex items-start gap-2 text-xs">
                  {r.outcome === "error" ? (
                    <ShieldAlert size={14} className="mt-px shrink-0 text-critical" />
                  ) : (
                    <CheckCircle2 size={14} className="mt-px shrink-0 text-good" />
                  )}
                  <span className="w-32 shrink-0 truncate text-content-muted">{r.kind}</span>
                  <span className="mono w-40 shrink-0 truncate text-content" title={r.name}>{r.name}</span>
                  <span className="min-w-0 flex-1 text-content-secondary">
                    {r.outcome === "error" ? r.message : `${results.dryRun ? "would be " : ""}${r.outcome}${r.namespace ? ` in ${r.namespace}` : ""}${r.message ? ` · ${r.message}` : ""}`}
                  </span>
                </div>
              ))}
            </div>
          </section>
        )}

        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={close}>
            Close
          </button>
          <button className="btn-ghost" disabled={!!busy || picked.size === 0} onClick={() => void run(true)}>
            {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
          </button>
          <button className="btn-primary" disabled={!!busy || picked.size === 0} onClick={() => void run(false)}>
            {busy === "import" ? <Spinner size={14} /> : null} Import {picked.size > 0 && picked.size}
          </button>
        </div>
      </div>
    </Modal>
  );
}
