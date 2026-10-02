import { useEffect, useMemo, useState } from "react";
import { Archive, ArchiveRestore, Download, FileCode2, FolderOpen, Play, ScrollText } from "lucide-react";
import * as ipc from "../lib/ipc";
import { restoreWorkload, runAction } from "../lib/actions";
import { errorMessage, fmtAge, fmtAgo, fmtCount, fmtDateTime } from "../lib/format";
import type { ArchiveMeta, LogSource, PodInfo, WorkloadInfo } from "../lib/types";
import { podsOf, workloadHealth } from "../lib/workloads";
import { findArchive, useArchivesStore } from "../stores/archives";
import { useClusterStore } from "../stores/cluster";
import { useNavStore, type WorkloadTab } from "../stores/nav";
import { toast } from "../stores/toast";
import IssueCard from "./IssueCard";
import { Manifest, ObjectEvents } from "./PodDrawer";
import StoredLogs from "./StoredLogs";
import { FileBrowser } from "./FileBrowser";
import { Drawer, EmptyState, Spinner, StatusPill } from "./ui";

const TABS: { id: WorkloadTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "logs", label: "Stored logs" },
  { id: "files", label: "Files" },
  { id: "events", label: "Events" },
  { id: "yaml", label: "YAML" },
];

export const ARCHIVABLE = ["Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"];

/**
 * One workload: live state when it's on the cluster, plus everything kept
 * locally (stored logs of all its pods, its archive), so it still opens when
 * it's scaled to zero, deleted or archived.
 */
export default function WorkloadDrawer() {
  const { workload: target, closeWorkload, openWorkload, openEditor, openExport, openArchive, openRestore, openLogs } = useNavStore();
  const snapshot = useClusterStore((s) => s.snapshot);
  const clusterId = useClusterStore((s) => s.status?.clusterId ?? s.snapshot?.clusterId);
  const archives = useArchivesStore((s) => s.archives);
  const loadArchives = useArchivesStore((s) => s.load);
  useEffect(() => {
    if (archives == null) void loadArchives();
  }, [archives, loadArchives]);

  const [sources, setSources] = useState<LogSource[] | null>(null);
  const logTick = useClusterStore((s) => s.logSyncTick);
  useEffect(() => {
    if (!target) return;
    ipc
      .logSources(target.namespace, { kind: target.kind, name: target.name })
      .then(setSources)
      .catch(() => setSources([]));
  }, [target?.kind, target?.namespace, target?.name, logTick]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!target) return null;
  // A pinned archive from another cluster says nothing about this one's objects.
  const otherCluster = !!target.archiveId && !!archives && archives.find((a) => a.id === target.archiveId)?.clusterId !== clusterId;
  const w = otherCluster
    ? undefined
    : snapshot?.workloads.find((x) => x.kind === target.kind && x.namespace === target.namespace && x.name === target.name);
  const archive = target.archiveId ? archives?.find((a) => a.id === target.archiveId) : findArchive(archives, clusterId, target);
  const setTab = (tab: WorkloadTab) => openWorkload(target, tab);
  const logsHere = !otherCluster; // stored logs are per cluster
  const ref = { kind: target.kind, namespace: target.namespace, name: target.name };

  return (
    <Drawer
      title={target.name}
      subtitle={
        <>
          {target.kind} · {target.namespace} ·{" "}
          {w ? workloadHealth(w).label : archive ? `archived ${fmtAgo(archive.archivedMs)}` : "not on the cluster"}
        </>
      }
      onClose={closeWorkload}
      actions={
        <>
          <button className="btn-ghost" onClick={() => openLogs({ namespace: target.namespace, workload: { kind: target.kind, name: target.name } })} title="Search every pod's stored logs in the Log explorer">
            <ScrollText size={14} /> Log explorer
          </button>
          {w && (
            <button className="btn-ghost" onClick={() => openEditor(ref)} title="Edit YAML">
              <FileCode2 size={14} /> Edit
            </button>
          )}
          {w && (
            <button className="btn-ghost" onClick={() => openExport(ref)} title="Export with its Services and config">
              <Download size={14} /> Export
            </button>
          )}
          {w && ARCHIVABLE.includes(w.kind) && (
            <button className="btn-ghost" onClick={() => openArchive(ref)} title="Save it to a local folder, then remove it from the cluster">
              <Archive size={14} /> Archive
            </button>
          )}
          {!w && archive && (
            <button className="btn-primary" onClick={() => openRestore(archive)} title="Deploy it again from its archive">
              <ArchiveRestore size={14} /> Restore
            </button>
          )}
        </>
      }
    >
      <div className="flex h-full flex-col">
        <div className="flex shrink-0 gap-1 border-b border-border-light px-4">
          {TABS.filter((t) => t.id !== "files" || (w?.claims.length ?? 0) > 0).map((t) => (
            <button key={t.id} className={`navtab ${target.tab === t.id ? "navtab-active" : ""}`} onClick={() => setTab(t.id)}>
              {t.label}
              {t.id === "logs" && sources && sources.length > 0 && (
                <span className="text-content-muted"> {fmtCount(sources.reduce((s, x) => s + x.lines, 0))}</span>
              )}
            </button>
          ))}
        </div>
        <div className="min-h-0 flex-1 overflow-auto">
          {target.tab === "overview" && (
            <Overview target={ref} w={w} archive={archive} sources={logsHere ? sources : []} pods={otherCluster ? [] : snapshot?.pods ?? []} />
          )}
          {target.tab === "logs" &&
            (logsHere ? (
              <StoredLogs namespace={target.namespace} workload={{ kind: target.kind, name: target.name }} sources={sources} />
            ) : (
              <EmptyState title="Stored on another cluster">
                Switch to {archive?.connectionName ?? "that cluster"} to search these logs, or open logs.txt in the archive folder.
              </EmptyState>
            ))}
          {target.tab === "files" &&
            (w && w.claims.length > 0 ? (
              <WorkloadFiles namespace={w.namespace} claims={w.claims} />
            ) : (
              <EmptyState title="No volumes">It doesn't mount any PersistentVolumeClaims.</EmptyState>
            ))}
          {target.tab === "events" && <ObjectEvents kind={target.kind} namespace={target.namespace} name={target.name} />}
          {target.tab === "yaml" &&
            (w ? (
              <Manifest kind={target.kind} namespace={target.namespace} name={target.name} />
            ) : archive ? (
              <ArchivedManifest id={archive.id} />
            ) : (
              <EmptyState title="Not on the cluster">There's no live object or archive to show YAML for.</EmptyState>
            ))}
        </div>
      </div>
    </Drawer>
  );
}

