import { Fragment, useMemo, useState } from "react";
import { Copy, Download, FileCode2, FileUp, RotateCcw, Scaling, Trash2 } from "lucide-react";
import { useImportDialog } from "../components/ImportDialog";
import { runAction, useDeleteDialog } from "../lib/actions";
import * as ipc from "../lib/ipc";
import { saveYamlFile } from "../lib/files";
import { errorMessage } from "../lib/format";
import { toast } from "../stores/toast";
import { fmtAge, fmtAgo } from "../lib/format";
import type { WorkloadInfo } from "../lib/types";
import SnapshotGate from "../components/SnapshotGate";
import { NamespaceSelect, PageHeader, SearchInput, StatusPill } from "../components/ui";
import { useNavStore } from "../stores/nav";

const KINDS: WorkloadInfo["kind"][] = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];

function health(w: WorkloadInfo): { label: string; tone: "good" | "warning" | "critical" | "muted" } {
  if (w.kind === "CronJob") return w.paused ? { label: "Suspended", tone: "muted" } : { label: "Scheduled", tone: "good" };
  if (w.kind === "Job") {
    if (w.conditionMessage) return { label: "Failed", tone: "critical" };
    if (w.ready >= w.desired) return { label: "Complete", tone: "muted" };
    return { label: "Running", tone: "good" };
  }
  if (w.desired === 0)
    return { label: w.disabledReplicas != null ? `Scaled to 0 (was ${w.disabledReplicas})` : "Scaled to 0", tone: "muted" };
  if (w.ready === 0) return { label: "Down", tone: "critical" };
  if (w.ready < w.desired) return { label: "Degraded", tone: "warning" };
  if (w.conditionMessage) return { label: "Rollout stalled", tone: "warning" };
  return { label: "Healthy", tone: "good" };
}

