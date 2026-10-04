import { useCallback, useEffect, useRef, useState } from "react";
import { errorMessage } from "./format";
import * as ipc from "./ipc";
import type { HistogramBucket, LogLevel, LogQuery, LogRecord } from "./types";
import { toast } from "../stores/toast";

/** Time windows offered over stored logs. `ms: null` = everything stored. */
export const LOG_RANGES: { label: string; ms: number | null }[] = [
  { label: "15m", ms: 15 * 60_000 },
  { label: "1h", ms: 3600_000 },
  { label: "6h", ms: 6 * 3600_000 },
  { label: "24h", ms: 24 * 3600_000 },
  { label: "7d", ms: 7 * 86_400_000 },
  { label: "All", ms: null },
];

export const FILTER_LEVELS: LogLevel[] = ["error", "warning", "info", "debug"];

/** Pick a histogram bucket width that yields roughly 60 columns. */
export function bucketFor(spanMs: number): number {
  const steps = [60_000, 5 * 60_000, 15 * 60_000, 30 * 60_000, 3600_000, 3 * 3600_000, 6 * 3600_000, 12 * 3600_000];
  return steps.find((s) => spanMs / s <= 80) ?? 86_400_000;
}

export function useDebounced<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const t = setTimeout(() => setV(value), ms);
    return () => clearTimeout(t);
  }, [value, ms]);
  return v;
}

export const LOG_PAGE = 500;

/**
 * Stored log lines and their histogram for a query, paged newest first.
 *
 * `resetKey` names what the user asked for (filters, range, zoom): a change
 * starts over at page one. `tick` is a log sync: new lines are merged in at
 * the top, so someone who has scrolled down a few pages stays where they are.
 * `query` itself may drift between the two (a "last hour" window moves with
 * the clock) without reloading anything.
 */
export function useLogResults(query: LogQuery, bucketMs: number, resetKey: string, tick: number) {
  const [rows, setRows] = useState<LogRecord[]>([]);
  const [hist, setHist] = useState<HistogramBucket[]>([]);
  const [loading, setLoading] = useState(false);
  const [hasMore, setHasMore] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const latest = useRef({ query, bucketMs });
  latest.current = { query, bucketMs };
  const shown = useRef({ rows, hasMore });
  shown.current = { rows, hasMore };
  // Bumped by every full load; an answer that belongs to an older one is dropped.
  const generation = useRef(0);
  const loadingMore = useRef(false);

  const fetchFirst = () => {
    const { query, bucketMs } = latest.current;
    return Promise.all([ipc.queryLogs({ ...query, limit: LOG_PAGE }), ipc.logHistogram({ ...query, limit: null }, bucketMs)]);
  };

  useEffect(() => {
    const mine = ++generation.current;
    setLoading(true);
    setError(null);
    fetchFirst()
      .then(([r, h]) => {
        if (mine !== generation.current) return;
        setRows(r);
        setHist(h);
        setHasMore(r.length === LOG_PAGE);
      })
      .catch((e) => mine === generation.current && setError(errorMessage(e)))
      .finally(() => mine === generation.current && setLoading(false));
  }, [resetKey]); // eslint-disable-line react-hooks/exhaustive-deps

  const firstTick = useRef(tick);
  useEffect(() => {
    if (tick === firstTick.current) return;
    const mine = generation.current;
    fetchFirst()
      .then(([r, h]) => {
        if (mine !== generation.current) return;
        setHist(h);
        const prev = shown.current.rows;
        const have = new Set(prev.map((x) => x.id));
        // One page on screen, or so many new lines that the newest page no
        // longer overlaps what's shown: page one is the whole truth again.
        if (prev.length <= LOG_PAGE || !r.some((x) => have.has(x.id))) {
          setRows(r);
          setHasMore(r.length === LOG_PAGE);
          return;
        }
        const fresh = r.filter((x) => !have.has(x.id));
        if (fresh.length) setRows([...fresh, ...prev].sort((a, b) => b.tsMs - a.tsMs || b.id - a.id));
      })
      .catch(() => undefined); // the next sync tries again; what's shown stays
  }, [tick]); // eslint-disable-line react-hooks/exhaustive-deps

  const loadMore = useCallback(async () => {
    const { rows, hasMore } = shown.current;
    if (!hasMore || loadingMore.current || !rows.length) return;
    loadingMore.current = true;
    const mine = generation.current;
    try {
      const more = await ipc.queryLogs({ ...latest.current.query, limit: LOG_PAGE, beforeId: rows[rows.length - 1].id });
      if (mine !== generation.current) return;
      const have = new Set(shown.current.rows.map((x) => x.id));
      setRows((r) => [...r, ...more.filter((x) => !have.has(x.id))]);
      setHasMore(more.length === LOG_PAGE);
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      loadingMore.current = false;
    }
  }, []);

  return { rows, hist, loading, hasMore, error, loadMore };
}
