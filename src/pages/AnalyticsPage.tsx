import { useEffect, useMemo, useState } from "react";
import * as ipc from "../lib/ipc";
import { fmtAgo, fmtCount, fmtDuration } from "../lib/format";
import type { ErrorPattern, HistogramBucket, IssueHistoryEntry, PodLogTotals } from "../lib/types";
import { LOG_LEVEL_SERIES, StackedBars } from "../components/charts";
import { EmptyState, PageHeader, SeverityBadge } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";

const RANGES = [
  { label: "6h", ms: 6 * 3600_000, bucket: 15 * 60_000 },
  { label: "24h", ms: 24 * 3600_000, bucket: 3600_000 },
  { label: "7d", ms: 7 * 86_400_000, bucket: 6 * 3600_000 },
];

export default function AnalyticsPage() {
  const [range, setRange] = useState(RANGES[1]);
  const [hist, setHist] = useState<HistogramBucket[]>([]);
  const [topPods, setTopPods] = useState<PodLogTotals[]>([]);
  const [patterns, setPatterns] = useState<ErrorPattern[]>([]);
  const [history, setHistory] = useState<IssueHistoryEntry[]>([]);
  const logTick = useClusterStore((s) => s.logSyncTick);
  const snapshot = useClusterStore((s) => s.snapshot);
  const { openLogs, openPod } = useNavStore();

  useEffect(() => {
    const since = Date.now() - range.ms;
    Promise.all([
      ipc.logHistogram({ sinceMs: since }, range.bucket),
      ipc.topErrorPods(since, 15),
      ipc.errorPatterns(since, 12),
      ipc.issueHistory(since, 2000),
    ])
      .then(([h, t, p, i]) => {
        setHist(h);
        setTopPods(t);
        setPatterns(p);
        setHistory(i);
      })
      .catch(() => undefined);
  }, [range, logTick]);

  // Problem frequency: occurrences and total open time per rule.
  const byRule = useMemo(() => {
    const now = Date.now();
    const m = new Map<string, { title: string; severity: IssueHistoryEntry["severity"]; count: number; openMs: number; open: number }>();
    for (const h of history) {
      const rule = h.key.split(":")[0];
      const e = m.get(rule) ?? { title: h.title.split(":")[0], severity: h.severity, count: 0, openMs: 0, open: 0 };
      e.count++;
      e.openMs += (h.resolvedMs ?? now) - h.firstSeenMs;
      if (h.resolvedMs == null) e.open++;
      m.set(rule, e);
    }
    return [...m.entries()].sort((a, b) => b[1].count - a[1].count);
  }, [history]);

  const restartLeaders = useMemo(
    () => [...(snapshot?.pods ?? [])].filter((p) => p.restarts > 0).sort((a, b) => b.restarts - a.restarts).slice(0, 10),
    [snapshot],
  );
  const maxErrors = Math.max(1, ...topPods.map((p) => p.errors));

  return (
    <div>
      <PageHeader title="Analytics" subtitle="Trends from locally stored logs, metrics and problem history.">
        {RANGES.map((r) => (
          <button key={r.label} className={`btn-chip ${range === r ? "!border-accent !text-content" : ""}`} onClick={() => setRange(r)}>
            {r.label}
          </button>
        ))}
      </PageHeader>
      <div className="grid gap-4 p-6 xl:grid-cols-2">
        <section className="card p-4 xl:col-span-2">
          <h2 className="card-title mb-3">Log volume by level</h2>
          <StackedBars data={hist} series={LOG_LEVEL_SERIES} bucketMs={range.bucket} height={200} />
        </section>

        <section className="card p-4">
          <h2 className="card-title mb-3">Noisiest pods (errors)</h2>
          {topPods.length === 0 ? (
            <EmptyState title="No errors or warnings logged in this range" />
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Pod</th>
                  <th className="w-[40%]">Errors</th>
                  <th className="text-right">Warnings</th>
                </tr>
              </thead>
              <tbody>
                {topPods.map((p) => (
                  <tr key={`${p.namespace}/${p.pod}`} className="cursor-pointer" onClick={() => openLogs({ namespace: p.namespace, pod: p.pod, levels: ["error"] })}>
                    <td>
                      <div className="max-w-[260px] truncate font-medium text-content" title={p.pod}>{p.pod}</div>
                      <div className="text-[11px] text-content-muted">{p.namespace}</div>
                    </td>
                    <td>
                      <div className="flex items-center gap-2">
                        <div className="h-2.5 rounded-r" style={{ width: `${(p.errors / maxErrors) * 100}%`, minWidth: p.errors ? 2 : 0, background: "var(--lvl-error)" }} />
                        <span className="shrink-0 text-xs tabular-nums text-content-secondary">{fmtCount(p.errors)}</span>
                      </div>
                    </td>
                    <td className="text-right tabular-nums text-content-secondary">{fmtCount(p.warnings)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        <section className="card p-4">
          <h2 className="card-title mb-3">Recurring error messages</h2>
          {patterns.length === 0 ? (
            <EmptyState title="No error-level lines in this range" />
          ) : (
            <div className="flex flex-col divide-y divide-border-light">
              {patterns.map((p) => (
                <button
                  key={p.pattern}
                  className="flex items-start gap-3 py-2 text-left hover:bg-muted"
                  onClick={() => openLogs({ levels: ["error"], search: p.sample.split(/\s+/).filter((w) => /^[A-Za-z]{4,}$/.test(w)).slice(0, 3).join(" ") })}
                >
                  <span className="w-14 shrink-0 text-right text-sm font-semibold tabular-nums text-content">{fmtCount(p.count)}×</span>
                  <span className="min-w-0 flex-1">
                    <span className="mono block truncate text-content" title={p.sample}>{p.pattern}</span>
                    <span className="block truncate text-[11px] text-content-muted">
                      {p.pods.join(", ")} · last {fmtAgo(p.lastMs)}
                    </span>
                  </span>
                </button>
              ))}
            </div>
          )}
        </section>

        <section className="card p-4">
          <h2 className="card-title mb-3">Most frequent problems</h2>
          {byRule.length === 0 ? (
            <EmptyState title="No problems recorded in this range" />
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Problem</th>
                  <th className="text-right">Occurrences</th>
                  <th className="text-right">Time open</th>
                  <th className="text-right">Open now</th>
                </tr>
              </thead>
              <tbody>
                {byRule.map(([rule, r]) => (
                  <tr key={rule}>
                    <td>
                      <span className="inline-flex items-center gap-2 whitespace-nowrap">
                        <SeverityBadge severity={r.severity} />
                        {r.title}
                      </span>
                    </td>
                    <td className="text-right tabular-nums">{r.count}</td>
                    <td className="text-right tabular-nums">{fmtDuration(r.openMs)}</td>
                    <td className="text-right tabular-nums">{r.open || "—"}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        <section className="card p-4">
          <h2 className="card-title mb-3">Restart leaders (current pods)</h2>
          {restartLeaders.length === 0 ? (
            <EmptyState title="No pod has restarted" />
          ) : (
            <table className="table">
              <thead>
                <tr>
                  <th>Pod</th>
                  <th>Last reason</th>
                  <th className="text-right">Restarts</th>
                  <th className="text-right">Last</th>
                </tr>
              </thead>
              <tbody>
                {restartLeaders.map((p) => {
                  const c = p.containers.find((c) => c.lastTerminatedReason);
                  return (
                    <tr key={p.uid} className="cursor-pointer" onClick={() => openPod(p.namespace, p.name, "previous")}>
                      <td>
                        <div className="max-w-[240px] truncate font-medium text-content">{p.name}</div>
                        <div className="text-[11px] text-content-muted">{p.namespace}</div>
                      </td>
                      <td className="text-xs text-content-secondary">
                        {c ? `${c.lastTerminatedReason}${c.lastTerminatedExitCode != null ? ` (${c.lastTerminatedExitCode})` : ""}` : "—"}
                      </td>
                      <td className="text-right tabular-nums">{p.restarts}</td>
                      <td className="text-right text-xs text-content-muted">{fmtAgo(p.lastRestartMs)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}
        </section>
      </div>
    </div>
  );
}
