import { useEffect, useMemo, useState } from "react";
import { RotateCcw, ScrollText } from "lucide-react";
import * as ipc from "../lib/ipc";
import { runAction } from "../lib/actions";
import { fmtAge, fmtBytes, fmtCpu } from "../lib/format";
import type { PodInfo, PodLogTotals } from "../lib/types";
import SnapshotGate from "../components/SnapshotGate";
import { NamespaceSelect, PageHeader, SearchInput, SortTh, StatusPill, statusTone } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";

type SortKey = "name" | "namespace" | "status" | "restarts" | "cpu" | "mem" | "errors" | "age";
type StatusFilter = "" | "problem" | "running" | "pending" | "completed";

export default function PodsPage() {
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("");
  const [sort, setSort] = useState<{ key: SortKey; dir: 1 | -1 }>({ key: "errors", dir: -1 });
  const [counts, setCounts] = useState<Map<string, PodLogTotals>>(new Map());
  const logTick = useClusterStore((s) => s.logSyncTick);
  const { openPod } = useNavStore();

  useEffect(() => {
    ipc
      .podLogCounts(Date.now() - 24 * 3600_000)
      .then((rows) => setCounts(new Map(rows.map((r) => [`${r.namespace}/${r.pod}`, r]))))
      .catch(() => setCounts(new Map()));
  }, [logTick]);

  return (
    <SnapshotGate>
      {(s) => (
        <PodsTable
          pods={s.pods}
          namespaces={s.namespaces}
          problemKeys={new Set(s.issues.filter((i) => i.kind === "Pod").map((i) => `${i.namespace}/${i.name}`))}
          {...{ namespace, setNamespace, search, setSearch, statusFilter, setStatusFilter, sort, setSort, counts, openPod }}
        />
      )}
    </SnapshotGate>
  );
}

