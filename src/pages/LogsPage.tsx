import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, X, ZoomOut } from "lucide-react";
import * as ipc from "../lib/ipc";
import { errorMessage, fmtCount, fmtDateTime } from "../lib/format";
import type { HistogramBucket, LogLevel, LogQuery, LogRecord } from "../lib/types";
import { LOG_LEVEL_SERIES, StackedBars } from "../components/charts";
import LogView from "../components/LogView";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput, Spinner } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";

const RANGES = [
  { label: "15m", ms: 15 * 60_000 },
  { label: "1h", ms: 3600_000 },
  { label: "6h", ms: 6 * 3600_000 },
  { label: "24h", ms: 24 * 3600_000 },
  { label: "7d", ms: 7 * 86_400_000 },
];
const LEVELS: LogLevel[] = ["error", "warning", "info", "debug"];
const PAGE = 500;

/** Pick a bucket width that yields roughly 60 columns. */
function bucketFor(spanMs: number): number {
  const steps = [60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000, 3600_000, 3 * 3600_000, 6 * 3600_000];
  return steps.find((s) => spanMs / s <= 80) ?? 12 * 3600_000;
}

function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export default function LogsPage() {
  const snapshot = useClusterStore((s) => s.snapshot);
  const logTick = useClusterStore((s) => s.logSyncTick);
  const settings = useClusterStore((s) => s.settings);
  const preset = useNavStore((s) => s.logPreset);
  const consumePreset = useNavStore((s) => s.consumeLogPreset);

  const [search, setSearch] = useState("");
  const [namespace, setNamespace] = useState("");
  const [pod, setPod] = useState("");
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set());
  const [rangeMs, setRangeMs] = useState(24 * 3600_000);
  /** Zoomed window from clicking a histogram bucket. */
  const [zoom, setZoom] = useState<{ since: number; until: number } | null>(null);

  // Deep links ("view error logs for this pod") land here — also while the
  // page is already open.
  useEffect(() => {
    const p = preset && consumePreset();
    if (!p) return;
    setSearch(p.search ?? "");
    setNamespace(p.namespace ?? "");
    setPod(p.pod ?? "");
    setLevels(new Set(p.levels ?? []));
    setZoom(null);
  }, [preset, consumePreset]);

  const [rows, setRows] = useState<LogRecord[]>([]);
  const [hist, setHist] = useState<HistogramBucket[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loadingMore = useRef(false);

  const debouncedSearch = useDebounced(search, 300);
  const now = useMemo(() => Date.now(), [logTick, rangeMs, zoom]); // eslint-disable-line react-hooks/exhaustive-deps
  const since = zoom?.since ?? now - rangeMs;
  const until = zoom?.until ?? null;
  const bucketMs = bucketFor((until ?? now) - since);

  const query: LogQuery = useMemo(
    () => ({
      search: debouncedSearch || null,
      namespace: namespace || null,
      pod: pod || null,
      levels: [...levels],
      sinceMs: since,
      untilMs: until,
      limit: PAGE,
    }),
    [debouncedSearch, namespace, pod, levels, since, until],
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

  const pods = useMemo(
    () =>
      [...new Set((snapshot?.pods ?? []).filter((p) => !namespace || p.namespace === namespace).map((p) => p.name))].sort(),
    [snapshot, namespace],
  );
  const total = hist.reduce((s, b) => s + b.trace + b.debug + b.info + b.warning + b.error, 0);
  const errors = hist.reduce((s, b) => s + b.error, 0);

  const logRows = useMemo(
    () =>
      rows.map((r) => ({
        key: r.id,
        tsMs: r.tsMs,
        level: r.level,
        message: r.message,
        source: `${r.namespace}/${r.pod}${r.container && r.container !== r.pod ? ` [${r.container}]` : ""}`,
      })),
    [rows],
  );

  if (settings && !settings.collectLogs) {
    return (
      <div>
        <PageHeader title="Logs" />
        <EmptyState title="Log collection is off">Turn it on in Settings to pull container logs into the local store for search and analytics.</EmptyState>
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      <PageHeader
        title="Log explorer"
        subtitle={`${fmtCount(total)} lines in range · ${fmtCount(errors)} errors · stored locally, searchable offline`}
      >
        <button className="btn-ghost" onClick={() => void ipc.syncLogsNow()} title="Pull new logs from the cluster now">
          <RefreshCw size={14} /> Sync now
        </button>
      </PageHeader>

      <div className="flex flex-wrap items-center gap-2 border-b border-border-light px-6 py-3">
        <SearchInput value={search} onChange={setSearch} placeholder="Search messages (all words, prefix match)…" className="w-80" />
        <NamespaceSelect
          namespaces={snapshot?.namespaces ?? []}
          value={namespace}
          onChange={(v) => {
            setNamespace(v);
            setPod("");
          }}
        />
        <select className="field max-w-[240px]" value={pod} onChange={(e) => setPod(e.target.value)}>
          <option value="">All pods</option>
          {pod && !pods.includes(pod) && <option value={pod}>{pod} (gone)</option>}
          {pods.map((p) => (
            <option key={p} value={p}>
              {p}
            </option>
          ))}
        </select>
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
        <div className="ml-auto flex items-center gap-1">
          {zoom ? (
            <button className="btn-chip !border-accent !text-content" onClick={() => setZoom(null)}>
              <ZoomOut size={12} /> {fmtDateTime(zoom.since)} – {fmtDateTime(zoom.until).split(" ").pop()} <X size={12} />
            </button>
          ) : (
            RANGES.map((r) => (
              <button
                key={r.label}
                className={`btn-chip ${rangeMs === r.ms ? "!border-accent !text-content" : ""}`}
                onClick={() => setRangeMs(r.ms)}
              >
                {r.label}
              </button>
            ))
          )}
        </div>
      </div>

      <div className="border-b border-border-light px-6 py-3">
        <StackedBars
          data={hist}
          series={LOG_LEVEL_SERIES}
          bucketMs={bucketMs}
          height={110}
          onSelect={(b) => setZoom({ since: b.bucketMs, until: b.bucketMs + bucketMs })}
        />
      </div>

      <div className="relative min-h-0 flex-1">
        {loading && (
          <div className="absolute right-4 top-2 z-10">
            <Spinner />
          </div>
        )}
        {error ? (
          <EmptyState title="Query failed">{error}</EmptyState>
        ) : (
          <LogView
            rows={logRows}
            showSource
            onEndReached={loadMore}
            emptyText={loading ? "Loading…" : "No logs match. Logs appear here after the first sync (see the status bar)."}
          />
        )}
      </div>
    </div>
  );
}
