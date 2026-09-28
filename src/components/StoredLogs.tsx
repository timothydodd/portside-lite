import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as ipc from "../lib/ipc";
import { errorMessage, fmtCount } from "../lib/format";
import { bucketFor, FILTER_LEVELS, LOG_RANGES, useDebounced } from "../lib/logs";
import type { HistogramBucket, LogLevel, LogQuery, LogRecord, LogSource, WorkloadRef } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { LOG_LEVEL_SERIES, StackedBars } from "./charts";
import LogView from "./LogView";
import { EmptyState, Spinner } from "./ui";

const PAGE = 500;

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
  const [rows, setRows] = useState<LogRecord[]>([]);
  const [hist, setHist] = useState<HistogramBucket[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loadingMore = useRef(false);

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
      limit: PAGE,
    }),
    [debouncedSearch, namespace, workload, pod, levels, since],
  );

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [r, h] = await Promise.all([ipc.queryLogs(query), ipc.logHistogram({ ...query, limit: null }, bucketMs)]);
      setRows(r);
      setHist(h);
      setHasMore(r.length === PAGE);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [query, bucketMs]);

  useEffect(() => void load(), [load]);

  const loadMore = useCallback(async () => {
    if (!hasMore || loadingMore.current || !rows.length) return;
    loadingMore.current = true;
    try {
      const more = await ipc.queryLogs({ ...query, beforeId: rows[rows.length - 1].id });
      setRows((r) => [...r, ...more]);
      setHasMore(more.length === PAGE);
    } finally {
      loadingMore.current = false;
    }
  }, [hasMore, rows, query]);

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
