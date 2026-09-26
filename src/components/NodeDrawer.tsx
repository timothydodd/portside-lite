import { useEffect, useState } from "react";
import { Ban, CircleCheck } from "lucide-react";
import * as ipc from "../lib/ipc";
import { runAction } from "../lib/actions";
import { fmtAge, fmtAgo, fmtBytes, fmtCpu, pct } from "../lib/format";
import type { Sample } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { LineChart } from "./charts";
import IssueCard from "./IssueCard";
import { ObjectEvents } from "./PodDrawer";
import { Drawer, EmptyState, Meter, StatusPill } from "./ui";

export default function NodeDrawer() {
  const { node: name, closeNode, openPod } = useNavStore();
  const snapshot = useClusterStore((s) => s.snapshot);
  const node = snapshot?.nodes.find((n) => n.name === name);
  const [history, setHistory] = useState<Sample[]>([]);
  const [tab, setTab] = useState<"overview" | "events">("overview");

  useEffect(() => {
    if (!name) return;
    ipc.nodeHistory(name, Date.now() - 24 * 3600_000, 5 * 60_000).then(setHistory).catch(() => setHistory([]));
  }, [name]);

  if (!name) return null;
  const pods = snapshot?.pods.filter((p) => p.node === name) ?? [];
  const issues = snapshot?.issues.filter((i) => i.kind === "Node" && i.name === name) ?? [];

  return (
    <Drawer
      title={name}
      subtitle={node ? `${node.roles.join(", ") || "worker"} · ${node.kubeletVersion} · ${node.internalIp ?? ""}` : undefined}
      onClose={closeNode}
      actions={
        node &&
        (node.unschedulable ? (
          <button className="btn-ghost" onClick={() => void runAction("uncordon", { kind: "Node", namespace: null, name })}>
            <CircleCheck size={14} /> Uncordon
          </button>
        ) : (
          <button className="btn-ghost" onClick={() => void runAction("cordon", { kind: "Node", namespace: null, name })}>
            <Ban size={14} /> Cordon
          </button>
        ))
      }
    >
      <div className="flex shrink-0 gap-1 border-b border-border-light px-4">
        {(["overview", "events"] as const).map((t) => (
          <button key={t} className={`navtab capitalize ${tab === t ? "navtab-active" : ""}`} onClick={() => setTab(t)}>
            {t}
          </button>
        ))}
      </div>
      {tab === "events" ? (
        <ObjectEvents kind="Node" namespace={null} name={name} />
      ) : !node ? (
        <EmptyState title="Node not found" />
      ) : (
        <div className="flex flex-col gap-5 p-5">
          {issues.map((i) => (
            <IssueCard key={i.key} issue={i} compact />
          ))}
          <div className="grid gap-4 md:grid-cols-3">
            <Meter
              value={pct(node.cpuUsage, node.cpuAllocatable)}
              label={<><span>CPU</span><span>{fmtCpu(node.cpuUsage)} / {fmtCpu(node.cpuAllocatable)}</span></>}
            />
            <Meter
              value={pct(node.memUsage, node.memAllocatable)}
              label={<><span>Memory</span><span>{fmtBytes(node.memUsage)} / {fmtBytes(node.memAllocatable)}</span></>}
            />
            <Meter
              value={pct(node.podCount, node.podCapacity)}
              label={<><span>Pods</span><span>{node.podCount} / {node.podCapacity}</span></>}
            />
          </div>
          <div className="text-xs text-content-secondary">
            Requests: CPU {fmtCpu(node.cpuRequests)} ({Math.round(pct(node.cpuRequests, node.cpuAllocatable) ?? 0)}%) · Memory{" "}
            {fmtBytes(node.memRequests)} ({Math.round(pct(node.memRequests, node.memAllocatable) ?? 0)}%) · {node.osImage} · kernel{" "}
            {node.kernelVersion} · up {fmtAge(node.createdMs)}
          </div>
          <div className="grid gap-4 lg:grid-cols-2">
            <div className="card p-3">
              <h3 className="card-title mb-2">CPU · 24h</h3>
              <LineChart data={history.map((s) => ({ t: s.tsMs, v: s.cpu }))} color="var(--chart-cpu)" format={fmtCpu} yMax={node.cpuAllocatable} label="CPU" height={140} />
            </div>
            <div className="card p-3">
              <h3 className="card-title mb-2">Memory · 24h</h3>
              <LineChart data={history.map((s) => ({ t: s.tsMs, v: s.mem }))} color="var(--chart-mem)" format={fmtBytes} yMax={node.memAllocatable} label="Memory" height={140} />
            </div>
          </div>
          <section>
            <h3 className="card-title mb-2">Conditions</h3>
            <table className="table">
              <tbody>
                {node.conditions.map((c) => {
                  const healthy = c.type === "Ready" ? c.status === "True" : c.status === "False";
                  return (
                    <tr key={c.type}>
                      <td className="w-44">{c.type}</td>
                      <td className="w-24"><StatusPill status={c.status} tone={healthy ? "good" : "critical"} /></td>
                      <td className="whitespace-normal text-xs text-content-secondary">{c.message}</td>
                      <td className="w-24 text-right text-xs text-content-muted">{fmtAgo(c.lastTransitionMs)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </section>
          <section>
            <h3 className="card-title mb-2">Pods on this node ({pods.length})</h3>
            <table className="table">
              <thead>
                <tr>
                  <th>Pod</th>
                  <th>Status</th>
                  <th className="text-right">CPU</th>
                  <th className="text-right">Memory</th>
                </tr>
              </thead>
              <tbody>
                {[...pods]
                  .sort((a, b) => (b.memUsage ?? 0) - (a.memUsage ?? 0))
                  .map((p) => (
                    <tr key={p.uid} className="cursor-pointer" onClick={() => openPod(p.namespace, p.name)}>
                      <td>
                        <div className="font-medium">{p.name}</div>
                        <div className="text-xs text-content-muted">{p.namespace}</div>
                      </td>
                      <td><StatusPill status={p.status} /></td>
                      <td className="text-right tabular-nums">{fmtCpu(p.cpuUsage)}</td>
                      <td className="text-right tabular-nums">{fmtBytes(p.memUsage)}</td>
                    </tr>
                  ))}
              </tbody>
            </table>
          </section>
        </div>
      )}
    </Drawer>
  );
}