function PodsTable(props: {
  pods: PodInfo[];
  namespaces: string[];
  problemKeys: Set<string>;
  namespace: string;
  setNamespace: (v: string) => void;
  search: string;
  setSearch: (v: string) => void;
  statusFilter: StatusFilter;
  setStatusFilter: (v: StatusFilter) => void;
  sort: { key: SortKey; dir: 1 | -1 };
  setSort: (s: { key: SortKey; dir: 1 | -1 }) => void;
  counts: Map<string, PodLogTotals>;
  openPod: (ns: string, name: string) => void;
}) {
  const { pods, namespaces, problemKeys, namespace, search, statusFilter, sort, counts, openPod } = props;
  const now = Date.now();

  const rows = useMemo(() => {
    const q = search.toLowerCase();
    const filtered = pods.filter((p) => {
      if (namespace && p.namespace !== namespace) return false;
      if (q && !`${p.name} ${p.node ?? ""} ${p.ownerName ?? ""}`.toLowerCase().includes(q)) return false;
      const key = `${p.namespace}/${p.name}`;
      switch (statusFilter) {
        case "problem":
          return problemKeys.has(key) || statusTone(p.status) === "critical";
        case "running":
          return p.phase === "Running";
        case "pending":
          return p.phase === "Pending";
        case "completed":
          return p.phase === "Succeeded";
      }
      return true;
    });
    const val = (p: PodInfo): number | string => {
      const c = counts.get(`${p.namespace}/${p.name}`);
      switch (sort.key) {
        case "name":
          return p.name;
        case "namespace":
          return p.namespace;
        case "status":
          return p.status;
        case "restarts":
          return p.restarts;
        case "cpu":
          return p.cpuUsage ?? -1;
        case "mem":
          return p.memUsage ?? -1;
        case "errors":
          return (c?.errors ?? 0) * 1e6 + (c?.warnings ?? 0) + (problemKeys.has(`${p.namespace}/${p.name}`) ? 1e12 : 0);
        case "age":
          return -(p.createdMs ?? 0);
      }
    };
    return filtered.sort((a, b) => {
      const va = val(a);
      const vb = val(b);
      return (va < vb ? -1 : va > vb ? 1 : 0) * sort.dir;
    });
  }, [pods, namespace, search, statusFilter, sort, counts, problemKeys]);

  return (
    <div className="flex h-full flex-col">
      <PageHeader title="Pods" subtitle={`${rows.length} of ${pods.length} pods`}>
        <select className="field" value={statusFilter} onChange={(e) => props.setStatusFilter(e.target.value as StatusFilter)}>
          <option value="">Any status</option>
          <option value="problem">Has problems</option>
          <option value="running">Running</option>
          <option value="pending">Pending</option>
          <option value="completed">Completed</option>
        </select>
        <NamespaceSelect namespaces={namespaces} value={namespace} onChange={props.setNamespace} />
        <SearchInput value={search} onChange={props.setSearch} placeholder="Name, node, owner…" className="w-60" />
      </PageHeader>
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="table">
          <thead>
            <tr>
              <SortTh label="Name" k="name" {...props} />
              <SortTh label="Namespace" k="namespace" {...props} />
              <SortTh label="Status" k="status" {...props} />
              <th>Ready</th>
              <SortTh label="Restarts" k="restarts" {...props} className="text-right" />
              <SortTh label="CPU" k="cpu" {...props} className="text-right" />
              <SortTh label="Memory" k="mem" {...props} className="text-right" />
              <SortTh label="Err / warn 24h" k="errors" {...props} className="text-right" />
              <th>Node</th>
              <SortTh label="Age" k="age" {...props} className="text-right" />
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((p) => {
              const c = counts.get(`${p.namespace}/${p.name}`);
              const problem = problemKeys.has(`${p.namespace}/${p.name}`);
              return (
                <tr key={p.uid} className="cursor-pointer" onClick={() => openPod(p.namespace, p.name)}>
                  <td>
                    <div className="flex max-w-[280px] items-center gap-2">
                      {problem && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-critical" title="Has problems" />}
                      <span className="truncate font-medium text-content" title={p.name}>{p.name}</span>
                    </div>
                  </td>
                  <td className="text-content-secondary">{p.namespace}</td>
                  <td><StatusPill status={p.status} /></td>
                  <td className="tabular-nums">{p.readyContainers}/{p.totalContainers}</td>
                  <td className={`text-right tabular-nums ${p.restarts > 0 ? "text-content" : "text-content-muted"}`}>{p.restarts}</td>
                  <td className="text-right tabular-nums">{fmtCpu(p.cpuUsage)}</td>
                  <td className="text-right tabular-nums">{fmtBytes(p.memUsage)}</td>
                  <td className="text-right tabular-nums">
                    {c ? (
                      <span>
                        <span className={c.errors ? "font-semibold text-critical" : "text-content-muted"}>{c.errors}</span>
                        <span className="text-content-muted"> / </span>
                        <span className={c.warnings ? "text-warning" : "text-content-muted"}>{c.warnings}</span>
                      </span>
                    ) : (
                      <span className="text-content-muted">—</span>
                    )}
                  </td>
                  <td className="text-xs text-content-secondary">
                    <div className="max-w-[120px] truncate">{p.node ?? "—"}</div>
                  </td>
                  <td className="text-right text-xs text-content-muted">{fmtAge(p.createdMs, now)}</td>
                  <td className="whitespace-nowrap text-right" onClick={(e) => e.stopPropagation()}>
                    <button className="btn-quiet" title="Logs" onClick={() => void runAction("viewLogs", { kind: "Pod", namespace: p.namespace, name: p.name })}>
                      <ScrollText size={14} />
                    </button>
                    <button className="btn-quiet" title="Restart (delete) pod" onClick={() => void runAction("deletePod", { kind: "Pod", namespace: p.namespace, name: p.name })}>
                      <RotateCcw size={14} />
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
    </div>
  );
}
