import { useEffect, useState } from "react";
import { RefreshCw } from "lucide-react";
import { refreshNow } from "../lib/ipc";
import { fmtAgo } from "../lib/format";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";

/** Bottom bar: connection state, cluster, poll + log sync freshness. */
export default function StatusBar() {
  const status = useClusterStore((s) => s.status);
  // The saved setting is the source of truth; the status event may lag it.
  const paused = useClusterStore((s) => s.settings?.monitoringPaused) || status?.paused;
  const go = useNavStore((s) => s.go);
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 5000);
    return () => clearInterval(t);
  }, []);

  const state = paused ? "paused" : (status?.state ?? "connecting");
  const dot = {
    connected: "bg-good",
    connecting: "bg-warning animate-pulse",
    error: "bg-critical",
    unconfigured: "bg-content-muted",
    paused: "bg-warning",
  }[state];
  const label = {
    connected: "Connected",
    connecting: "Connecting…",
    error: "Connection error",
    unconfigured: "Not configured",
    paused: "Monitoring paused",
  }[state];

  return (
    <footer className="flex h-7 shrink-0 items-center gap-4 border-t border-border-light bg-surface px-3 text-[11px] text-content-muted">
      <button className="flex items-center gap-1.5 hover:text-content" onClick={() => go("settings")} title={status?.message ?? undefined}>
        <span className={`h-2 w-2 rounded-full ${dot}`} />
        <span className="text-content-secondary">{label}</span>
        {status?.clusterId && <span className="mono">{status.clusterId}</span>}
      </button>
      {status?.serverVersion && <span>k8s {status.serverVersion}</span>}
      {status?.message && <span className="truncate text-warning">{status.message}</span>}
      <span className="ml-auto">
        Polled {fmtAgo(status?.lastPollMs)}
        {status?.lastPollDurationMs != null && ` (${status.lastPollDurationMs} ms)`}
      </span>
      <span>
        Logs synced {fmtAgo(status?.lastLogSyncMs)}
        {status?.lastLogSyncMs != null && ` · +${status.lastLogSyncLines.toLocaleString()} lines`}
        {status?.logSyncErrors ? ` · ${status.logSyncErrors} failed` : ""}
      </span>
      <button className="flex items-center gap-1 hover:text-content" onClick={() => void refreshNow()} title="Poll now">
        <RefreshCw size={12} />
      </button>
    </footer>
  );
}
