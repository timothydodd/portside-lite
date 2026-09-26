import { useEffect, useState } from "react";
import { CheckCircle2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { fmtAgo, fmtBytes, fmtCpu, fmtPct, pct } from "../lib/format";
import type { Sample } from "../lib/types";
import { LineChart } from "../components/charts";
import IssueCard from "../components/IssueCard";
import SnapshotGate from "../components/SnapshotGate";
import { Meter, PageHeader, StatTile, StatusPill } from "../components/ui";
import { useNavStore } from "../stores/nav";

export default function OverviewPage() {
  const { go, openNode, openPod } = useNavStore();
  const [history, setHistory] = useState<Sample[]>([]);

  useEffect(() => {
    const load = () =>
      ipc.clusterHistory(Date.now() - 24 * 3600_000, 5 * 60_000).then(setHistory).catch(() => setHistory([]));
    void load();
    const t = setInterval(load, 60_000);
    return () => clearInterval(t);
  }, []);

  return (
    <SnapshotGate>
      {(s) => {
        const critical = s.issues.filter((i) => i.severity === "critical");
        const warning = s.issues.filter((i) => i.severity === "warning");
        const cpuPct = pct(s.totals.cpuUsage, s.totals.cpuAllocatable);
        const memPct = pct(s.totals.memUsage, s.totals.memAllocatable);
        const top = [...critical, ...warning].slice(0, 6);
        return (
          <div>
            <PageHeader title="Cluster overview" subtitle={`${s.clusterId} · updated ${fmtAgo(s.collectedAtMs)}`} />
            <div className="flex flex-col gap-5 p-6">
              <div className="grid grid-cols-2 gap-3 lg:grid-cols-5">
                <StatTile
                  label="Problems"
                  value={critical.length + warning.length}
                  sub={`${critical.length} critical · ${warning.length} warning`}
                  tone={critical.length ? "critical" : warning.length ? "warning" : "good"}
                  onClick={() => go("problems")}
                />
                <StatTile
                  label="Nodes ready"
                  value={`${s.totals.nodesReady}/${s.totals.nodes}`}
                  tone={s.totals.nodesReady < s.totals.nodes ? "critical" : undefined}
                  onClick={() => go("nodes")}
                />
                <StatTile
                  label="Pods running"
                  value={`${s.totals.podsRunning}/${s.totals.pods}`}
                  sub={`${s.totals.podsPending} pending · ${s.totals.podsFailed} failed`}
                  onClick={() => go("pods")}
                />
                <StatTile
                  label="CPU"
                  value={fmtPct(cpuPct)}
                  sub={`${fmtCpu(s.totals.cpuUsage)} of ${fmtCpu(s.totals.cpuAllocatable)} cores`}
                />
                <StatTile
                  label="Memory"
                  value={fmtPct(memPct)}
                  sub={`${fmtBytes(s.totals.memUsage)} of ${fmtBytes(s.totals.memAllocatable)}`}
                />
              </div>

              <div className="grid gap-4 xl:grid-cols-[3fr_2fr]">
                <section className="flex flex-col gap-2">
                  <div className="flex items-center justify-between">
                    <h2 className="card-title">Top problems</h2>
                    {s.issues.length > top.length && (
                      <button className="text-xs text-accent hover:underline" onClick={() => go("problems")}>
                        View all {s.issues.length}
                      </button>
                    )}
                  </div>
                  {top.length === 0 ? (
                    <div className="card flex items-center gap-3 px-4 py-6 text-sm text-content-secondary">
                      <CheckCircle2 size={20} className="text-good" />
                      No critical or warning problems detected.
                      {s.issues.length > 0 && ` ${s.issues.length} informational.`}
                    </div>
                  ) : (
                    top.map((i) => <IssueCard key={i.key} issue={i} compact />)
                  )}
                </section>

                <section className="flex flex-col gap-3">
                  <div className="card p-3">
                    <h2 className="card-title mb-2">Cluster CPU · 24h</h2>
                    <LineChart
                      data={history.map((h) => ({ t: h.tsMs, v: h.cpu }))}
                      color="var(--chart-cpu)"
                      format={fmtCpu}
                      yMax={s.totals.cpuAllocatable || undefined}
                      label="CPU"
                      height={130}
                    />
                  </div>
                  <div className="card p-3">
                    <h2 className="card-title mb-2">Cluster memory · 24h</h2>
                    <LineChart
                      data={history.map((h) => ({ t: h.tsMs, v: h.mem }))}
                      color="var(--chart-mem)"
                      format={fmtBytes}
                      yMax={s.totals.memAllocatable || undefined}
                      label="Memory"
                      height={130}
                    />
                  </div>
                </section>
              </div>

              <section>
                <h2 className="card-title mb-2">Nodes</h2>
                <div className="grid gap-3 md:grid-cols-2 xl:grid-cols-3">
                  {s.nodes.map((n) => (
                    <button key={n.name} className="card px-4 py-3 text-left transition-colors hover:border-accent" onClick={() => openNode(n.name)}>
                      <div className="mb-2 flex items-center gap-2">
                        <span className="font-medium text-content">{n.name}</span>
                        <StatusPill status={n.ready ? (n.unschedulable ? "Cordoned" : "Ready") : "NotReady"} tone={n.ready ? (n.unschedulable ? "warning" : "good") : "critical"} />
                        <span className="ml-auto text-xs text-content-muted">{n.podCount} pods</span>
                      </div>
                      <div className="flex flex-col gap-2">
                        <Meter value={pct(n.cpuUsage, n.cpuAllocatable)} label={<><span>CPU</span><span>{fmtPct(pct(n.cpuUsage, n.cpuAllocatable))}</span></>} />
                        <Meter value={pct(n.memUsage, n.memAllocatable)} label={<><span>Memory</span><span>{fmtPct(pct(n.memUsage, n.memAllocatable))}</span></>} />
                      </div>
                    </button>
                  ))}
                </div>
              </section>

              {s.events.length > 0 && (
                <section>
                  <div className="mb-2 flex items-center justify-between">
                    <h2 className="card-title">Recent warning events</h2>
                    <button className="text-xs text-accent hover:underline" onClick={() => go("events")}>
                      All events
                    </button>
                  </div>
                  <div className="card divide-y divide-border-light">
                    {s.events.slice(0, 6).map((e, i) => (
                      // Grid with minmax(0, …) tracks: every cell truncates instead of
                      // spilling into its neighbour, however long the text.
                      <div
                        key={i}
                        className="grid grid-cols-[minmax(0,11rem)_minmax(0,15rem)_minmax(0,1fr)_auto] items-baseline gap-3 px-4 py-2 text-sm"
                      >
                        <span className="truncate font-medium text-content" title={e.reason}>{e.reason}</span>
                        <button
                          className="truncate text-left text-xs text-content-secondary hover:text-accent"
                          title={`${e.objectKind}/${e.objectName}`}
                          onClick={() => e.objectKind === "Pod" ? openPod(e.namespace, e.objectName) : e.objectKind === "Node" ? openNode(e.objectName) : undefined}
                        >
                          {e.objectKind}/{e.objectName}
                        </button>
                        <span className="truncate text-xs text-content-muted" title={e.message}>{e.message}</span>
                        <span className="whitespace-nowrap text-xs text-content-muted">{fmtAgo(e.lastMs)}</span>
                      </div>
                    ))}
                  </div>
                </section>
              )}
            </div>
          </div>
        );
      }}
    </SnapshotGate>
  );
}
