import { Fragment, useCallback, useEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { AlertOctagon, AlertTriangle, Info, Loader2, MoreHorizontal, Search, X } from "lucide-react";
import type { Severity } from "../lib/types";
import { isWindows } from "./WindowControls";

// --- severity -----------------------------------------------------------------

const SEVERITY_META: Record<Severity, { label: string; tint: string; Icon: typeof Info }> = {
  critical: { label: "Critical", tint: "tint-critical", Icon: AlertOctagon },
  warning: { label: "Warning", tint: "tint-warning", Icon: AlertTriangle },
  info: { label: "Info", tint: "tint-info", Icon: Info },
};

export function SeverityIcon({ severity, size = 16 }: { severity: Severity; size?: number }) {
  const { Icon } = SEVERITY_META[severity];
  const color = severity === "critical" ? "text-critical" : severity === "warning" ? "text-warning" : "text-info";
  return <Icon size={size} className={`shrink-0 ${color}`} aria-label={SEVERITY_META[severity].label} />;
}

/** Icon + label — status is never conveyed by color alone. */
export function SeverityBadge({ severity }: { severity: Severity }) {
  const { label, tint, Icon } = SEVERITY_META[severity];
  return (
    <span className={`inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] font-semibold ${tint}`}>
      <Icon size={12} />
      {label}
    </span>
  );
}

// --- pod / generic status pill ------------------------------------------------

const GOOD = new Set(["Running", "Completed", "Succeeded", "Bound", "Ready"]);
const BAD_PREFIX = ["CrashLoop", "Err", "ImagePull", "Init:Err", "Init:CrashLoop", "OOM", "Failed", "Error", "Evicted", "CreateContainer", "InvalidImage", "Lost"];

export function statusTone(status: string): "good" | "warning" | "critical" | "muted" {
  if (status === "Completed" || status === "Succeeded") return "muted";
  if (GOOD.has(status)) return "good";
  if (BAD_PREFIX.some((p) => status.startsWith(p))) return "critical";
  return "warning";
}

export function StatusPill({ status, tone }: { status: string; tone?: ReturnType<typeof statusTone> }) {
  const t = tone ?? statusTone(status);
  const dot =
    t === "good" ? "bg-good" : t === "critical" ? "bg-critical" : t === "warning" ? "bg-warning" : "bg-content-muted";
  return (
    <span className="inline-flex items-center gap-1.5 whitespace-nowrap text-xs text-content-secondary">
      <span className={`h-2 w-2 rounded-full ${dot}`} />
      {status}
    </span>
  );
}

// --- meter --------------------------------------------------------------------

/** Usage meter: fill carries severity, the track is a lighter step of the same hue. */
export function Meter({
  value,
  warnAt = 75,
  critAt = 90,
  label,
  className = "",
}: {
  value: number | null;
  warnAt?: number;
  critAt?: number;
  label?: ReactNode;
  className?: string;
}) {
  const v = value == null ? 0 : Math.max(0, Math.min(100, value));
  const color = value == null ? "var(--border)" : v >= critAt ? "var(--critical)" : v >= warnAt ? "var(--warning)" : "var(--accent)";
  return (
    <div className={className}>
      {label && <div className="mb-1 flex justify-between text-xs text-content-secondary">{label}</div>}
      <div
        className="h-1.5 w-full overflow-hidden rounded-full"
        style={{ backgroundColor: `color-mix(in srgb, ${color} 18%, transparent)` }}
        role="meter"
        aria-valuenow={value ?? undefined}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className="h-full rounded-full transition-[width] duration-500" style={{ width: `${v}%`, backgroundColor: color }} />
      </div>
    </div>
  );
}

// --- layout bits --------------------------------------------------------------

export function StatTile({
  label,
  value,
  sub,
  tone,
  onClick,
}: {
  label: string;
  value: ReactNode;
  sub?: ReactNode;
  tone?: "critical" | "warning" | "good";
  onClick?: () => void;
}) {
  const ring = tone === "critical" ? "border-critical/50" : tone === "warning" ? "border-warning/50" : "border-border-light";
  const Tag = onClick ? "button" : "div";
  return (
    <Tag
      onClick={onClick}
      className={`card flex flex-col items-start gap-1 border px-4 py-3 text-left ${ring} ${onClick ? "transition-colors hover:border-accent" : ""}`}
    >
      <span className="text-xs text-content-muted">{label}</span>
      <span className="text-2xl font-semibold tabular-nums text-content">{value}</span>
      {sub && <span className="text-xs text-content-secondary">{sub}</span>}
    </Tag>
  );
}

export function PageHeader({ title, subtitle, children }: { title: string; subtitle?: ReactNode; children?: ReactNode }) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-3 border-b border-border-light px-6 pb-3 pt-5">
      <div>
        <h1 className="text-lg font-semibold text-content">{title}</h1>
        {subtitle && <p className="mt-0.5 text-xs text-content-muted">{subtitle}</p>}
      </div>
      {children && <div className="flex flex-wrap items-center gap-2">{children}</div>}
    </div>
  );
}

