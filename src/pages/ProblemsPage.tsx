import { useEffect, useMemo, useState } from "react";
import { CheckCircle2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { fmtAgo, fmtDateTime, fmtDuration } from "../lib/format";
import type { IssueHistoryEntry, Severity } from "../lib/types";
import IssueCard from "../components/IssueCard";
import SnapshotGate from "../components/SnapshotGate";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput, SeverityBadge } from "../components/ui";
import { useClusterStore } from "../stores/cluster";

const CATEGORIES = ["pod", "node", "workload", "storage", "event", "logs"] as const;

export default function ProblemsPage() {
  const [view, setView] = useState<"live" | "history">("live");
  const [severity, setSeverity] = useState<Severity | "">("");
  const [category, setCategory] = useState("");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");

  return (
    <div>
      <PageHeader title="Problems" subtitle="Everything Portside thinks needs attention, worst first.">
        <div className="flex rounded-md border border-border p-0.5">
          {(["live", "history"] as const).map((v) => (
            <button
              key={v}
              className={`rounded px-3 py-1 text-xs capitalize ${view === v ? "bg-accent text-on-accent" : "text-content-secondary hover:text-content"}`}
              onClick={() => setView(v)}
            >
              {v === "live" ? "Current" : "History"}
            </button>
          ))}
        </div>
      </PageHeader>
      {view === "history" ? (
        <IssueHistory />
      ) : (
        <SnapshotGate>
          {(s) => {
            const q = search.toLowerCase();
            const list = s.issues.filter(
              (i) =>
                (!severity || i.severity === severity) &&
                (!category || i.category === category) &&
                (!namespace || i.namespace === namespace) &&
                (!q || `${i.title} ${i.detail} ${i.name}`.toLowerCase().includes(q)),
            );
            const counts = { critical: 0, warning: 0, info: 0 } as Record<Severity, number>;
            s.issues.forEach((i) => counts[i.severity]++);
            return (
              <div className="flex flex-col gap-4 p-6">
                <div className="flex flex-wrap items-center gap-2">
                  {(["critical", "warning", "info"] as Severity[]).map((sv) => (
                    <button
                      key={sv}
                      className={`rounded-md border px-2 py-1 ${severity === sv ? "border-accent" : "border-transparent"}`}
                      onClick={() => setSeverity(severity === sv ? "" : sv)}
                    >
                      <span className="inline-flex items-center gap-1.5 text-xs text-content-secondary">
                        <SeverityBadge severity={sv} /> {counts[sv]}
                      </span>
                    </button>
                  ))}
                  <select className="field" value={category} onChange={(e) => setCategory(e.target.value)}>
                    <option value="">All categories</option>
                    {CATEGORIES.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                  <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />
                  <SearchInput value={search} onChange={setSearch} className="w-64" />
                </div>
                {list.length === 0 ? (
                  <EmptyState icon={<CheckCircle2 size={32} className="text-good" />} title={s.issues.length ? "Nothing matches these filters" : "No problems detected"}>
                    {!s.issues.length && "Portside checks pods, nodes, workloads, volumes, warning events and error-log volume every poll."}
                  </EmptyState>
                ) : (
                  <div className="flex flex-col gap-2">
                    {list.map((i) => (
                      <IssueCard key={i.key} issue={i} />
                    ))}
                  </div>
                )}
              </div>
            );
          }}
        </SnapshotGate>
      )}
    </div>
  );
}

function IssueHistory() {
  const [range, setRange] = useState(7);
  const [rows, setRows] = useState<IssueHistoryEntry[]>([]);
  const [showOpen, setShowOpen] = useState(false);
  const tick = useClusterStore((s) => s.snapshot?.collectedAtMs);

  useEffect(() => {
    ipc.issueHistory(Date.now() - range * 86_400_000, 1000).then(setRows).catch(() => setRows([]));
  }, [range, tick]);

  const shown = useMemo(() => (showOpen ? rows.filter((r) => r.resolvedMs == null) : rows), [rows, showOpen]);
  const now = Date.now();

  return (
    <div className="flex flex-col gap-3 p-6">
      <div className="flex items-center gap-3">
        <select className="field" value={range} onChange={(e) => setRange(Number(e.target.value))}>
          <option value={1}>Last 24 hours</option>
          <option value={7}>Last 7 days</option>
          <option value={30}>Last 30 days</option>
        </select>
        <label className="flex items-center gap-1.5 text-xs text-content-secondary">
          <input type="checkbox" className="accent-brand" checked={showOpen} onChange={(e) => setShowOpen(e.target.checked)} />
          Only still open
        </label>
        <span className="ml-auto text-xs text-content-muted">
          {rows.length} problems · {rows.filter((r) => r.resolvedMs != null).length} resolved
        </span>
      </div>
      {shown.length === 0 ? (
        <EmptyState title="No problem history in this range" />
      ) : (
        <div className="card overflow-hidden">
          <table className="table">
            <thead>
              <tr>
                <th>Severity</th>
                <th>Problem</th>
                <th>Started</th>
                <th>Duration</th>
                <th>State</th>
              </tr>
            </thead>
            <tbody>
              {shown.map((r) => (
                <tr key={r.id}>
                  <td className="w-28"><SeverityBadge severity={r.severity} /></td>
                  <td>
                    <div className="font-medium text-content">{r.title}</div>
                    <div className="max-w-xl truncate text-xs text-content-muted" title={r.detail}>{r.detail}</div>
                  </td>
                  <td className="whitespace-nowrap text-xs text-content-secondary" title={fmtDateTime(r.firstSeenMs)}>{fmtAgo(r.firstSeenMs)}</td>
                  <td className="whitespace-nowrap text-xs tabular-nums">{fmtDuration((r.resolvedMs ?? now) - r.firstSeenMs)}</td>
                  <td className="whitespace-nowrap text-xs">
                    {r.resolvedMs ? (
                      <span className="text-content-muted">Resolved {fmtAgo(r.resolvedMs)}</span>
                    ) : (
                      <span className="font-medium text-content">Open</span>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
