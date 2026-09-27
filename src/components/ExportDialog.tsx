import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Download } from "lucide-react";
import * as ipc from "../lib/ipc";
import { saveYamlFile } from "../lib/files";
import { errorMessage } from "../lib/format";
import type { RelatedRef } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { Modal, Spinner } from "./ui";

const refKey = (r: { kind: string; name: string }) => `${r.kind}/${r.name}`;

/** Export a workload together with the objects that make it run. */
export default function ExportDialog() {
  const { exporting: target, closeExport } = useNavStore();
  const [related, setRelated] = useState<RelatedRef[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!target) return;
    setRelated(null);
    setError(null);
    ipc
      .relatedObjects(target.kind, target.namespace, target.name)
      .then((r) => {
        setRelated(r);
        setPicked(new Set(r.filter((x) => x.exists && x.defaultSelected).map(refKey)));
      })
      .catch((e) => setError(errorMessage(e)));
  }, [target]);

  const secretsPicked = useMemo(() => (related ?? []).some((r) => r.sensitive && picked.has(refKey(r))), [related, picked]);
  if (!target) return null;

  const save = async () => {
    setBusy(true);
    try {
      const extras = (related ?? []).filter((r) => picked.has(refKey(r))).map(({ kind, name }) => ({ kind, name }));
      const yaml = await ipc.exportBundle(target.kind, target.namespace, target.name, extras);
      const file = extras.length ? `${target.name}-bundle.yaml` : `${target.name}.${target.kind.toLowerCase()}.yaml`;
      const path = await saveYamlFile(file, yaml);
      if (path) {
        toast.success(`Exported ${extras.length + 1} object${extras.length ? "s" : ""} to ${path}`);
        closeExport();
      }
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(false);
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
    <Modal title={`Export ${target.kind} ${target.name}`} onClose={closeExport} wide>
      <div className="flex flex-col gap-4 text-sm">
        <p className="text-xs text-content-muted">
          One YAML file, dependencies first, with cluster-specific fields removed (status, uids, Service cluster IPs, volume
          bindings…) so it applies cleanly anywhere, e.g. with <b className="text-content-secondary">Import</b>.
        </p>
        <section>
          <h3 className="field-label">Include with it</h3>
          {error ? (
            <p className="text-xs text-critical">{error}</p>
          ) : related == null ? (
            <Spinner />
          ) : related.length === 0 ? (
            <p className="text-xs text-content-muted">Nothing related found: no config, Services, Ingresses or autoscalers point at it.</p>
          ) : (
            <div className="flex flex-col gap-1 rounded-md bg-muted p-2">
              {related.map((r) => (
                <label key={refKey(r)} className={`flex items-center gap-2 text-xs ${r.exists ? "cursor-pointer" : ""}`}>
                  <input type="checkbox" className="accent-brand" disabled={!r.exists} checked={picked.has(refKey(r))} onChange={() => toggle(r)} />
                  <span className="w-44 shrink-0 truncate text-content-muted" title={r.kind}>{r.kind}</span>
                  <span className="mono w-44 shrink-0 truncate text-content" title={r.name}>{r.name}</span>
                  <span className="min-w-0 flex-1 truncate text-[11px] text-content-muted" title={r.reason}>
                    {r.exists ? r.reason : "referenced but doesn't exist"}
                  </span>
                </label>
              ))}
            </div>
          )}
        </section>
        {secretsPicked && (
          <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} className="mt-px shrink-0" />
            The file will contain the selected Secrets' values (base64, not encrypted). Keep it out of git and shared drives.
          </div>
        )}
        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={closeExport}>
            Cancel
          </button>
          <button className="btn-primary" disabled={busy || related == null} onClick={() => void save()}>
            {busy ? <Spinner size={14} /> : <Download size={14} />} Save YAML ({picked.size + 1} object{picked.size ? "s" : ""})
          </button>
        </div>
      </div>
    </Modal>
  );
}