async function exportAll(kind: string, namespace: string | null) {
  try {
    const y = await ipc.exportManifests(kind, namespace, null);
    if (!y.trim()) {
      toast.info(`No ${kind}s to export`);
      return;
    }
    const path = await saveYamlFile(`${kind.toLowerCase()}s${namespace ? `-${namespace}` : ""}.yaml`, y);
    if (path) toast.success(`Exported to ${path}`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

async function exportOne(kind: string, namespace: string, name: string) {
  try {
    const y = await ipc.exportManifests(kind, namespace, name);
    const path = await saveYamlFile(`${name}.${kind.toLowerCase()}.yaml`, y);
    if (path) toast.success(`Exported to ${path}`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

export default function WorkloadsPage() {
  const [kind, setKind] = useState<WorkloadInfo["kind"]>("Deployment");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const openPod = useNavStore((s) => s.openPod);

  return (
    <SnapshotGate>
      {(s) => {
        const count = (k: string) => s.workloads.filter((w) => w.kind === k).length;
        return (
          <div className="flex h-full flex-col">
            <PageHeader title="Workloads">
              <button className="btn-ghost" title="Apply YAML files (one or many) to a cluster" onClick={() => useImportDialog.getState().show()}>
                <FileUp size={14} /> Import
              </button>
              <button
                className="btn-ghost"
                title={`Save every ${kind} ${namespace ? `in ${namespace}` : "in all namespaces"} as one YAML file`}
                onClick={() => void exportAll(kind, namespace || null)}
              >
                <Download size={14} /> Export all
              </button>
              <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />
              <SearchInput value={search} onChange={setSearch} className="w-56" />
            </PageHeader>
            <div className="flex gap-1 border-b border-border-light px-6">
              {KINDS.map((k) => (
                <button key={k} className={`navtab ${kind === k ? "navtab-active" : ""}`} onClick={() => setKind(k)}>
                  {k}s <span className="text-content-muted">{count(k)}</span>
                </button>
              ))}
            </div>
            <WorkloadTable
              rows={s.workloads.filter(
                (w) =>
                  w.kind === kind &&
                  (!namespace || w.namespace === namespace) &&
                  (!search || w.name.toLowerCase().includes(search.toLowerCase())),
              )}
              kind={kind}
              podsFor={(w) =>
                s.pods.filter((p) => p.namespace === w.namespace && p.ownerName && (p.ownerName === w.name || p.ownerName.startsWith(`${w.name}-`)))
              }
              openPod={openPod}
            />
          </div>
        );
      }}
    </SnapshotGate>
  );
}

function WorkloadTable({
  rows,
  kind,
  podsFor,
  openPod,
}: {
  rows: WorkloadInfo[];
  kind: WorkloadInfo["kind"];
  podsFor: (w: WorkloadInfo) => { namespace: string; name: string; status: string }[];
  openPod: (ns: string, name: string) => void;
}) {
  const [expanded, setExpanded] = useState<string | null>(null);
  const { openEditor, openCopy } = useNavStore();
  const sorted = useMemo(() => {
    const rank = { critical: 0, warning: 1, good: 2, muted: 3 };
    return [...rows].sort((a, b) => rank[health(a).tone] - rank[health(b).tone] || a.name.localeCompare(b.name));
  }, [rows]);
  const restartable = kind === "Deployment" || kind === "StatefulSet" || kind === "DaemonSet";
  const scalable = kind === "Deployment" || kind === "StatefulSet";

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Namespace</th>
            <th>Health</th>
            <th>{kind === "Job" ? "Succeeded" : kind === "CronJob" ? "Schedule" : "Ready"}</th>
            <th>Images</th>
            <th className="text-right">{kind === "CronJob" ? "Last run" : "Age"}</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {sorted.map((w) => {
            const h = health(w);
            const key = `${w.namespace}/${w.name}`;
            const isOpen = expanded === key;
            return (
              <Fragment key={key}>
                <tr className="cursor-pointer" onClick={() => setExpanded(isOpen ? null : key)}>
                  <td className="font-medium text-content">{w.name}</td>
                  <td className="text-content-secondary">{w.namespace}</td>
                  <td>
                    <StatusPill status={h.label} tone={h.tone} />
                    {w.conditionMessage && <div className="max-w-xs truncate text-[11px] text-content-muted" title={w.conditionMessage}>{w.conditionMessage}</div>}
                  </td>
                  <td className="tabular-nums">
                    {kind === "CronJob" ? <span className="mono">{w.schedule}</span> : kind === "Job" ? `${w.ready}/${w.desired}${w.failed ? ` · ${w.failed} failed` : ""}` : `${w.ready}/${w.desired}`}
                  </td>
                  <td className="text-xs text-content-muted" title={w.images.join("\n")}>
                    <div className="max-w-[280px] truncate">{w.images.join(", ")}</div>
                  </td>
                  <td className="text-right text-xs text-content-muted">{kind === "CronJob" ? fmtAgo(w.lastScheduleMs) : fmtAge(w.createdMs)}</td>
                  <td className="whitespace-nowrap text-right" onClick={(e) => e.stopPropagation()}>
                    <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: w.kind, namespace: w.namespace, name: w.name })}>
                      <FileCode2 size={14} />
                    </button>
                    <button className="btn-quiet" title="Export YAML to a file" onClick={() => void exportOne(w.kind, w.namespace, w.name)}>
                      <Download size={14} />
                    </button>
                    <button className="btn-quiet" title="Copy to another cluster or namespace" onClick={() => openCopy({ kind: w.kind, namespace: w.namespace, name: w.name })}>
                      <Copy size={14} />
                    </button>
                    {restartable && (
                      <button className="btn-quiet" title="Rollout restart" onClick={() => void runAction("rolloutRestart", { kind: w.kind, namespace: w.namespace, name: w.name })}>
                        <RotateCcw size={14} />
                      </button>
                    )}
                    {scalable && (
                      <button className="btn-quiet" title="Scale" onClick={() => void runAction("scale", { kind: w.kind, namespace: w.namespace, name: w.name, replicas: w.desired, remembered: w.disabledReplicas })}>
                        <Scaling size={14} />
                      </button>
                    )}
                    <button
                      className="btn-quiet hover:!text-critical"
                      title={`Delete ${w.kind}`}
                      onClick={() => useDeleteDialog.getState().open({ kind: w.kind, namespace: w.namespace, name: w.name })}
                    >
                      <Trash2 size={14} />
                    </button>
                  </td>
                </tr>
                {isOpen && (
                  <tr>
                    <td colSpan={7} className="bg-raised">
                      <div className="flex flex-wrap gap-2 py-1">
                        {podsFor(w).length === 0 && <span className="text-xs text-content-muted">No pods.</span>}
                        {podsFor(w).map((p) => (
                          <button key={p.name} className="btn-chip" onClick={() => openPod(p.namespace, p.name)}>
                            <StatusPill status={p.status} /> <span className="ml-1">{p.name}</span>
                          </button>
                        ))}
                      </div>
                    </td>
                  </tr>
                )}
              </Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
