import { useCallback, useEffect, useMemo, useState, type CSSProperties } from "react";
import { Eye, EyeOff, FileCode2, Plus, RefreshCw, RotateCcw, Save, ShieldAlert, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtBytes } from "../lib/format";
import type { ConfigData } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { Drawer, EmptyState, Spinner } from "./ui";

interface Row {
  id: number;
  key: string;
  value: string;
  binary: boolean;
  size: number;
  reveal: boolean;
}

const KEY_RE = /^[-._a-zA-Z0-9]{1,253}$/;
let nextId = 1;

function toRows(d: ConfigData): Row[] {
  return d.entries.map((e) => ({ id: nextId++, key: e.key, value: e.value ?? "", binary: e.binary, size: e.size, reveal: false }));
}

/** Masks text in a textarea without changing how it's edited (Chromium / WebView2). */
const masked: CSSProperties = { WebkitTextSecurity: "disc" } as CSSProperties;

export default function ConfigEditor() {
  const { config: target, closeConfig, openEditor } = useNavStore();
  const usedBy = useClusterStore(
    (s) => s.snapshot?.configs.find((c) => c.kind === target?.kind && c.namespace === target?.namespace && c.name === target?.name)?.usedBy,
  );
  const [data, setData] = useState<ConfigData | null>(null);
  const [rows, setRows] = useState<Row[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [busy, setBusy] = useState<null | "load" | "save" | "restart">(null);
  const [justSaved, setJustSaved] = useState(false);

  const load = useCallback(async () => {
    if (!target) return;
    setBusy("load");
    setLoadError(null);
    setSaveError(null);
    try {
      const d = await ipc.getConfig(target.kind, target.namespace, target.name);
      setData(d);
      setRows(toRows(d));
    } catch (e) {
      setLoadError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  }, [target]);

  useEffect(() => {
    setJustSaved(false);
    void load();
  }, [load]);

  const dirty = useMemo(() => {
    if (!data) return false;
    const now = rows.map((r) => `${r.key}\u0000${r.binary ? "#bin" : r.value}`).join("\u0001");
    const was = toRows(data).map((r) => `${r.key}\u0000${r.binary ? "#bin" : r.value}`).join("\u0001");
    return now !== was;
  }, [rows, data]);

  const problems = useMemo(() => {
    const out: string[] = [];
    const seen = new Set<string>();
    for (const r of rows) {
      if (!KEY_RE.test(r.key)) out.push(`"${r.key || "(empty)"}" isn't a valid key: letters, digits, "-", "_" and "." only`);
      if (seen.has(r.key)) out.push(`Duplicate key "${r.key}"`);
      seen.add(r.key);
    }
    return out;
  }, [rows]);

  if (!target) return null;
  const isSecret = target.kind === "Secret";
  const readOnly = data?.immutable ?? false;
  const restartable = (usedBy ?? []).filter((u) => /^(Deployment|StatefulSet|DaemonSet)\//.test(u));

  const close = async () => {
    if (dirty && !(await confirmDestructive("Discard your unsaved changes?", "Unsaved changes"))) return;
    closeConfig();
  };

  const reload = async () => {
    if (dirty && !(await confirmDestructive("Reload from the cluster and lose your edits?", "Reload"))) return;
    setJustSaved(false);
    await load();
  };

  const update = (id: number, patch: Partial<Row>) => setRows((rs) => rs.map((r) => (r.id === id ? { ...r, ...patch } : r)));

  const save = async () => {
    if (!data || problems.length) return;
    const ok = await confirmDestructive(
      `Save ${rows.length} key${rows.length === 1 ? "" : "s"} to ${target.kind} ${target.namespace}/${target.name}?` +
        (restartable.length ? `\n\nPods using it keep the old values until they restart.` : ""),
      `Save ${target.kind}`,
    );
    if (!ok) return;
    setBusy("save");
    setSaveError(null);
    try {
      const text: Record<string, string> = {};
      rows.filter((r) => !r.binary).forEach((r) => (text[r.key] = r.value));
      const keepBinary = rows.filter((r) => r.binary).map((r) => r.key);
      const d = await ipc.saveConfig(target.kind, target.namespace, target.name, data.resourceVersion, text, keepBinary);
      setData(d);
      setRows(toRows(d));
      setJustSaved(true);
      toast.success(`Saved ${target.name}`);
    } catch (e) {
      setSaveError(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const restartUsers = async () => {
    setBusy("restart");
    const failures: string[] = [];
    for (const u of restartable) {
      const [kind, name] = u.split("/");
      try {
        await ipc.rolloutRestart(kind, target.namespace, name);
      } catch (e) {
        failures.push(`${u}: ${errorMessage(e)}`);
      }
    }
    setBusy(null);
    setJustSaved(false);
    if (failures.length) toast.error(failures.join("\n"));
    else toast.success(`Restarting ${restartable.join(", ")}`);
  };

  return (
    <Drawer
      title={`${target.kind} ${target.name}`}
      subtitle={`${target.namespace}${data?.secretType ? ` · ${data.secretType}` : ""}${dirty ? " · unsaved changes" : ""}`}
      onClose={() => void close()}
      actions={
        <>
          <button className="btn-ghost" title="Reload from cluster" disabled={!!busy} onClick={() => void reload()}>
            {busy === "load" ? <Spinner size={14} /> : <RefreshCw size={14} />}
          </button>
          <button className="btn-ghost" onClick={() => openEditor({ ...target })} title="Edit the full YAML (labels, annotations…)">
            <FileCode2 size={14} /> YAML
          </button>
          <button className="btn-primary" disabled={!dirty || !!busy || problems.length > 0 || readOnly} onClick={() => void save()}>
            {busy === "save" ? <Spinner size={14} /> : <Save size={14} />} Save
          </button>
        </>
      }
    >
      {loadError ? (
        <EmptyState title="Couldn't load">{loadError}</EmptyState>
      ) : !data ? (
        <EmptyState icon={<Spinner />} title="Loading…" />
      ) : (
        <div className="flex flex-col gap-3 p-5">
          {readOnly && (
            <div className="tint-warning rounded-md px-3 py-2 text-xs">This {target.kind} is immutable, so Kubernetes won't allow edits. Recreate it to change values.</div>
          )}
          {justSaved && restartable.length > 0 && (
            <div className="tint-info flex items-center gap-3 rounded-md px-3 py-2 text-xs">
              <span className="flex-1">
                Saved. {restartable.join(", ")} still {restartable.length === 1 ? "has" : "have"} the old values until {restartable.length === 1 ? "it restarts" : "they restart"}.
              </span>
              <button className="btn-chip shrink-0 !border-info !text-info" disabled={!!busy} onClick={() => void restartUsers()}>
                {busy === "restart" ? <Spinner size={12} /> : <RotateCcw size={12} />} Restart {restartable.length === 1 ? "it" : "them"} now
              </button>
            </div>
          )}
          {(saveError || problems.length > 0) && (
            <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
              <ShieldAlert size={14} className="mt-px shrink-0" />
              <span className="whitespace-pre-wrap break-words">{saveError ?? problems.join("\n")}</span>
            </div>
          )}
          <div className="text-xs text-content-muted">
            {usedBy?.length ? <>Used by {usedBy.join(", ")}.</> : "Not referenced by any workload's pod spec."}
            {isSecret && " Values are decoded and shown masked; click the eye to reveal one."}
          </div>

          {rows.map((r) => (
            <div key={r.id} className="card flex flex-col gap-1.5 p-3">
              <div className="flex items-center gap-2">
                <input
                  className="field mono flex-1 py-1"
                  value={r.key}
                  placeholder="KEY"
                  disabled={readOnly || r.binary}
                  onChange={(e) => update(r.id, { key: e.target.value })}
                />
                {isSecret && !r.binary && (
                  <button className="btn-quiet" title={r.reveal ? "Hide" : "Reveal"} onClick={() => update(r.id, { reveal: !r.reveal })}>
                    {r.reveal ? <EyeOff size={14} /> : <Eye size={14} />}
                  </button>
                )}
                <button className="btn-quiet hover:!text-critical" title="Remove key" disabled={readOnly} onClick={() => setRows((rs) => rs.filter((x) => x.id !== r.id))}>
                  <Trash2 size={14} />
                </button>
              </div>
              {r.binary ? (
                <div className="rounded-md bg-muted px-2.5 py-1.5 text-xs text-content-muted">Binary value ({fmtBytes(r.size)}). Kept as-is; edit the YAML to change it.</div>
              ) : (
                <textarea
                  className="field mono min-h-[38px] w-full resize-y py-1.5"
                  rows={Math.min(12, Math.max(1, r.value.split("\n").length))}
                  value={r.value}
                  disabled={readOnly}
                  spellCheck={false}
                  style={isSecret && !r.reveal ? masked : undefined}
                  onChange={(e) => update(r.id, { value: e.target.value })}
                />
              )}
            </div>
          ))}
          {!readOnly && (
            <button
              className="btn-ghost self-start"
              onClick={() => setRows((rs) => [...rs, { id: nextId++, key: "", value: "", binary: false, size: 0, reveal: true }])}
            >
              <Plus size={14} /> Add key
            </button>
          )}
        </div>
      )}
    </Drawer>
  );
}
