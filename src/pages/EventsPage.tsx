import { useMemo, useState } from "react";
import { fmtAgo, fmtDateTime } from "../lib/format";
import SnapshotGate from "../components/SnapshotGate";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput } from "../components/ui";
import { useNavStore } from "../stores/nav";
import type { EventInfo } from "../lib/types";

export default function EventsPage() {
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [reason, setReason] = useState("");
  return (
    <SnapshotGate>
      {(s) => (
        <EventsTable
          events={s.events}
          namespaces={s.namespaces}
          {...{ namespace, setNamespace, search, setSearch, reason, setReason }}
        />
      )}
    </SnapshotGate>
  );
}

function EventsTable(p: {
  events: EventInfo[];
  namespaces: string[];
  namespace: string;
  setNamespace: (v: string) => void;
  search: string;
  setSearch: (v: string) => void;
  reason: string;
  setReason: (v: string) => void;
}) {
  const { openPod, openNode } = useNavStore();
  const reasons = useMemo(() => {
    const m = new Map<string, number>();
    p.events.forEach((e) => m.set(e.reason, (m.get(e.reason) ?? 0) + e.count));
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [p.events]);
  const q = p.search.toLowerCase();
  const rows = p.events.filter(
    (e) =>
      (!p.namespace || e.namespace === p.namespace) &&
      (!p.reason || e.reason === p.reason) &&
      (!q || `${e.objectName} ${e.message}`.toLowerCase().includes(q)),
  );

  return (
    <div className="flex h-full flex-col">
      <PageHeader title="Warning events" subtitle="Kubernetes keeps events for about an hour; Portside shows what the API server still has.">
        <select className="field" value={p.reason} onChange={(e) => p.setReason(e.target.value)}>
          <option value="">All reasons</option>
          {reasons.map(([r, n]) => (
            <option key={r} value={r}>
              {r} ({n})
            </option>
          ))}
        </select>
        <NamespaceSelect namespaces={p.namespaces} value={p.namespace} onChange={p.setNamespace} />
        <SearchInput value={p.search} onChange={p.setSearch} className="w-56" />
      </PageHeader>
      {rows.length === 0 ? (
        <EmptyState title="No warning events" />
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <table className="table">
            <thead>
              <tr>
                <th>Reason</th>
                <th>Object</th>
                <th>Message</th>
                <th className="text-right">Count</th>
                <th className="text-right">Last seen</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((e, i) => {
                const open = e.objectKind === "Pod" ? () => openPod(e.namespace, e.objectName) : e.objectKind === "Node" ? () => openNode(e.objectName) : undefined;
                return (
                  <tr key={i}>
                    <td className="whitespace-nowrap font-medium">{e.reason}</td>
                    <td>
                      <button className={`block max-w-[260px] truncate text-left ${open ? "hover:text-accent" : "cursor-default"}`} onClick={open}>
                        <span className="text-content-muted">{e.objectKind}/</span>
                        {e.objectName}
                      </button>
                      <div className="text-[11px] text-content-muted">{e.namespace}</div>
                    </td>
                    <td className="whitespace-normal text-xs text-content-secondary">{e.message}</td>
                    <td className="text-right tabular-nums">{e.count}</td>
                    <td className="whitespace-nowrap text-right text-xs text-content-muted" title={e.lastMs ? fmtDateTime(e.lastMs) : ""}>
                      {fmtAgo(e.lastMs)}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
