import { useCallback, useEffect, useState } from "react";
import { CheckCircle2, Copy, Download, FlaskConical, RefreshCw, ShieldAlert, Upload } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { saveYamlFile } from "../lib/files";
import { errorMessage } from "../lib/format";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { Drawer, EmptyState, Spinner } from "./ui";
import YamlEditor from "./YamlEditor";

/** Drawer for editing a workload's YAML and applying it back to the cluster. */
export default function ManifestEditor() {
  const { editor: target, closeEditor, openCopy, openExport } = useNavStore();
  const [original, setOriginal] = useState<string | null>(null);
  const [text, setText] = useState("");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [busy, setBusy] = useState<null | "dry" | "apply" | "load" | "export">(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const dirty = original != null && text !== original;

  const load = useCallback(async () => {
    if (!target) return;
    setBusy("load");
    setLoadError(null);
    try {
      const y = await ipc.editManifest(target.kind, target.namespace, target.name);
      setOriginal(y);
      setText(y);
    } catch (e) {
      setLoadError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }, [target]);

  useEffect(() => {
    setResult(null);
    void load();
  }, [load]);

  if (!target) return null;

  const close = async () => {
    if (dirty && !(await confirmDestructive("Discard your unsaved YAML changes?", "Unsaved changes"))) return;
    closeEditor();
  };

  const reload = async () => {
    if (dirty && !(await confirmDestructive("Reload from the cluster and lose your edits?", "Reload"))) return;
    setResult(null);
    await load();
  };

  const run = async (dryRun: boolean) => {
    if (!dryRun && !(await confirmDestructive(`Apply your changes to ${target.kind} ${target.namespace}/${target.name}?`, "Apply YAML"))) return;
    setBusy(dryRun ? "dry" : "apply");
    setResult(null);
    try {
      const msg = await ipc.applyManifest(text, target.kind, target.namespace, target.name, dryRun);
      setResult({ ok: true, text: msg });
      if (!dryRun) {
        toast.success(`Applied ${target.name}`);
        await load(); // pick up the new resourceVersion
      }
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  const isWorkload = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"].includes(target.kind);

  const exportClean = async () => {
    if (isWorkload) {
      openExport(target); // pick related Services/config to include
      return;
    }
    setBusy("export");
    try {
      const y = await ipc.exportManifests(target.kind, target.namespace, target.name);
      const path = await saveYamlFile(`${target.name}.${target.kind.toLowerCase()}.yaml`, y);
      if (path) toast.success(`Exported to ${path}`);
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  return (
    <Drawer
      title={`${target.kind} ${target.name}`}
      subtitle={`${target.namespace} · edit YAML${dirty ? " · unsaved changes" : ""}`}
      onClose={() => void close()}
      actions={
        <>
          <button className="btn-ghost" onClick={() => void reload()} disabled={!!busy} title="Reload from cluster">
            {busy === "load" ? <Spinner size={14} /> : <RefreshCw size={14} />}
          </button>
          <button className="btn-ghost" onClick={() => void exportClean()} disabled={!!busy} title="Save a clean, re-appliable copy to a file">
            {busy === "export" ? <Spinner size={14} /> : <Download size={14} />} Export
          </button>
          {isWorkload && (
            <button className="btn-ghost" onClick={() => openCopy(target)} disabled={!!busy}>
              <Copy size={14} /> Copy to cluster
            </button>
          )}
          <button className="btn-ghost" onClick={() => void run(true)} disabled={!!busy || original == null} title="Ask the server to validate without changing anything">
            {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
          </button>
          <button className="btn-primary" onClick={() => void run(false)} disabled={!!busy || !dirty}>
            {busy === "apply" ? <Spinner size={14} /> : <Upload size={14} />} Apply
          </button>
        </>
      }
    >
      <div className="flex h-full flex-col">
        {result && (
          <div
            className={`flex shrink-0 items-start gap-2 border-b px-4 py-2 text-xs ${
              result.ok ? "tint-good border-border-light" : "tint-critical border-border-light"
            }`}
          >
            {result.ok ? <CheckCircle2 size={14} className="mt-px shrink-0" /> : <ShieldAlert size={14} className="mt-px shrink-0" />}
            <span className="mono whitespace-pre-wrap break-words">{result.text}</span>
          </div>
        )}
        <div className="min-h-0 flex-1">
          {loadError ? (
            <EmptyState title="Couldn't load YAML">{loadError}</EmptyState>
          ) : original == null ? (
            <EmptyState icon={<Spinner />} title="Loading…" />
          ) : (
            <YamlEditor value={text} onChange={setText} />
          )}
        </div>
        <div className="shrink-0 border-t border-border-light px-4 py-1.5 text-[11px] text-content-muted">
          Saves like <span className="mono">kubectl edit</span>: if the object changed on the cluster since you opened it, Apply is
          refused. Reload and reapply. Status and managed fields are hidden.
        </div>
      </div>
    </Drawer>
  );
}
