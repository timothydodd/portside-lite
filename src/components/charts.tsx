import { useMemo, useRef, useState, type ReactNode } from "react";
import { fmtDateTime, fmtTime } from "../lib/format";

// Chart conventions (see dataviz spec): one series per chart and one y-axis;
// 2px lines with a ~10% area wash; hairline solid grid; bars ≤24px with a
// rounded data-end and a 2px surface gap; every chart has a hover tooltip;
// text uses text tokens, never the series color.

const PAD = { top: 10, right: 12, bottom: 22, left: 48 };

function niceMax(v: number): number {
  if (v <= 0) return 1;
  const exp = Math.pow(10, Math.floor(Math.log10(v)));
  const n = v / exp;
  const step = n <= 1 ? 1 : n <= 2 ? 2 : n <= 5 ? 5 : 10;
  return step * exp;
}

function useWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(600);
  const observer = useMemo(
    () =>
      typeof ResizeObserver !== "undefined"
        ? new ResizeObserver((entries) => setWidth(Math.max(200, entries[0].contentRect.width)))
        : null,
    [],
  );
  const setRef = (el: HTMLDivElement | null) => {
    if (ref.current && observer) observer.unobserve(ref.current);
    ref.current = el;
    if (el && observer) observer.observe(el);
  };
  return { setRef, width };
}

function timeTicks(t0: number, t1: number, count: number): number[] {
  if (t1 <= t0) return [t0];
  const out: number[] = [];
  for (let i = 0; i <= count; i++) out.push(t0 + ((t1 - t0) * i) / count);
  return out;
}

function tickLabel(t: number, spanMs: number): string {
  const d = new Date(t);
  if (spanMs > 2 * 86_400_000) return d.toLocaleDateString([], { month: "short", day: "numeric" });
  return fmtTime(t).slice(0, 5);
}

function Tooltip({ x, width, children }: { x: number; width: number; children: ReactNode }) {
  const left = Math.min(Math.max(x + 12, 0), width - 170);
  return (
    <div
      className="pointer-events-none absolute top-1 z-10 min-w-[150px] rounded-md border border-border bg-raised px-2.5 py-1.5 text-xs shadow-[var(--shadow-md)]"
      style={{ left }}
    >
      {children}
    </div>
  );
}

export interface Point {
  t: number;
  v: number;
}

