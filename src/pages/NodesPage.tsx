import { Ban, CircleCheck } from "lucide-react";
import { runAction } from "../lib/actions";
import { fmtAge, fmtBytes, fmtCpu, fmtPct, pct } from "../lib/format";
import SnapshotGate from "../components/SnapshotGate";
import { Meter, PageHeader, StatusPill } from "../components/ui";
import { useNavStore } from "../stores/nav";

export default function NodesPage() {
  const openNode = useNavStore((s) => s.openNode);
  return (
    <SnapshotGate>
      {(s) => (
        <div>
          <PageHeader title="Nodes" subtitle={`${s.totals.nodesReady}/${s.totals.nodes} ready`} />
          <div className="grid gap-4 p-6 lg:grid-cols-2">
            {s.nodes.map((n) => {
              const bad = n.conditions.filter((c) => (c.type === "Ready" ? c.status !== "True" : c.status === "True"));
              return (
                <div key={n.name} className="card flex flex-col gap-3 p-4">
                  <div className="flex items-start gap-2">
                    <button className="text-left" onClick={() => openNode(n.name)}>
                      <div className="font-semibold text-content hover:text-accent">{n.name}</div>
                      <div className="text-xs text-content-muted">
                        {n.roles.join(", ") || "worker"} · {n.kubeletVersion} · {n.internalIp} · up {fmtAge(n.createdMs)}
                      </div>
                    </button>
                    <div className="ml-auto flex items-center gap-2">
                      <StatusPill status={n.ready ? (n.unschedulable ? "Cordoned" : "Ready") : "NotReady"} tone={n.ready ? (n.unschedulable ? "warning" : "good") : "critical"} />
                      {n.unschedulable ? (
                        <button className="btn-chip" onClick={() => void runAction("uncordon", { kind: "Node", namespace: null, name: n.name })}>
                          <CircleCheck size={12} /> Uncordon
                        </button>
                      ) : (
                        <button className="btn-chip" onClick={() => void runAction("cordon", { kind: "Node", namespace: null, name: n.name })}>
                          <Ban size={12} /> Cordon
                        </button>
                      )}
                    </div>
                  </div>
                  <div className="grid grid-cols-3 gap-4">
                    <Meter value={pct(n.cpuUsage, n.cpuAllocatable)} label={<><span>CPU</span><span>{fmtPct(pct(n.cpuUsage, n.cpuAllocatable))}</span></>} />
                    <Meter value={pct(n.memUsage, n.memAllocatable)} label={<><span>Memory</span><span>{fmtPct(pct(n.memUsage, n.memAllocatable))}</span></>} />
                    <Meter value={pct(n.podCount, n.podCapacity)} label={<><span>Pods</span><span>{n.podCount}/{n.podCapacity}</span></>} />
                  </div>
                  <div className="text-xs text-content-secondary">
                    Usage {fmtCpu(n.cpuUsage)} / {fmtCpu(n.cpuAllocatable)} cores · {fmtBytes(n.memUsage)} / {fmtBytes(n.memAllocatable)}
                    <br />
                    Requested {fmtCpu(n.cpuRequests)} cores ({fmtPct(pct(n.cpuRequests, n.cpuAllocatable))}) · {fmtBytes(n.memRequests)} ({fmtPct(pct(n.memRequests, n.memAllocatable))})
                  </div>
                  {bad.length > 0 && (
                    <div className="flex flex-wrap gap-1.5">
                      {bad.map((c) => (
                        <span key={c.type} className="tint-critical rounded px-1.5 py-0.5 text-[11px] font-medium" title={c.message ?? ""}>
                          {c.type}
                        </span>
                      ))}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </SnapshotGate>
  );
}
