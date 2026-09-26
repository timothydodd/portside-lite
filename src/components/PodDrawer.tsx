import { useCallback, useEffect, useMemo, useState } from "react";
import { FileCode2, RefreshCw, RotateCcw, ScrollText, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { runAction } from "../lib/actions";
import { errorMessage, fmtAge, fmtAgo, fmtBytes, fmtCpu, fmtDateTime } from "../lib/format";
import type { EventInfo, LiveLogLine, LogLevel, PodInfo, Sample } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore, type PodTab } from "../stores/nav";
import { LineChart } from "./charts";
import IssueCard from "./IssueCard";
import LogView from "./LogView";
import { Drawer, EmptyState, Spinner, StatusPill } from "./ui";

const TABS: { id: PodTab; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "logs", label: "Logs" },
  { id: "previous", label: "Crash logs" },
  { id: "events", label: "Events" },
  { id: "yaml", label: "YAML" },
];

export default function PodDrawer() {
  const { pod: target, closePod, openPod, openLogs, openEditor } = useNavStore();
  const snapshot = useClusterStore((s) => s.snapshot);
  const pod = snapshot?.pods.find((p) => p.namespace === target?.namespace && p.name === target?.name);
  if (!target) return null;
  const tab = target.tab;
  const setTab = (t: PodTab) => openPod(target.namespace, target.name, t);
  const restartable = pod?.ownerKind && ["Deployment", "StatefulSet", "DaemonSet"].includes(pod.ownerKind);

  return (
    <Drawer
      title={target.name}
      subtitle={
        <>
          {target.namespace}
          {pod?.node && ` · on ${pod.node}`}
          {pod?.ownerKind && ` · ${pod.ownerKind} ${pod.ownerName}`}
        </>
      }
      onClose={closePod}
      actions={
        <>
          <button className="btn-ghost" onClick={() => openLogs({ namespace: target.namespace, pod: target.name })} title="Search this pod's stored logs">
            <ScrollText size={14} /> Log explorer
          </button>
          {restartable && pod && (
            <button
              className="btn-ghost"
              onClick={() => openEditor({ kind: pod.ownerKind!, namespace: pod.namespace, name: pod.ownerName! })}
              title={`Edit the ${pod.ownerKind}'s YAML`}
            >
              <FileCode2 size={14} /> Edit {pod.ownerKind}
            </button>
          )}
          {restartable && pod && (
            <button
              className="btn-ghost"
              onClick={() => void runAction("rolloutRestart", { kind: pod.ownerKind!, namespace: pod.namespace, name: pod.ownerName! })}
            >
              <RotateCcw size={14} /> Restart {pod.ownerKind}
            </button>
          )}
          <button className="btn-ghost hover:!border-critical" onClick={() => void runAction("deletePod", { kind: "Pod", namespace: target.namespace, name: target.name })}>
            <Trash2 size={14} /> Delete pod
          </button>
        </>
      }
    >
      <div className="flex h-full flex-col">
        <div className="flex shrink-0 gap-1 border-b border-border-light px-4">
          {TABS.map((t) => (
            <button key={t.id} className={`navtab ${tab === t.id ? "navtab-active" : ""}`} onClick={() => setTab(t.id)}>
              {t.label}
            </button>
          ))}
        </div>
        <div className="min-h-0 flex-1">
          {!pod && tab === "overview" ? (
            <EmptyState title="Pod no longer exists">It may have been deleted or replaced since you opened it.</EmptyState>
          ) : (
            <>
              {tab === "overview" && pod && <PodOverview pod={pod} />}
              {tab === "logs" && <LiveLogs namespace={target.namespace} pod={target.name} info={pod} previous={false} />}
              {tab === "previous" && <LiveLogs namespace={target.namespace} pod={target.name} info={pod} previous />}
              {tab === "events" && <ObjectEvents kind="Pod" namespace={target.namespace} name={target.name} />}
              {tab === "yaml" && <Manifest kind="Pod" namespace={target.namespace} name={target.name} />}
            </>
          )}
        </div>
      </div>
    </Drawer>
  );
}