/** Single-series time line with area wash, crosshair and tooltip. */
export function LineChart({
  data,
  color,
  format,
  height = 160,
  yMax,
  label,
}: {
  data: Point[];
  color: string;
  format: (v: number) => string;
  height?: number;
  /** Fixed ceiling (e.g. capacity); otherwise a nice max over the data. */
  yMax?: number;
  label: string;
}) {
  const { setRef, width } = useWidth();
  const [hover, setHover] = useState<number | null>(null);

  const innerW = width - PAD.left - PAD.right;
  const innerH = height - PAD.top - PAD.bottom;
  const t0 = data[0]?.t ?? 0;
  const t1 = data[data.length - 1]?.t ?? 1;
  const max = yMax ?? niceMax(Math.max(...data.map((d) => d.v), 0) * 1.1);
  const x = (t: number) => PAD.left + (t1 === t0 ? innerW / 2 : ((t - t0) / (t1 - t0)) * innerW);
  const y = (v: number) => PAD.top + innerH - (Math.min(v, max) / max) * innerH;

  const line = data.map((d, i) => `${i ? "L" : "M"}${x(d.t).toFixed(1)},${y(d.v).toFixed(1)}`).join("");
  const area = data.length
    ? `${line}L${x(t1).toFixed(1)},${PAD.top + innerH}L${x(t0).toFixed(1)},${PAD.top + innerH}Z`
    : "";
  const yTicks = [0, 0.25, 0.5, 0.75, 1].map((f) => f * max);

  const onMove = (e: React.MouseEvent<SVGSVGElement>) => {
    if (!data.length) return;
    const rect = e.currentTarget.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const t = t0 + ((px - PAD.left) / innerW) * (t1 - t0);
    let best = 0;
    for (let i = 1; i < data.length; i++) if (Math.abs(data[i].t - t) < Math.abs(data[best].t - t)) best = i;
    setHover(best);
  };

  const h = hover != null ? data[hover] : null;

  return (
    <div ref={setRef} className="relative w-full" style={{ height }}>
      {data.length < 2 ? (
        <div className="flex h-full items-center justify-center text-xs text-content-muted">
          Not enough history yet — samples accumulate every poll.
        </div>
      ) : (
        <svg
          width={width}
          height={height}
          onMouseMove={onMove}
          onMouseLeave={() => setHover(null)}
          role="img"
          aria-label={label}
        >
          {yTicks.map((v) => (
            <g key={v}>
              <line x1={PAD.left} x2={width - PAD.right} y1={y(v)} y2={y(v)} stroke="var(--chart-grid)" strokeWidth={1} />
              <text x={PAD.left - 6} y={y(v) + 4} textAnchor="end" fontSize={10} fill="var(--text-muted)">
                {format(v)}
              </text>
            </g>
          ))}
          {timeTicks(t0, t1, Math.max(2, Math.floor(innerW / 110))).map((t, i, arr) => (
            <text
              key={t}
              x={x(t)}
              y={height - 6}
              textAnchor={i === 0 ? "start" : i === arr.length - 1 ? "end" : "middle"}
              fontSize={10}
              fill="var(--text-muted)"
            >
              {tickLabel(t, t1 - t0)}
            </text>
          ))}
          <path d={area} fill={color} opacity={0.1} />
          <path d={line} fill="none" stroke={color} strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
          {h && (
            <>
              <line x1={x(h.t)} x2={x(h.t)} y1={PAD.top} y2={PAD.top + innerH} stroke="var(--text-muted)" strokeWidth={1} />
              <circle cx={x(h.t)} cy={y(h.v)} r={4.5} fill={color} stroke="var(--bg-surface)" strokeWidth={2} />
            </>
          )}
        </svg>
      )}
      {h && (
        <Tooltip x={x(h.t)} width={width}>
          <div className="text-content-muted">{fmtDateTime(h.t)}</div>
          <div className="mt-0.5 flex items-center gap-1.5 text-content">
            <span className="h-2 w-2 rounded-full" style={{ background: color }} />
            {label}: <span className="font-semibold tabular-nums">{format(h.v)}</span>
          </div>
        </Tooltip>
      )}
    </div>
  );
}

export interface StackSeries<T> {
  key: keyof T & string;
  label: string;
  color: string;
}

