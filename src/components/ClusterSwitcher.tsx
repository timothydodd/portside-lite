import { useEffect, useRef, useState } from "react";
import { Check, ChevronsUpDown, Laptop, Plus, Settings2, Terminal } from "lucide-react";
import { errorMessage } from "../lib/format";
import type { Connection } from "../lib/types";
import { activeProfile, useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";

export function ModeIcon({ connection, size = 14, className = "" }: { connection: Connection; size?: number; className?: string }) {
  const Icon = connection.mode === "ssh" ? Terminal : Laptop;
  return <Icon size={size} className={`shrink-0 ${className}`} aria-label={connection.mode === "ssh" ? "SSH" : "Local"} />;
}

export function connectionSummary(c: Connection): string {
  if (c.mode === "local") return c.context || "current context";
  if (!c.host) return "SSH — not set up yet";
  return `${c.username ? `${c.username}@` : ""}${c.host}${c.port !== 22 ? `:${c.port}` : ""}`;
}

/** Sidebar dropdown for switching the monitored cluster. */
export default function ClusterSwitcher() {
  const settings = useClusterStore((s) => s.settings);
  const status = useClusterStore((s) => s.status);
  const switchConnection = useClusterStore((s) => s.switchConnection);
  const go = useNavStore((s) => s.go);
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => !ref.current?.contains(e.target as Node) && setOpen(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const active = activeProfile(settings);
  const dot =
    status?.state === "connected" ? "bg-good" : status?.state === "error" ? "bg-critical" : status?.state === "connecting" ? "bg-warning animate-pulse" : "bg-content-muted";

  const manage = () => {
    setOpen(false);
    go("settings");
  };

  return (
    <div ref={ref} className="relative mb-3">
      <button
        onClick={() => (settings?.connections.length ? setOpen((o) => !o) : manage())}
        className="flex w-full items-center gap-2 rounded-md border border-border-light bg-raised px-2.5 py-2 text-left transition-colors hover:border-accent"
        title="Switch cluster"
      >
        <span className={`h-2 w-2 shrink-0 rounded-full ${active ? dot : "bg-content-muted"}`} />
        <span className="min-w-0 flex-1">
          <span className="block truncate text-sm font-medium text-content">{active?.name ?? "Add a cluster"}</span>
          {active && <span className="block truncate text-[11px] text-content-muted">{connectionSummary(active.connection)}</span>}
        </span>
        {settings?.connections.length ? <ChevronsUpDown size={14} className="shrink-0 text-content-muted" /> : <Plus size={14} className="shrink-0 text-content-muted" />}
      </button>

      {open && settings && (
        <div className="card absolute left-0 right-0 top-full z-30 mt-1 overflow-hidden py-1 shadow-[var(--shadow-md)]">
          {settings.connections.map((p) => {
            const isActive = p.id === settings.activeConnectionId;
            return (
              <button
                key={p.id}
                className="flex w-full items-center gap-2 px-2.5 py-1.5 text-left hover:bg-muted"
                onClick={async () => {
                  setOpen(false);
                  try {
                    await switchConnection(p.id);
                  } catch (e) {
                    toast.error(errorMessage(e));
                  }
                }}
              >
                <ModeIcon connection={p.connection} className="text-content-muted" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm text-content">{p.name}</span>
                  <span className="block truncate text-[11px] text-content-muted">{connectionSummary(p.connection)}</span>
                </span>
                {isActive && <Check size={14} className="shrink-0 text-accent" />}
              </button>
            );
          })}
          <div className="my-1 border-t border-border-light" />
          <button className="flex w-full items-center gap-2 whitespace-nowrap px-2.5 py-1.5 text-left text-sm text-content-secondary hover:bg-muted hover:text-content" onClick={manage}>
            <Settings2 size={14} className="text-content-muted" /> Manage connections…
          </button>
        </div>
      )}
    </div>
  );
}