export function EmptyState({ icon, title, children }: { icon?: ReactNode; title: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col items-center justify-center gap-2 px-6 py-16 text-center">
      {icon && <div className="text-content-muted">{icon}</div>}
      <div className="text-sm font-medium text-content">{title}</div>
      {children && <div className="max-w-md text-xs text-content-muted">{children}</div>}
    </div>
  );
}

export function Spinner({ size = 16 }: { size?: number }) {
  return <Loader2 size={size} className="animate-spin text-content-muted" />;
}

export function SearchInput({
  value,
  onChange,
  placeholder = "Filter…",
  className = "",
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  className?: string;
}) {
  return (
    <div className={`relative ${className}`}>
      <Search size={14} className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-content-muted" />
      <input className="field w-full pl-8" value={value} onChange={(e) => onChange(e.target.value)} placeholder={placeholder} />
    </div>
  );
}

export function NamespaceSelect({
  namespaces,
  value,
  onChange,
}: {
  namespaces: string[];
  value: string;
  onChange: (v: string) => void;
}) {
  return (
    <select className="field" value={value} onChange={(e) => onChange(e.target.value)}>
      <option value="">All namespaces</option>
      {namespaces.map((n) => (
        <option key={n} value={n}>
          {n}
        </option>
      ))}
    </select>
  );
}

/** Sortable table header cell. */
export function SortTh<K extends string>({
  label,
  k,
  sort,
  setSort,
  className = "",
  text = false,
}: {
  label: string;
  /** A text column: the first click sorts A→Z (numbers start with the largest). */
  text?: boolean;
  k: K;
  sort: { key: K; dir: 1 | -1 };
  setSort: (s: { key: K; dir: 1 | -1 }) => void;
  className?: string;
}) {
  const active = sort.key === k;
  return (
    <th className={className}>
      <button
        className={`inline-flex items-center gap-1 hover:text-content ${active ? "text-content" : ""}`}
        onClick={() => setSort({ key: k, dir: active ? ((-sort.dir) as 1 | -1) : text ? 1 : -1 })}
      >
        {label}
        {active && <span aria-hidden>{sort.dir === 1 ? "▲" : "▼"}</span>}
      </button>
    </th>
  );
}

// --- row actions --------------------------------------------------------------

export interface MenuAction {
  label: string;
  icon?: ReactNode;
  onSelect: () => void;
  /** Destructive: set apart below a divider, critical color on hover. */
  danger?: boolean;
  disabled?: boolean;
  title?: string;
}

/**
 * Row actions behind a "…" button. One or two actions stay as plain icon
 * buttons (a menu would only add a click); three or more collapse into the
 * menu. Falsy items are skipped, so callers can write `cond && { … }`.
 */
export function ActionMenu({ items, label = "Actions" }: { items: (MenuAction | false | null | undefined)[]; label?: string }) {
  // Fixed position, so the menu isn't clipped by the scrolling table around it.
  const [pos, setPos] = useState<CSSProperties | null>(null);
  const btn = useRef<HTMLButtonElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const shown = items.filter((i): i is MenuAction => !!i);
  const close = useCallback(() => setPos(null), []);
  const open = pos != null;

  useEffect(() => {
    if (!open) return;
    const enabled = () => Array.from(menu.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    menu.current?.focus();
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // Captured first, so Esc closes the menu and not the drawer behind it.
        e.stopPropagation();
        close();
        btn.current?.focus();
      } else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const els = enabled();
        const i = els.indexOf(document.activeElement as HTMLButtonElement);
        const next = e.key === "ArrowDown" ? i + 1 : i < 0 ? els.length - 1 : i - 1;
        els[(next + els.length) % els.length]?.focus();
      } else if (e.key === "Tab") {
        close();
      }
    };
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("scroll", close, true);
    window.addEventListener("resize", close);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("scroll", close, true);
      window.removeEventListener("resize", close);
    };
  }, [open, close]);

  const toggle = () => {
    if (open || !btn.current) return close();
    const r = btn.current.getBoundingClientRect();
    const height = shown.length * 32 + 20;
    const right = Math.max(8, window.innerWidth - r.right);
    const flip = r.bottom + height > window.innerHeight && r.top > height;
    setPos(flip ? { right, bottom: window.innerHeight - r.top + 4 } : { right, top: r.bottom + 4 });
  };

  if (shown.length < 3) {
    return (
      <>
        {shown.map((it) => (
          <button
            key={it.label}
            className={`btn-quiet ${it.danger ? "hover:!text-critical" : ""}`}
            disabled={it.disabled}
            title={it.title ?? it.label}
            aria-label={it.label}
            onClick={it.onSelect}
          >
            {it.icon ?? it.label}
          </button>
        ))}
      </>
    );
  }
  const firstDanger = shown.findIndex((i) => i.danger);
  return (
    <>
      <button
        ref={btn}
        className={`btn-quiet ${open ? "bg-muted !text-content" : ""}`}
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={toggle}
      >
        <MoreHorizontal size={16} />
      </button>
      {open && (
        <div ref={menu} role="menu" tabIndex={-1} style={{ ...pos, outline: "none" }} className="card fixed z-50 min-w-44 overflow-hidden py-1 text-left shadow-[var(--shadow-md)]">
          {shown.map((it, i) => (
            <Fragment key={it.label}>
              {i === firstDanger && i > 0 && <div className="my-1 border-t border-border-light" />}
              <button
                role="menuitem"
                disabled={it.disabled}
                title={it.title}
                className={`flex w-full items-center gap-2 whitespace-nowrap px-3 py-1.5 text-left text-sm text-content-secondary transition-colors hover:bg-muted focus:bg-muted focus:outline-none disabled:cursor-not-allowed disabled:opacity-40 ${it.danger ? "hover:text-critical focus:text-critical" : "hover:text-content focus:text-content"}`}
                onClick={() => {
                  close();
                  it.onSelect();
                }}
              >
                {it.icon && <span className="flex shrink-0 opacity-70">{it.icon}</span>}
                {it.label}
              </button>
            </Fragment>
          ))}
        </div>
      )}
    </>
  );
}

