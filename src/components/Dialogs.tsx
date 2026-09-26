import { useEffect, useState } from "react";
import { AlertOctagon, CheckCircle2, Download, Info, Power, PowerOff, Trash2, X } from "lucide-react";
import { deleteWorkload, restoreWorkload, scaleTo, stopWorkload, useDeleteDialog, useScaleDialog } from "../lib/actions";
import * as ipc from "../lib/ipc";
import { saveYamlFile } from "../lib/files";
import { errorMessage } from "../lib/format";
import { toast, useToastStore } from "../stores/toast";
import { Modal } from "./ui";

export function ScaleDialog() {
  const { target, close } = useScaleDialog();
  const [value, setValue] = useState(0);
  useEffect(() => setValue(target?.replicas ?? 1), [target]);
  if (!target) return null;
  const current = target.replicas ?? 0;
  const canRemember = target.kind === "Deployment" || target.kind === "StatefulSet";

  return (
    <Modal title={`Scale ${target.kind} ${target.name}`} onClose={close}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          close();
          void scaleTo(target, value);
        }}
      >
        <label className="field-label" htmlFor="replicas">
          Replicas {target.replicas != null && <span className="text-content-muted">(currently {current})</span>}
        </label>
        <input
          id="replicas"
          type="number"
          min={0}
          max={100}
          autoFocus
          className="field w-full"
          value={value}
          onChange={(e) => setValue(Math.max(0, parseInt(e.target.value || "0", 10)))}
        />
        {canRemember && current > 0 && (
          <div className="mt-3 flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-2 text-xs text-content-secondary">
            <span>Stop it for now: scale to 0 and remember {current} for later.</span>
            <button
              type="button"
              className="btn-chip shrink-0"
              onClick={() => {
                close();
                void stopWorkload(target);
              }}
            >
              <PowerOff size={12} /> Scale to 0
            </button>
          </div>
        )}
        {canRemember && current === 0 && target.remembered != null && (
          <div className="mt-3 flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-2 text-xs text-content-secondary">
            <span>Scaled to 0 from Portside; it was running {target.remembered}.</span>
            <button
              type="button"
              className="btn-chip shrink-0"
              onClick={() => {
                close();
                void restoreWorkload(target);
              }}
            >
              <Power size={12} /> Restore to {target.remembered}
            </button>
          </div>
        )}
        <div className="mt-4 flex justify-end gap-2">
          <button type="button" className="btn-ghost" onClick={close}>
            Cancel
          </button>
          <button type="submit" className="btn-primary">
            Scale
          </button>
        </div>
      </form>
    </Modal>
  );
}

/** Delete a workload after typing its name, with an export-first escape hatch. */
export function DeleteDialog() {
  const { target, close } = useDeleteDialog();
  const [typed, setTyped] = useState("");
  const [exported, setExported] = useState<string | null>(null);
  useEffect(() => {
    setTyped("");
    setExported(null);
  }, [target]);
  if (!target) return null;
  const ns = target.namespace ?? "";

  const backup = async () => {
    try {
      const y = await ipc.exportManifests(target.kind, ns, target.name);
      const path = await saveYamlFile(`${target.name}.${target.kind.toLowerCase()}.yaml`, y);
      if (path) setExported(path);
    } catch (e) {
      toast.error(errorMessage(e));
    }
  };

  return (
    <Modal title={`Delete ${target.kind} ${target.name}?`} onClose={close}>
      <div className="flex flex-col gap-3 text-sm">
        <p className="text-content-secondary">
          This removes <b className="text-content">{ns}/{target.name}</b> and its pods from the cluster. It can't be undone.
        </p>
        <div className="flex items-center justify-between gap-3 rounded-md bg-muted px-3 py-2 text-xs text-content-secondary">
          {exported ? (
            <span className="min-w-0 break-all">Backed up to {exported}</span>
          ) : (
            <span>Save its YAML first so you can re-create it.</span>
          )}
          <button type="button" className="btn-chip shrink-0" onClick={() => void backup()}>
            <Download size={12} /> {exported ? "Export again" : "Export YAML"}
          </button>
        </div>
        <label className="block">
          <span className="field-label">
            Type <span className="mono text-content">{target.name}</span> to confirm
          </span>
          <input className="field mono w-full" autoFocus value={typed} onChange={(e) => setTyped(e.target.value)} />
        </label>
        <div className="flex justify-end gap-2">
          <button type="button" className="btn-ghost" onClick={close}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-danger"
            disabled={typed !== target.name}
            onClick={() => {
              close();
              void deleteWorkload(target);
            }}
          >
            <Trash2 size={14} /> Delete
          </button>
        </div>
      </div>
    </Modal>
  );
}

export function Toasts() {
  const { toasts, dismiss } = useToastStore();
  return (
    <div className="pointer-events-none fixed bottom-10 right-4 z-[60] flex w-80 flex-col gap-2">
      {toasts.map((t) => {
        const Icon = t.kind === "success" ? CheckCircle2 : t.kind === "error" ? AlertOctagon : Info;
        const color = t.kind === "success" ? "text-good" : t.kind === "error" ? "text-critical" : "text-info";
        return (
          <div key={t.id} className="card pointer-events-auto flex items-start gap-2 px-3 py-2.5 text-sm shadow-[var(--shadow-md)]">
            <Icon size={16} className={`mt-0.5 shrink-0 ${color}`} />
            <span className="min-w-0 flex-1 break-words text-content">{t.message}</span>
            <button className="text-content-muted hover:text-content" onClick={() => dismiss(t.id)}>
              <X size={14} />
            </button>
          </div>
        );
      })}
    </div>
  );
}
