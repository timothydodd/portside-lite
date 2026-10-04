import { useMemo, useState } from "react";
import { fmtCount } from "../lib/format";
import { bucketFor, FILTER_LEVELS, LOG_PAGE, LOG_RANGES, useDebounced, useLogResults } from "../lib/logs";
import type { LogLevel, LogQuery, LogSource, WorkloadRef } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { LOG_LEVEL_SERIES, StackedBars } from "./charts";
import LogView from "./LogView";
import { EmptyState, Spinner } from "./ui";


/**
 * A workload's lines from the local store: every pod it has had, including
 * pods that are gone because it was scaled down, redeployed or deleted.
 */
export default function StoredLogs({
  namespace,
  workload,
  sources,
}: {
  namespace: string;
  workload: WorkloadRef;
  /** Its pods with stored lines (for the pod picker and the "All" span). */
  sources: LogSource[] | null;
}) {
  const logTick = useClusterStore((s) => s.logSyncTick);
  const [search, setSearch] = useState("");
  const [pod, setPod] = useState("");
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set());
  const [rangeMs, setRangeMs] = useState<number | null>(null);

  const debouncedSearch = useDebounced(search, 300);
  const now = useMemo(() => Date.now(), [logTick, rangeMs]); // eslint-disable-line react-hooks/exhaustive-deps
  const oldest = useMemo(() => (sources?.length ? Math.min(...sources.map((s) => s.firstMs)) : now - 86_400_000), [sources, now]);
  const since = rangeMs == null ? null : now - rangeMs;
  const bucketMs = bucketFor(now - (since ?? oldest));

  const query: LogQuery = useMemo(
    () => ({
      search: debouncedSearch || null,
      namespace,
      workload,
      pod: pod || null,
      levels: [...levels],
      sinceMs: since,
      limit: LOG_PAGE,
    }),
    [debouncedSearch, namespace, workload, pod, levels, since],
  );

  // What was asked for, as opposed to `query`, whose time window moves with every sync.
  const resetKey = JSON.stringify([debouncedSearch, namespace, workload, pod, [...levels], rangeMs]);
  const { rows, hist, loading, error, loadMore } = useLogResults(query, bucketMs, resetKey, logTick);

  const logRows = useMemo(
    () =>
      rows.map((r) => ({
        key: r.id,
        tsMs: r.tsMs,
        level: r.level,
        message: r.message,
        source: `${r.pod}${r.container && r.container !== r.pod ? ` [${r.container}]` : ""}`,
      })),
    [rows],
  );
  const total = hist.reduce((s, b) => s + b.trace + b.debug + b.info + b.warning + b.error, 0);

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-wrap items-center gap-2 border-b border-border-light px-4 py-2">
        <input className="field w-56 py-1" placeholder="Search messages…" value={search} onChange={(e) => setSearch(e.target.value)} />
        <select className="field max-w-[220px] py-1" value={pod} onChange={(e) => setPod(e.target.value)}>
          <option value="">All pods ({sources?.length ?? 0})</option>
          {(sources ?? []).map((s) => (
            <option key={s.pod} value={s.pod}>
              {s.pod}
            </option>
          ))}
        </select>
        <div className="flex gap-1">
          {FILTER_LEVELS.map((l) => (
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
        <div className="ml-auto flex items-center gap-1">
          {LOG_RANGES.map((r) => (
            <button key={r.label} className={`btn-chip ${rangeMs === r.ms ? "!border-accent !text-content" : ""}`} onClick={() => setRangeMs(r.ms)}>
              {r.label}
            </button>
          ))}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-3 border-b border-border-light px-4 py-2">
        <div className="min-w-0 flex-1">
          <StackedBars data={hist} series={LOG_LEVEL_SERIES} bucketMs={bucketMs} height={80} />
        </div>
        <span className="w-24 shrink-0 text-right text-xs text-content-muted">
          {loading ? <Spinner size={12} /> : `${fmtCount(total)} lines`}
        </span>
      </div>
      <div className="min-h-0 flex-1">
        {error ? (
          <EmptyState title="Query failed">{error}</EmptyState>
        ) : (
          <LogView
            rows={logRows}
            showSource
            onEndReached={loadMore}
            emptyText={loading ? "Loading…" : "No stored lines match. Lines are kept for the retention period set in Settings."}
          />
        )}
      </div>
    </div>
  );
}
