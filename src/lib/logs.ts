import { useEffect, useState } from "react";
import type { LogLevel } from "./types";

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
