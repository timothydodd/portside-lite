/** Cores → "1.25" / "250m". */
export function fmtCpu(cores: number | null | undefined): string {
  if (cores == null) return "—";
  if (cores === 0) return "0";
  if (cores < 0.9995) return `${Math.round(cores * 1000)}m`; // above that it would read "1000m"
  return cores.toFixed(cores >= 10 ? 0 : 2);
}

/** Bytes → binary units. */
export function fmtBytes(bytes: number | null | undefined): string {
  if (bytes == null) return "—";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let v = bytes;
  let i = 0;
  // Step up a unit once the rounded value would read 1024 (e.g. "1024 KiB").
  while (Math.round(v) >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 99.95 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

export function fmtPct(v: number | null | undefined): string {
  if (v == null || !isFinite(v)) return "—";
  return `${Math.round(v)}%`;
}

export function pct(part: number | null | undefined, whole: number): number | null {
  if (part == null || !whole) return null;
  return (part / whole) * 100;
}

/** Compact counts: 1,284 / 12.9K / 4.2M. */
export function fmtCount(n: number): string {
  if (n < 10_000) return n.toLocaleString();
  if (n < 999_950) return `${(n / 1000).toFixed(1)}K`; // above that it would read "1000.0K"
  return `${(n / 1_000_000).toFixed(1)}M`;
}

/** Age like kubectl: 45s, 12m, 3h, 5d. */
export function fmtAge(ms: number | null | undefined, now = Date.now()): string {
  if (ms == null) return "—";
  const s = Math.max(0, Math.floor((now - ms) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

export function fmtAgo(ms: number | null | undefined, now = Date.now()): string {
  if (ms == null) return "never";
  return `${fmtAge(ms, now)} ago`;
}

export function fmtDuration(ms: number): string {
  const s = Math.floor(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 48) return `${h}h ${m % 60}m`;
  return `${Math.floor(h / 24)}d ${h % 24}h`;
}

export function fmtTime(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });
}

export function fmtDateTime(ms: number): string {
  const d = new Date(ms);
  return `${d.toLocaleDateString([], { month: "short", day: "numeric" })} ${fmtTime(ms)}`;
}

export function errorMessage(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
