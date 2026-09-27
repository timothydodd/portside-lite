import { useEffect, useMemo, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { AlertTriangle, ArrowRightLeft, Copy, ExternalLink, Square } from "lucide-react";
import * as ipc from "../lib/ipc";
import { errorMessage, fmtAgo } from "../lib/format";
import type { ForwardInfo } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { Modal, Spinner } from "./ui";

/** Same rule as the backend's suggestion (80→8080, 443→8443, 22→10022). */
export function suggestLocalPort(servicePort: number): number {
  if (servicePort === 80) return 8080;
  if (servicePort === 443) return 8443;
  if (servicePort >= 1024) return servicePort;
  return 10000 + servicePort;
}

function forwardUrl(f: ForwardInfo, portName?: string | null): string {
  const tls = f.servicePort === 443 || f.targetPort === 443 || f.targetPort === 8443 || (portName ?? "").includes("https");
  return `${tls ? "https" : "http"}://localhost:${f.localPort}`;
}

async function open(url: string) {
  try {
    await openUrl(url);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

async function copy(text: string) {
  try {
    await navigator.clipboard.writeText(text);
    toast.info(`Copied ${text}`);
  } catch {
    toast.error("Couldn't copy to the clipboard");
  }
}

/** Dialog: pick a Service port and a local port, then start forwarding. */
export function ForwardDialog() {
  const { forward: target, closeForward } = useNavStore();
  const svc = useClusterStore((s) => s.snapshot?.services.find((x) => x.namespace === target?.namespace && x.name === target?.service));
  const [port, setPort] = useState<number | null>(null);
  const [local, setLocal] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [started, setStarted] = useState<ForwardInfo | null>(null);

  useEffect(() => {
    setPort(svc?.ports[0]?.port ?? null);
    setLocal("");
    setError(null);
    setStarted(null);
  }, [target]); // eslint-disable-line react-hooks/exhaustive-deps

  if (!target) return null;
  const portName = svc?.ports.find((p) => p.port === port)?.name;
  const localNum = local.trim() ? Number(local) : null;
  const localValid = localNum === null || (Number.isInteger(localNum) && localNum >= 1 && localNum <= 65535);

  const start = async () => {
    if (port == null) return;
    setBusy(true);
    setError(null);
    try {
      setStarted(await ipc.startPortForward(target.namespace, target.service, port, localNum));
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title={`Forward ${target.service}`} onClose={closeForward}>
      {started ? (
        <div className="flex flex-col gap-3 text-sm">
          <p className="text-content-secondary">
            Forwarding <span className="mono text-content">{started.service}:{started.servicePort}</span> →{" "}
            <span className="mono text-content">localhost:{started.localPort}</span>
            {started.pod && <span className="text-content-muted"> (via {started.pod})</span>}
          </p>
          <div className="flex gap-2">
            <button className="btn-primary" onClick={() => void open(forwardUrl(started, portName))}>
              <ExternalLink size={14} /> Open {forwardUrl(started, portName)}
            </button>
            <button className="btn-ghost" onClick={() => void copy(`localhost:${started.localPort}`)}>
              <Copy size={14} />
            </button>
          </div>
          <p className="text-xs text-content-muted">It keeps running until you stop it on the Services page (or quit the app).</p>
          <div className="flex justify-end">
            <button className="btn-ghost" onClick={closeForward}>
              Done
            </button>
          </div>
        </div>
      ) : (
        <div className="flex flex-col gap-3 text-sm">
          <label className="block">
            <span className="field-label">Service port</span>
            <select className="field w-full" value={port ?? ""} onChange={(e) => setPort(Number(e.target.value))}>
              {(svc?.ports ?? []).map((p) => (
                <option key={`${p.port}/${p.protocol}`} value={p.port} disabled={p.protocol !== "TCP"}>
                  {p.port}/{p.protocol}
                  {p.name ? ` (${p.name})` : ""}
                  {p.protocol !== "TCP" ? " — only TCP can be forwarded" : ""}
                </option>
              ))}
            </select>
          </label>
          <label className="block">
            <span className="field-label">Local port</span>
            <input
              className="field w-full"
              inputMode="numeric"
              value={local}
              placeholder={port != null ? `auto (${suggestLocalPort(port)}, or any free port)` : "auto"}
              onChange={(e) => setLocal(e.target.value.replace(/[^0-9]/g, ""))}
            />
            <span className="mt-1 block text-[11px] text-content-muted">Listens on 127.0.0.1 only: reachable from this computer, not your network.</span>
          </label>
          {error && (
            <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
              <AlertTriangle size={14} className="mt-px shrink-0" />
              <span className="break-words">{error}</span>
            </div>
          )}
          <div className="flex justify-end gap-2">
            <button className="btn-ghost" onClick={closeForward}>
              Cancel
            </button>
            <button className="btn-primary" disabled={busy || port == null || !localValid} onClick={() => void start()}>
              {busy ? <Spinner size={14} /> : <ArrowRightLeft size={14} />} Start forwarding
            </button>
          </div>
        </div>
      )}
    </Modal>
  );
}

/** Active forwards with open / copy / stop. Renders nothing when there are none. */
export function ForwardsPanel() {
  const forwards = useClusterStore((s) => s.forwards);
  const services = useClusterStore((s) => s.snapshot?.services);
  const [, tick] = useState(0);
  useEffect(() => {
    const t = setInterval(() => tick((n) => n + 1), 10_000);
    return () => clearInterval(t);
  }, []);
  const portNames = useMemo(() => {
    const m = new Map<string, string | null>();
    services?.forEach((s) => s.ports.forEach((p) => m.set(`${s.namespace}/${s.name}:${p.port}`, p.name)));
    return m;
  }, [services]);
  if (!forwards.length) return null;

  return (
    <section className="mx-6 mt-4 rounded-lg border border-accent/40 bg-surface">
      <h2 className="flex items-center gap-2 border-b border-border-light px-4 py-2 text-xs font-semibold text-content">
        <ArrowRightLeft size={14} className="text-accent" /> Port forwards ({forwards.length})
      </h2>
      <div className="divide-y divide-border-light">
        {forwards.map((f) => {
          const url = forwardUrl(f, portNames.get(`${f.namespace}/${f.service}:${f.servicePort}`));
          return (
            <div key={f.id} className="flex flex-wrap items-center gap-x-4 gap-y-1 px-4 py-2 text-xs">
              <span className="mono text-content">
                localhost:{f.localPort}
                <span className="text-content-muted"> → </span>
                {f.namespace}/{f.service}:{f.servicePort}
              </span>
              <span className="text-content-muted">
                {f.pod ? `via ${f.pod}${f.targetPort ? `:${f.targetPort}` : ""}` : "no pod yet"} · {f.activeConnections} open · {f.totalConnections} total ·
                started {fmtAgo(f.startedMs)}
                {f.clusterName && ` · ${f.clusterName}`}
              </span>
              {f.lastError && (
                <span className="flex min-w-0 items-center gap-1 text-warning" title={f.lastError}>
                  <AlertTriangle size={12} className="shrink-0" />
                  <span className="max-w-[320px] truncate">{f.lastError}</span>
                </span>
              )}
              <span className="ml-auto flex gap-1">
                <button className="btn-chip" onClick={() => void open(url)}>
                  <ExternalLink size={12} /> Open
                </button>
                <button className="btn-chip" title="Copy address" onClick={() => void copy(`localhost:${f.localPort}`)}>
                  <Copy size={12} />
                </button>
                <button className="btn-chip hover:!border-critical hover:!text-critical" onClick={() => void ipc.stopPortForward(f.id)}>
                  <Square size={12} /> Stop
                </button>
              </span>
            </div>
          );
        })}
      </div>
    </section>
  );
}