function PodOverview({ pod }: { pod: PodInfo }) {
  // Select the stable array and derive in useMemo: a selector that returns a
  // new array every call makes zustand re-render forever.
  const allIssues = useClusterStore((s) => s.snapshot?.issues);
  const issues = useMemo(
    () => allIssues?.filter((i) => i.kind === "Pod" && i.namespace === pod.namespace && i.name === pod.name),
    [allIssues, pod.namespace, pod.name],
  );
  const [history, setHistory] = useState<Sample[]>([]);
  useEffect(() => {
    const since = Date.now() - 24 * 3600_000;
    ipc.podHistory(pod.namespace, pod.name, since, 5 * 60_000).then(setHistory).catch(() => setHistory([]));
  }, [pod.namespace, pod.name]);

  return (
    <div className="flex flex-col gap-5 p-5">
      {issues && issues.length > 0 && (
        <div className="flex flex-col gap-2">
          {issues.map((i) => (
            <IssueCard key={i.key} issue={i} compact />
          ))}
        </div>
      )}

      <div className="grid grid-cols-2 gap-x-8 gap-y-2 text-sm md:grid-cols-4">
        <Field label="Status"><StatusPill status={pod.status} /></Field>
        <Field label="Ready">{pod.readyContainers}/{pod.totalContainers}</Field>
        <Field label="Restarts">{pod.restarts}{pod.lastRestartMs && <span className="text-content-muted"> · last {fmtAgo(pod.lastRestartMs)}</span>}</Field>
        <Field label="Age">{fmtAge(pod.createdMs)}</Field>
        <Field label="CPU">{fmtCpu(pod.cpuUsage)} <span className="text-content-muted">req {fmtCpu(pod.cpuRequests)}</span></Field>
        <Field label="Memory">{fmtBytes(pod.memUsage)} <span className="text-content-muted">lim {pod.memLimits ? fmtBytes(pod.memLimits) : "none"}</span></Field>
        <Field label="Pod IP"><span className="mono">{pod.podIp ?? "—"}</span></Field>
        <Field label="QoS">{pod.qosClass ?? "—"}</Field>
      </div>

      <section>
        <h3 className="card-title mb-2">Containers</h3>
        <div className="flex flex-col gap-2">
          {pod.containers.map((c) => (
            <div key={(c.init ? "init-" : "") + c.name} className="card px-3 py-2">
              <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                <span className="font-medium text-content">{c.name}</span>
                {c.init && <span className="tint-muted rounded px-1.5 text-[10px]">init</span>}
                <StatusPill status={c.state === "running" ? (c.ready ? "Running" : "Not ready") : c.reason ?? c.state} />
                <span className="text-xs text-content-muted">{c.restarts} restarts</span>
                <span className="mono ml-auto truncate text-content-muted" title={c.image}>{c.image}</span>
              </div>
              {c.message && <p className="mono mt-1 whitespace-pre-wrap break-words text-content-secondary">{c.message}</p>}
              {c.lastTerminatedReason && (
                <p className="mt-1 text-xs text-content-secondary">
                  Last terminated: <b>{c.lastTerminatedReason}</b>
                  {c.lastTerminatedExitCode != null && ` (exit ${c.lastTerminatedExitCode}${exitHint(c.lastTerminatedExitCode)})`}
                  {c.lastTerminatedMs && ` · ${fmtAgo(c.lastTerminatedMs)}`}
                </p>
              )}
            </div>
          ))}
        </div>
      </section>

      <section className="grid gap-4 lg:grid-cols-2">
        <div className="card p-3">
          <h3 className="card-title mb-2">CPU · 24h</h3>
          <LineChart data={history.map((s) => ({ t: s.tsMs, v: s.cpu }))} color="var(--chart-cpu)" format={fmtCpu} label="CPU" height={140} />
        </div>
        <div className="card p-3">
          <h3 className="card-title mb-2">Memory · 24h</h3>
          <LineChart
            data={history.map((s) => ({ t: s.tsMs, v: s.mem }))}
            color="var(--chart-mem)"
            format={fmtBytes}
            label="Memory"
            height={140}
            yMax={pod.memLimits || undefined}
          />
        </div>
      </section>

      <section>
        <h3 className="card-title mb-2">Conditions</h3>
        <table className="table">
          <tbody>
            {pod.conditions.map((c) => (
              <tr key={c.type}>
                <td className="w-40">{c.type}</td>
                <td className="w-20"><StatusPill status={c.status} tone={c.status === "True" ? "good" : "warning"} /></td>
                <td className="whitespace-normal text-xs text-content-secondary">{c.message ?? c.reason ?? ""}</td>
                <td className="w-24 text-right text-xs text-content-muted">{fmtAgo(c.lastTransitionMs)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
    </div>
  );
}

function exitHint(code: number): string {
  if (code === 137) return " — SIGKILL, often OOM";
  if (code === 143) return " — SIGTERM";
  if (code === 139) return " — segfault";
  if (code === 1) return " — app error";
  return "";
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="text-[11px] text-content-muted">{label}</div>
      <div className="text-content">{children}</div>
    </div>
  );
}

const LEVELS: LogLevel[] = ["error", "warning", "info", "debug"];

function LiveLogs({ namespace, pod, info, previous }: { namespace: string; pod: string; info?: PodInfo; previous: boolean }) {
  const containers = info?.containers.filter((c) => !c.init) ?? [];
  const defaultContainer =
    (previous ? containers.find((c) => c.restarts > 0) : undefined)?.name ?? containers[0]?.name ?? "";
  const [container, setContainer] = useState(defaultContainer);
  // The snapshot may arrive after the drawer opens.
  useEffect(() => {
    if (!container && defaultContainer) setContainer(defaultContainer);
  }, [container, defaultContainer]);
  const [lines, setLines] = useState<LiveLogLine[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set());
  const [search, setSearch] = useState("");

  const load = useCallback(async () => {
    if (!container) return;
    setLoading(true);
    setError(null);
    try {
      setLines(await ipc.liveLogs(namespace, pod, container, previous, 2000));
    } catch (e) {
      setLines([]);
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [namespace, pod, container, previous]);

  useEffect(() => void load(), [load]);

  const rows = useMemo(() => {
    const q = search.toLowerCase();
    return lines
      .filter((l) => (levels.size === 0 || levels.has(l.level)) && (!q || l.message.toLowerCase().includes(q)))
      .map((l, i) => ({ key: i, ...l }));
  }, [lines, levels, search]);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border-light px-4 py-2">
        {containers.length > 1 && (
          <select className="field py-1" value={container} onChange={(e) => setContainer(e.target.value)}>
            {containers.map((c) => (
              <option key={c.name} value={c.name}>
                {c.name}
                {c.restarts ? ` (${c.restarts} restarts)` : ""}
              </option>
            ))}
          </select>
        )}
        <input className="field w-56 py-1" placeholder="Filter lines…" value={search} onChange={(e) => setSearch(e.target.value)} />
        <div className="flex gap-1">
          {LEVELS.map((l) => (
            <button
              key={l}
              className={`btn-chip capitalize ${levels.has(l) ? "!border-accent !text-content" : ""}`}
              onClick={() =>
                setLevels((s) => {
                  const n = new Set(s);
                  if (n.has(l)) n.delete(l);
                  else n.add(l);
                  return n;
                })
              }
            >
              {l}
            </button>
          ))}
        </div>
        <span className="ml-auto text-xs text-content-muted">
          {previous ? "Previous (crashed) instance · " : "Last 2,000 lines · "}
          {rows.length.toLocaleString()} shown
        </span>
        <button className="btn-quiet" onClick={() => void load()} title="Reload">
          {loading ? <Spinner size={14} /> : <RefreshCw size={14} />}
        </button>
      </div>
      <div className="min-h-0 flex-1">
        {error ? (
          <EmptyState title={previous ? "No previous instance logs" : "Couldn't load logs"}>
            <span className="mono">{error}</span>
            {previous && <p className="mt-2">This container hasn't restarted, or the kubelet has already discarded the old instance.</p>}
          </EmptyState>
        ) : (
          <LogView rows={rows} stickToBottom emptyText={loading ? "Loading…" : "No log lines."} />
        )}
      </div>
    </div>
  );
}

export function ObjectEvents({ kind, namespace, name }: { kind: string; namespace: string | null; name: string }) {
  const [events, setEvents] = useState<EventInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    ipc.objectEvents(kind, namespace, name).then(setEvents).catch((e) => setError(errorMessage(e)));
  }, [kind, namespace, name]);
  if (error) return <EmptyState title="Couldn't load events">{error}</EmptyState>;
  if (!events) return <EmptyState icon={<Spinner />} title="Loading events…" />;
  if (!events.length) return <EmptyState title="No events">Kubernetes keeps events for about an hour by default.</EmptyState>;
  return (
    <table className="table">
      <thead>
        <tr>
          <th>Type</th>
          <th>Reason</th>
          <th>Message</th>
          <th className="text-right">Count</th>
          <th className="text-right">Last seen</th>
        </tr>
      </thead>
      <tbody>
        {events.map((e, i) => (
          <tr key={i}>
            <td><StatusPill status={e.type} tone={e.type === "Warning" ? "warning" : "good"} /></td>
            <td className="whitespace-nowrap font-medium">{e.reason}</td>
            <td className="whitespace-normal text-xs text-content-secondary">{e.message}</td>
            <td className="text-right tabular-nums">{e.count}</td>
            <td className="whitespace-nowrap text-right text-xs text-content-muted" title={e.lastMs ? fmtDateTime(e.lastMs) : ""}>
              {fmtAgo(e.lastMs)}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function Manifest({ kind, namespace, name }: { kind: string; namespace: string | null; name: string }) {
  const [yaml, setYaml] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    ipc.getManifest(kind, namespace, name).then(setYaml).catch((e) => setError(errorMessage(e)));
  }, [kind, namespace, name]);
  if (error) return <EmptyState title="Couldn't load manifest">{error}</EmptyState>;
  if (yaml == null) return <EmptyState icon={<Spinner />} title="Loading…" />;
  return <pre className="mono h-full overflow-auto whitespace-pre p-4 text-content-secondary">{yaml}</pre>;
}