function Overview({
  target,
  w,
  archive,
  sources,
  pods,
}: {
  target: { kind: string; namespace: string; name: string };
  w: WorkloadInfo | undefined;
  archive: ArchiveMeta | undefined;
  sources: LogSource[] | null;
  pods: PodInfo[];
}) {
  const { openPod, openLogs, openWorkload } = useNavStore();
  const allIssues = useClusterStore((s) => s.snapshot?.issues);
  const issues = useMemo(
    () => allIssues?.filter((i) => i.kind === target.kind && i.namespace === target.namespace && i.name === target.name) ?? [],
    [allIssues, target.kind, target.namespace, target.name],
  );
  const live = useMemo(() => podsOf(pods, target), [pods, target]);
  const liveNames = useMemo(() => new Set(live.map((p) => p.name)), [live]);
  const h = w && workloadHealth(w);

  return (
    <div className="flex flex-col gap-5 p-5">
      {!w && archive && <ArchiveBanner archive={archive} />}
      {!w && !archive && (
        <div className="tint-muted rounded-md px-3 py-2 text-xs">
          Not on the cluster: it was deleted, or it lives on another cluster. What's stored locally (its pods' logs) is still
          below.
        </div>
      )}
      {w && w.desired === 0 && (w.kind === "Deployment" || w.kind === "StatefulSet") && (
        <div className="tint-muted flex items-center gap-3 rounded-md px-3 py-2 text-xs">
          <span className="flex-1">
            Scaled to 0: no pods are running. Their stored logs are still under <b>Stored logs</b>.
          </span>
          {w.disabledReplicas != null ? (
            <button className="btn-chip" onClick={() => void restoreWorkload({ ...target, remembered: w.disabledReplicas })}>
              <Play size={12} /> Start ({w.disabledReplicas})
            </button>
          ) : (
            <button className="btn-chip" onClick={() => void runAction("scale", { ...target, replicas: 0 })}>
              Scale…
            </button>
          )}
        </div>
      )}

      {issues.length > 0 && (
        <div className="flex flex-col gap-2">
          {issues.map((i) => (
            <IssueCard key={i.key} issue={i} compact />
          ))}
        </div>
      )}

      {w && h && (
        <div className="grid grid-cols-2 gap-x-8 gap-y-3 text-sm md:grid-cols-4">
          <Field label="Health">
            <StatusPill status={h.label} tone={h.tone} />
          </Field>
          {w.kind === "CronJob" ? (
            <>
              <Field label="Schedule"><span className="mono">{w.schedule}</span></Field>
              <Field label="Last run">{fmtAgo(w.lastScheduleMs)}</Field>
            </>
          ) : (
            <>
              <Field label={w.kind === "Job" ? "Succeeded" : "Ready"}>
                {w.ready}/{w.desired}
                {w.failed > 0 && <span className="text-critical"> · {w.failed} failed</span>}
              </Field>
              <Field label="Up to date">{w.updated}</Field>
            </>
          )}
          <Field label="Age">{fmtAge(w.createdMs)}</Field>
          {w.conditionMessage && (
            <div className="col-span-full text-xs text-content-secondary">{w.conditionMessage}</div>
          )}
        </div>
      )}

      {(w?.images ?? archive?.images ?? []).length > 0 && (
        <section>
          <h3 className="card-title mb-2">Images</h3>
          <div className="flex flex-col gap-1">
            {(w?.images ?? archive?.images ?? []).map((img) => (
              <span key={img} className="mono truncate text-content-secondary" title={img}>{img}</span>
            ))}
          </div>
        </section>
      )}

      {w && (
        <section>
          <h3 className="card-title mb-2">Running pods</h3>
          <div className="flex flex-wrap gap-2">
            {live.length === 0 && <span className="text-xs text-content-muted">None right now.</span>}
            {live.map((p) => (
              <button key={p.name} className="btn-chip" onClick={() => openPod(p.namespace, p.name)}>
                <StatusPill status={p.status} /> <span className="ml-1">{p.name}</span>
              </button>
            ))}
          </div>
        </section>
      )}

      <section>
        <h3 className="card-title mb-1">Pods with stored logs</h3>
        <p className="mb-2 text-xs text-content-muted">Every pod it has had whose logs are still in the local store, including ones that are gone.</p>
        {sources == null ? (
          <Spinner />
        ) : sources.length === 0 ? (
          <p className="text-xs text-content-muted">No stored lines. Logs are collected while pods run (unless the namespace is excluded in Settings).</p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Pod</th>
                <th className="text-right">Lines</th>
                <th className="text-right">Errors</th>
                <th className="text-right">First line</th>
                <th className="text-right">Last line</th>
              </tr>
            </thead>
            <tbody>
              {sources.map((s) => {
                const alive = liveNames.has(s.pod);
                return (
                  <tr
                    key={s.pod}
                    className="cursor-pointer"
                    title={alive ? "Open pod" : "Open its logs"}
                    onClick={() => (alive ? openPod(s.namespace, s.pod) : openLogs({ namespace: s.namespace, pod: s.pod }))}
                  >
                    <td>
                      <span className="mono text-content">{s.pod}</span>
                      {!alive && <span className="tint-muted ml-2 rounded px-1.5 text-[10px]">gone</span>}
                    </td>
                    <td className="text-right tabular-nums">{fmtCount(s.lines)}</td>
                    <td className={`text-right tabular-nums ${s.errors ? "text-critical" : "text-content-muted"}`}>{fmtCount(s.errors)}</td>
                    <td className="whitespace-nowrap text-right text-xs text-content-muted" title={fmtDateTime(s.firstMs)}>{fmtAgo(s.firstMs)}</td>
                    <td className="whitespace-nowrap text-right text-xs text-content-muted" title={fmtDateTime(s.lastMs)}>{fmtAgo(s.lastMs)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        {sources && sources.length > 0 && (
          <button className="mt-2 text-xs text-accent hover:underline" onClick={() => openWorkload(target, "logs")}>
            Read them →
          </button>
        )}
      </section>

      {w && archive && (
        <p className="text-xs text-content-muted">
          An older archive of it exists (from {fmtDateTime(archive.archivedMs)}); archiving again replaces it.
        </p>
      )}
    </div>
  );
}

export function ArchiveBanner({ archive }: { archive: ArchiveMeta }) {
  const openRestore = useNavStore((s) => s.openRestore);
  const removed = archive.objects.filter((o) => o.removed).length;
  return (
    <div className="card flex flex-col gap-2 p-3 text-xs">
      <div className="flex items-center gap-2">
        <Archive size={14} className="text-content-muted" />
        <span className="flex-1 text-content">
          Archived {fmtDateTime(archive.archivedMs)} from <b>{archive.connectionName}</b>
          {archive.replicas != null && ` with ${archive.replicas} replica${archive.replicas === 1 ? "" : "s"}`}
          {archive.restoredMs && (
            <span className="text-content-muted"> · restored {fmtAgo(archive.restoredMs)}{archive.restoredTo && ` to ${archive.restoredTo}`}</span>
          )}
        </span>
        <button className="btn-chip" onClick={() => void ipc.openArchiveFolder(archive.id).catch((e) => toast.error(errorMessage(e)))}>
          <FolderOpen size={12} /> Folder
        </button>
        <button className="btn-chip" onClick={() => openRestore(archive)}>
          <ArchiveRestore size={12} /> Restore
        </button>
      </div>
      <div className="flex flex-wrap gap-1.5">
        {archive.objects.map((o) => (
          <span
            key={`${o.kind}/${o.name}`}
            className={`rounded px-1.5 py-0.5 ${o.error ? "tint-critical" : "bg-muted text-content-secondary"}`}
            title={o.error ?? (o.removed ? "Saved and removed from the cluster" : "Saved; left on the cluster")}
          >
            {o.kind} <span className="mono">{o.name}</span>
            {!o.removed && !o.error && <span className="text-content-muted"> · kept</span>}
          </span>
        ))}
      </div>
      <p className="text-content-muted">
        {archive.objects.length} object{archive.objects.length === 1 ? "" : "s"} saved, {removed} removed from the cluster
        {archive.logLines > 0 && ` · ${fmtCount(archive.logLines)} log lines in logs.txt`}.
      </p>
    </div>
  );
}

/** The file browser for one of the workload's claims. */
function WorkloadFiles({ namespace, claims }: { namespace: string; claims: string[] }) {
  const [claim, setClaim] = useState(claims[0]);
  const current = claims.includes(claim) ? claim : claims[0];
  return (
    <div className="flex h-full flex-col">
      {claims.length > 1 && (
        <div className="flex shrink-0 items-center gap-2 border-b border-border-light px-4 py-2 text-xs text-content-secondary">
          Volume
          <select className="field" value={current} onChange={(e) => setClaim(e.target.value)}>
            {claims.map((c) => (
              <option key={c} value={c}>{c}</option>
            ))}
          </select>
        </div>
      )}
      <div className="min-h-0 flex-1">
        <FileBrowser namespace={namespace} claim={current} />
      </div>
    </div>
  );
}

function ArchivedManifest({ id }: { id: string }) {
  const [yaml, setYaml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    ipc.archiveManifest(id).then(setYaml).catch((e) => setError(errorMessage(e)));
  }, [id]);
  if (error) return <EmptyState title="Couldn't read the archive">{error}</EmptyState>;
  if (yaml == null) return <EmptyState icon={<Spinner />} title="Loading…" />;
  return (
    <div className="flex h-full flex-col">
      <div className="tint-muted shrink-0 px-4 py-2 text-xs">From the archive's manifest.yaml (what Restore applies).</div>
      <pre className="mono min-h-0 flex-1 overflow-auto whitespace-pre p-4 text-content-secondary">{yaml}</pre>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="text-[11px] text-content-muted">{label}</div>
      <div className="text-content">{children}</div>
    </div>
  );
}