// --- overlays -----------------------------------------------------------------

// Open overlays in the order they opened. Esc goes to the last one only, so a
// dialog over a drawer closes by itself and leaves the drawer open.
const escStack: { current: (() => void) | null }[] = [];
let escListening = false;

function useEscape(onClose: () => void, enabled = true) {
  // `null` = takes Esc but does nothing with it (an overlay that only closes by its button).
  const handler = useRef<(() => void) | null>(null);
  handler.current = enabled ? onClose : null;
  useEffect(() => {
    if (!escListening) {
      escListening = true;
      window.addEventListener("keydown", (e) => e.key === "Escape" && escStack[escStack.length - 1]?.current?.());
    }
    escStack.push(handler);
    return () => void escStack.splice(escStack.indexOf(handler), 1);
  }, []);
}

export function Drawer({
  title,
  subtitle,
  onClose,
  children,
  actions,
  wide = false,
  explicitClose = false,
}: {
  title: ReactNode;
  subtitle?: ReactNode;
  onClose: () => void;
  children: ReactNode;
  actions?: ReactNode;
  /** Nearly the whole window, for content that needs the room (file browser). */
  wide?: boolean;
  /** Only the close button closes it: no Esc, no click outside. For work that's easy to lose. */
  explicitClose?: boolean;
}) {
  useEscape(onClose, !explicitClose);
  return (
    // Starts below the custom title bar, so the window can still be moved, minimized and closed.
    <div className={`fixed inset-x-0 bottom-0 z-40 flex justify-end ${isWindows ? "top-8" : "top-0"}`} onMouseDown={explicitClose ? undefined : onClose}>
      <div className="absolute inset-0 bg-black/30" />
      <aside
        className={`relative flex h-full flex-col ${wide ? "w-[min(1440px,96vw)]" : "w-[min(920px,92vw)]"} border-l border-border bg-surface shadow-[var(--shadow-md)]`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="flex items-start justify-between gap-3 border-b border-border-light px-5 py-3">
          <div className="min-w-0">
            <div className="truncate text-base font-semibold text-content">{title}</div>
            {subtitle && <div className="mt-0.5 truncate text-xs text-content-muted">{subtitle}</div>}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            {actions}
            <button className="btn-quiet" onClick={onClose} title={explicitClose ? "Close" : "Close (Esc)"}>
              <X size={16} />
            </button>
          </div>
        </header>
        <div className="min-h-0 flex-1 overflow-auto">{children}</div>
      </aside>
    </div>
  );
}

export function Modal({
  title,
  onClose,
  children,
  wide = false,
  explicitClose = false,
}: {
  title: string;
  onClose: () => void;
  children: ReactNode;
  wide?: boolean;
  /** Only its own buttons close it: no Esc, no click outside. For input or results that are easy to lose. */
  explicitClose?: boolean;
}) {
  useEscape(onClose, !explicitClose);
  return (
    <div
      className={`fixed inset-x-0 bottom-0 z-50 flex items-center justify-center bg-black/40 ${isWindows ? "top-8" : "top-0"}`}
      onMouseDown={explicitClose ? undefined : onClose}
    >
      <div
        className={`card max-h-[90vh] overflow-auto p-5 shadow-[var(--shadow-md)] ${wide ? "w-[min(640px,94vw)]" : "w-[380px]"}`}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <h2 className="mb-3 text-sm font-semibold text-content">{title}</h2>
        {children}
      </div>
    </div>
  );
}
