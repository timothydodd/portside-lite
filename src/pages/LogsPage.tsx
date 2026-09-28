import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCw, X, ZoomOut } from "lucide-react";
import * as ipc from "../lib/ipc";
import { errorMessage, fmtCount, fmtDateTime } from "../lib/format";
import { ownedBy } from "../lib/workloads";
import { bucketFor, FILTER_LEVELS as LEVELS, LOG_RANGES as RANGES, useDebounced } from "../lib/logs";
import type { HistogramBucket, LogLevel, LogQuery, LogRecord, LogSource } from "../lib/types";
import { LOG_LEVEL_SERIES, StackedBars } from "../components/charts";
import LogView from "../components/LogView";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput, Spinner } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";

const PAGE = 500;

export default function LogsPage() {
  const snapshot = useClusterStore((s) => s.snapshot);
  const logTick = useClusterStore((s) => s.logSyncTick);
  const settings = useClusterStore((s) => s.settings);
  const preset = useNavStore((s) => s.logPreset);
  const consumePreset = useNavStore((s) => s.consumeLogPreset);

  const [search, setSearch] = useState("");
  const [namespace, setNamespace] = useState("");
  const [pod, setPod] = useState("");
  /** "Kind/name": every pod of that workload, gone ones included. */
  const [workload, setWorkload] = useState("");
  const [levels, setLevels] = useState<Set<LogLevel>>(new Set());
  const [rangeMs, setRangeMs] = useState<number | null>(24 * 3600_000);
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
    setWorkload(p.workload ? `${p.workload.kind}/${p.workload.name}` : "");
    if (p.workload) setRangeMs(null); // its whole stored history
    setLevels(new Set(p.levels ?? []));
    setZoom(null);
  }, [preset, consumePreset]);

  const [rows, setRows] = useState<LogRecord[]>([]);
  const [hist, setHist] = useState<HistogramBucket[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const loadingMore = useRef(false);

  // Pods with stored lines, alive or gone: feeds the pod and workload pickers.
  const [sources, setSources] = useState<LogSource[]>([]);
  useEffect(() => {
    ipc.logSources(null, null).then(setSources).catch(() => setSources([]));
  }, [logTick]);

  const debouncedSearch = useDebounced(search, 300);
  const now = useMemo(() => Date.now(), [logTick, rangeMs, zoom]); // eslint-disable-line react-hooks/exhaustive-deps
  const oldest = useMemo(() => (sources.length ? Math.min(...sources.map((s) => s.firstMs)) : now - 86_400_000), [sources, now]);
  const since = zoom?.since ?? (rangeMs == null ? null : now - rangeMs);
  const until = zoom?.until ?? null;
  const bucketMs = bucketFor((until ?? now) - (since ?? oldest));
  const workloadRef = useMemo(() => {
    const i = workload.indexOf("/");
    return i > 0 ? { kind: workload.slice(0, i), name: workload.slice(i + 1) } : null;
  }, [workload]);

  const query: LogQuery = useMemo(
    () => ({
      search: debouncedSearch || null,
      namespace: namespace || null,
      workload: namespace ? workloadRef : null,
      pod: pod || null,
      levels: [...levels],
      sinceMs: since,
      untilMs: until,
      limit: PAGE,
    }),
    [debouncedSearch, namespace, workloadRef, pod, levels, since, until],
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

  // Live pods plus pods that only exist in the store now (scaled down,
  // replaced, deleted, archived).
  const alive = useMemo(() => new Set((snapshot?.pods ?? []).map((p) => `${p.namespace}/${p.name}`)), [snapshot]);
  const pods = useMemo(() => {
    const inNs = (ns: string) => !namespace || ns === namespace;
    const inWorkload = (ownerKind: string | null, ownerName: string | null) =>
      !workloadRef || ownedBy(ownerKind, ownerName, workloadRef.kind, workloadRef.name);
    const names = new Map<string, boolean>();
    for (const p of snapshot?.pods ?? []) if (inNs(p.namespace) && inWorkload(p.ownerKind, p.ownerName)) names.set(p.name, true);
    for (const s of sources)
      if (inNs(s.namespace) && inWorkload(s.ownerKind, s.ownerName) && !names.has(s.pod)) names.set(s.pod, alive.has(`${s.namespace}/${s.pod}`));
    return [...names.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([name, live]) => ({ name, live }));
  }, [snapshot, sources, namespace, workloadRef, alive]);
  // Workloads that have had pods in the namespace, including ones no longer on the cluster.
  const workloads = useMemo(() => {
    if (!namespace) return [];
    const set = new Set<string>();
    for (const w of snapshot?.workloads ?? []) if (w.namespace === namespace) set.add(`${w.kind}/${w.name}`);
    for (const s of sources) if (s.namespace === namespace && s.ownerKind && s.ownerName) set.add(`${s.ownerKind}/${s.ownerName}`);
    return [...set].sort();
  }, [snapshot, sources, namespace]);
  const liveWorkloads = useMemo(() => new Set((snapshot?.workloads ?? []).map((w) => `${w.namespace}/${w.kind}/${w.name}`)), [snapshot]);
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
            setWorkload("");
            setPod("");
          }}
        />
        <select
          className="field max-w-[240px]"
          value={workload}
          disabled={!namespace}
          title={namespace ? "Every pod a workload has had, including gone ones" : "Pick a namespace to filter by workload"}
          onChange={(e) => {
            setWorkload(e.target.value);
            setPod("");
          }}
        >
          <option value="">All workloads</option>
          {workload && !workloads.includes(workload) && <option value={workload}>{workload}</option>}
          {workloads.map((w) => (
            <option key={w} value={w}>
              {w}
              {liveWorkloads.has(`${namespace}/${w}`) ? "" : " (gone)"}
            </option>
          ))}
        </select>
        <select className="field max-w-[240px]" value={pod} onChange={(e) => setPod(e.target.value)}>
          <option value="">All pods</option>
          {pod && !pods.some((p) => p.name === pod) && <option value={pod}>{pod} (gone)</option>}
          {pods.map((p) => (
            <option key={p.name} value={p.name}>
              {p.name}
              {p.live ? "" : " (gone)"}
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