/** Stacked time columns (e.g. log volume by level) with legend and tooltip. */
export function StackedBars<T extends { bucketMs: number }>({
  data,
  series,
  height = 180,
  bucketMs,
  onSelect,
}: {
  data: T[];
  series: readonly StackSeries<T>[];
  height?: number;
  bucketMs: number;
  onSelect?: (row: T) => void;
}) {
  const { setRef, width } = useWidth();
  const [hover, setHover] = useState<number | null>(null);
  const innerW = width - PAD.left - PAD.right;
  const innerH = height - PAD.top - PAD.bottom;

  const total = (r: T) => series.reduce((s, k) => s + (r[k.key] as unknown as number), 0);
  const max = niceMax(Math.max(...data.map(total), 0));
  const t0 = data[0]?.bucketMs ?? 0;
  const t1 = (data[data.length - 1]?.bucketMs ?? 0) + bucketMs;
  const slots = Math.max(1, Math.round((t1 - t0) / bucketMs));
  const slotW = innerW / slots;
  const barW = Math.max(2, Math.min(24, slotW - 2));
  const x = (t: number) => PAD.left + ((t - t0) / bucketMs) * slotW + (slotW - barW) / 2;
  const y = (v: number) => (v / max) * innerH;
  const yTicks = [0, 0.5, 1].map((f) => f * max);
  const GAP = 2;

  const h = hover != null ? data[hover] : null;

  return (
    <div>
      <div className="mb-2 flex flex-wrap gap-3 text-xs text-content-secondary">
        {series.map((s) => (
          <span key={s.key} className="inline-flex items-center gap-1.5">
            <span className="h-2.5 w-2.5 rounded-sm" style={{ background: s.color }} />
            {s.label}
          </span>
        ))}
      </div>
      <div ref={setRef} className="relative w-full" style={{ height }}>
        {data.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-content-muted">No log data in this range.</div>
        ) : (
          <svg width={width} height={height} role="img" aria-label="Log volume by level" onMouseLeave={() => setHover(null)}>
            {yTicks.map((v) => (
              <g key={v}>
                <line
                  x1={PAD.left}
                  x2={width - PAD.right}
                  y1={PAD.top + innerH - y(v)}
                  y2={PAD.top + innerH - y(v)}
                  stroke="var(--chart-grid)"
                  strokeWidth={1}
                />
                <text x={PAD.left - 6} y={PAD.top + innerH - y(v) + 4} textAnchor="end" fontSize={10} fill="var(--text-muted)">
                  {Math.round(v).toLocaleString()}
                </text>
              </g>
            ))}
            {timeTicks(t0, t1, Math.max(2, Math.floor(innerW / 110))).map((t, i, arr) => (
              <text
                key={t}
                x={PAD.left + ((t - t0) / (t1 - t0)) * innerW}
                y={height - 6}
                textAnchor={i === 0 ? "start" : i === arr.length - 1 ? "end" : "middle"}
                fontSize={10}
                fill="var(--text-muted)"
              >
                {tickLabel(t, t1 - t0)}
              </text>
            ))}
            {data.map((row, i) => {
              let acc = 0;
              const segs = series
                .map((s) => ({ s, v: row[s.key] as unknown as number }))
                .filter((d) => d.v > 0);
              return (
                <g
                  key={row.bucketMs}
                  onMouseEnter={() => setHover(i)}
                  onClick={() => onSelect?.(row)}
                  className={onSelect ? "cursor-pointer" : undefined}
                >
                  {/* Hit target spans the full slot height, bigger than the mark. */}
                  <rect x={x(row.bucketMs) - (slotW - barW) / 2} y={PAD.top} width={slotW} height={innerH} fill="transparent" />
                  {segs.map(({ s, v }, j) => {
                    // Each segment above the first leaves a surface gap under itself.
                    const hgt = Math.max(1, y(v) - (j > 0 ? GAP : 0));
                    const top = PAD.top + innerH - y(acc) - y(v);
                    acc += v;
                    const isTop = j === segs.length - 1;
                    const r = isTop ? Math.min(4, barW / 2, hgt) : 0;
                    return (
                      <path
                        key={s.key}
                        d={roundedTop(x(row.bucketMs), top, barW, hgt, r)}
                        fill={s.color}
                        opacity={hover == null || hover === i ? 1 : 0.55}
                      />
                    );
                  })}
                </g>
              );
            })}
          </svg>
        )}
        {h && (
          <Tooltip x={x(h.bucketMs)} width={width}>
            <div className="mb-1 text-content-muted">
              {fmtDateTime(h.bucketMs)} · {Math.round(bucketMs / 60000)} min
            </div>
            {series
              .slice()
              .reverse()
              .map((s) => (
                <div key={s.key} className="flex items-center justify-between gap-4 text-content">
                  <span className="inline-flex items-center gap-1.5">
                    <span className="h-2 w-2 rounded-sm" style={{ background: s.color }} />
                    {s.label}
                  </span>
                  <span className="tabular-nums">{(h[s.key] as unknown as number).toLocaleString()}</span>
                </div>
              ))}
            {onSelect && <div className="mt-1 text-content-muted">Click to view these logs</div>}
          </Tooltip>
        )}
      </div>
    </div>
  );
}

function roundedTop(x: number, y: number, w: number, h: number, r: number): string {
  if (r <= 0) return `M${x},${y}h${w}v${h}h${-w}Z`;
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}

/** Log-level series in severity order (bottom → top: info first, errors on top). */
export const LOG_LEVEL_SERIES = [
  { key: "debug", label: "Debug", color: "var(--lvl-debug)" },
  { key: "info", label: "Info", color: "var(--lvl-info)" },
  { key: "warning", label: "Warning", color: "var(--lvl-warning)" },
  { key: "error", label: "Error", color: "var(--lvl-error)" },
] as const;
