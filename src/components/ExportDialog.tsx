import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Download } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { saveYamlFile } from "../lib/files";
import { errorMessage, fmtBytes } from "../lib/format";
import type { FileProgress, RelatedRef } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { TransferBar } from "./FileBrowser";
import { Modal, Spinner } from "./ui";

const refKey = (r: { kind: string; name: string }) => `${r.kind}/${r.name}`;

/** Where a claim's files go: next to the YAML, as `<claim>.data.tar.gz`. */
const dataPath = (yamlPath: string, claim: string) =>
  `${yamlPath.slice(0, Math.max(yamlPath.lastIndexOf("/"), yamlPath.lastIndexOf("\\")) + 1)}${claim}.data.tar.gz`;

async function exists(path: string): Promise<boolean> {
  try {
    await ipc.localPathInfo([path]);
    return true;
  } catch {
    return false;
  }
}

/** Export a workload together with the objects that make it run. */
export default function ExportDialog() {
  const { exporting: target, closeExport } = useNavStore();
  const [related, setRelated] = useState<RelatedRef[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [withData, setWithData] = useState(false);
  /** What the data copy is doing: starting a claim's helper pod, or streaming its files. */
  const [stage, setStage] = useState<string | null>(null);
  const [transfer, setTransfer] = useState<FileProgress | null>(null);
  const transferId = useRef<string | null>(null);
  const stopped = useRef(false);

  useEffect(() => {
    // StrictMode runs this cleanup once before the real mount, so clear it again here.
    stopped.current = false;
    const un = listen<FileProgress>("files:progress", (e) => {
      if (e.payload.transferId === transferId.current) setTransfer(e.payload);
    });
    // Closing the dialog mid-copy stops it.
    return () => {
      stopped.current = true;
      void un.then((f) => f());
      if (transferId.current) void ipc.cancelTransfer(transferId.current);
    };
  }, []);

  useEffect(() => {
    if (!target) return;
    setRelated(null);
    setError(null);
    ipc
      .relatedObjects(target.kind, target.namespace, target.name)
      .then((r) => {
        setRelated(r);
        // Unlike Copy, an export should carry the claims its pods mount: without them the
        // imported workload sits Pending. Only the claim is written, never the data.
        setPicked(new Set(r.filter((x) => x.exists && (x.defaultSelected || x.kind === "PersistentVolumeClaim")).map(refKey)));
      })
      .catch((e) => {
        // The workload itself can still be exported without what belongs to it.
        setRelated([]);
        setError(`Couldn't look up what belongs with it (${errorMessage(e)}). You can still export the workload on its own.`);
      });
  }, [target]);

  const secretsPicked = useMemo(() => (related ?? []).some((r) => r.sensitive && picked.has(refKey(r))), [related, picked]);
  const claimsPicked = useMemo(
    () => (related ?? []).filter((r) => r.kind === "PersistentVolumeClaim" && picked.has(refKey(r))).map((r) => r.name),
    [related, picked],
  );
  if (!target) return null;

  /** Copy each claim's files next to the YAML. Stops at the first failure; returns what was saved. */
  const saveData = async (yamlPath: string, claims: string[]) => {
    const saved: string[] = [];
    const live: string[] = [];
    for (const claim of claims) {
      setStage(`Starting a helper pod for ${claim}…`);
      const session = await ipc.openVolumeFiles(target.namespace, claim);
      try {
        // Closed while the helper was starting.
        if (stopped.current) throw new Error("Cancelled.");
        if (session.mountedBy.length) live.push(claim);
        setStage(null);
        const id = crypto.randomUUID();
        transferId.current = id;
        setTransfer({ transferId: id, label: claim, done: 0, total: null });
        const bytes = await ipc.downloadVolumePath(session.id, "", true, dataPath(yamlPath, claim), id);
        saved.push(`${claim} (${fmtBytes(bytes)})`);
      } catch (e) {
        throw new Error(`${claim}: ${errorMessage(e)}${saved.length ? ` Saved before it: ${saved.join(", ")}.` : ""}`);
      } finally {
        transferId.current = null;
        setTransfer(null);
        void ipc.closeVolumeFiles(session.id);
      }
    }
    return { saved, live };
  };

  const save = async () => {
    setBusy(true);
    try {
      const extras = (related ?? []).filter((r) => picked.has(refKey(r))).map(({ kind, name }) => ({ kind, name }));
      const yaml = await ipc.exportBundle(target.kind, target.namespace, target.name, extras);
      const file = extras.length ? `${target.name}-bundle.yaml` : `${target.name}.${target.kind.toLowerCase()}.yaml`;
      const path = await saveYamlFile(file, yaml);
      if (!path) return;
      const objects = `${extras.length + 1} object${extras.length ? "s" : ""}`;
      const claims = withData ? claimsPicked : [];
      if (claims.length) {
        const taken = (await Promise.all(claims.map((c) => dataPath(path, c)).map(async (p) => ((await exists(p)) ? p : null)))).filter(Boolean);
        if (taken.length && !(await confirmDestructive(`Replace these files?\n\n${taken.join("\n")}`, "Replace volume data?"))) {
          toast.info(`Exported ${objects} to ${path}, without volume data`);
          return;
        }
        try {
          const { saved, live } = await saveData(path, claims);
          toast.success(`Exported ${objects} to ${path}, plus the files of ${saved.join(", ")}`);
          if (live.length) toast.info(`${live.join(", ")} ${live.length > 1 ? "were" : "was"} in use while copying, so the copy may not be consistent.`);
        } catch (e) {
          toast.error(`Exported ${objects} to ${path}, but copying volume data failed. ${errorMessage(e)}`);
          return;
        }
      } else {
        toast.success(`Exported ${objects} to ${path}`);
      }
      closeExport();
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(false);
      setStage(null);
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
    <Modal title={`Export ${target.kind} ${target.name}`} onClose={closeExport} wide explicitClose={busy}>
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
        {claimsPicked.length > 0 && (
          <section className="flex flex-col gap-1">
            <label className="flex cursor-pointer items-center gap-2 text-xs">
              <input type="checkbox" className="accent-brand" disabled={busy} checked={withData} onChange={(e) => setWithData(e.target.checked)} />
              <span className="text-content">Include the files on {claimsPicked.length > 1 ? "these volumes" : "this volume"}</span>
            </label>
            <p className="pl-5 text-[11px] text-content-muted">
              Saved next to the YAML as <span className="mono">{claimsPicked.length === 1 ? claimsPicked[0] : "<claim>"}.data.tar.gz</span>, through a
              short-lived helper pod. If the app is running its files are copied while it writes, so stop it first for a consistent copy
              (databases especially).
            </p>
          </section>
        )}
        {(stage || transfer) && (
          <div className="flex items-center gap-2 text-xs text-content-muted">
            {transfer ? <TransferBar transfer={transfer} /> : <><Spinner size={12} /> {stage}</>}
          </div>
        )}
        {secretsPicked && (
          <div className="tint-warning flex items-start gap-2 rounded-md px-3 py-2 text-xs">
            <AlertTriangle size={14} className="mt-px shrink-0" />
            The file will contain the selected Secrets' values (base64, not encrypted). Keep it out of git and shared drives.
          </div>
        )}
        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={closeExport}>
            {busy ? "Stop" : "Cancel"}
          </button>
          <button className="btn-primary" disabled={busy || related == null} onClick={() => void save()}>
            {busy ? <Spinner size={14} /> : <Download size={14} />} Save YAML ({picked.size + 1} object{picked.size ? "s" : ""})
          </button>
        </div>
      </div>
    </Modal>
  );
}
