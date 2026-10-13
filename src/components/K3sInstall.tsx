import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { AlertTriangle, Download, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage } from "../lib/format";
import type { Connection, HostProbe, K3sInstallLog, K3sInstallOptions } from "../lib/types";
import { Spinner } from "./ui";

const DEFAULTS: K3sInstallOptions = {
  updateSystem: true,
  channel: "stable",
  version: null,
  kubeconfigReadable: true,
  traefik: true,
  servicelb: true,
  localStorage: true,
  metricsServer: true,
  secretsEncryption: false,
  clusterInit: false,
  tlsSan: null,
  nodeName: null,
};

type Flag = Exclude<{ [K in keyof K3sInstallOptions]: K3sInstallOptions[K] extends boolean ? K : never }[keyof K3sInstallOptions], undefined>;

/** Each switch, what it does on the server, and when to turn it the other way. */
const FLAGS: { key: Flag; label: string; description: string; flag: string }[] = [
  {
    key: "updateSystem",
    label: "Update the system first",
    description:
      "Brings the OS packages up to date before installing (apt-get update && upgrade, or the equivalent for dnf, yum, zypper and apk). Takes a few minutes; a kernel update may need a reboot later.",
    flag: "",
  },
  {
    key: "kubeconfigReadable",
    label: "Kubeconfig readable without sudo",
    description:
      "Lets Portside and kubectl on the server read the cluster credentials without a sudo password. Anyone who can log in to the server then has admin access to the cluster. Off, Portside needs the sudo password above.",
    flag: "--write-kubeconfig-mode 644",
  },
  {
    key: "traefik",
    label: "Traefik ingress controller",
    description:
      "Routes HTTP and HTTPS traffic from outside the cluster to your Services through Ingress objects. Turn off if you'll install another ingress controller (nginx, HAProxy…).",
    flag: "--disable traefik when off",
  },
  {
    key: "servicelb",
    label: "ServiceLB load balancer",
    description:
      "Gives LoadBalancer Services an address on the node's own IP, so they work on a single box with nothing else. Turn off if you'll use MetalLB or a cloud load balancer.",
    flag: "--disable servicelb when off",
  },
  {
    key: "localStorage",
    label: "Local-path storage",
    description:
      "The default StorageClass: volumes live on the node's disk under /var/lib/rancher/k3s/storage. Needed for PersistentVolumeClaims unless you bring other storage (NFS, Longhorn…).",
    flag: "--disable local-storage when off",
  },
  {
    key: "metricsServer",
    label: "Metrics server",
    description: "CPU and memory usage per node and pod. Portside's usage charts and autoscalers need it.",
    flag: "--disable metrics-server when off",
  },
  {
    key: "secretsEncryption",
    label: "Encrypt Secrets at rest",
    description: "Encrypts Secret objects in k3s's datastore on disk. Recommended on a shared or exposed server; a little CPU per Secret read.",
    flag: "--secrets-encryption",
  },
  {
    key: "clusterInit",
    label: "Embedded etcd (for more servers later)",
    description:
      "Uses etcd instead of the default SQLite datastore, so more server nodes can join for high availability. More disk writes and memory; only worth it if you plan a multi-server cluster.",
    flag: "--cluster-init",
  },
];

function commandPreview(o: K3sInstallOptions) {
  const env = o.version?.trim() ? `INSTALL_K3S_VERSION=${o.version.trim()}` : `INSTALL_K3S_CHANNEL=${o.channel}`;
  const args = ["server"];
  if (o.kubeconfigReadable) args.push("--write-kubeconfig-mode 644");
  if (!o.traefik) args.push("--disable traefik");
  if (!o.servicelb) args.push("--disable servicelb");
  if (!o.localStorage) args.push("--disable local-storage");
  if (!o.metricsServer) args.push("--disable metrics-server");
  if (o.secretsEncryption) args.push("--secrets-encryption");
  if (o.clusterInit) args.push("--cluster-init");
  if (o.tlsSan?.trim()) args.push(`--tls-san ${o.tlsSan.trim()}`);
  if (o.nodeName?.trim()) args.push(`--node-name ${o.nodeName.trim()}`);
  return `curl -sfL https://get.k3s.io | ${env} sh -s - ${args.join(" ")}`;
}

/** Offer to install k3s on an SSH host that doesn't have it, with the installer's output live. */
export default function K3sInstall({
  connection,
  profileId,
  probe,
  onInstalled,
}: {
  connection: Connection;
  profileId: string | null;
  probe: HostProbe;
  onInstalled: (probe: HostProbe) => void;
}) {
  const [opts, setOpts] = useState<K3sInstallOptions>(DEFAULTS);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [log, setLog] = useState("");
  const [error, setError] = useState<string | null>(null);
  const jobId = useRef<string | null>(null);
  const logRef = useRef<HTMLPreElement>(null);

  useEffect(() => {
    const un = listen<K3sInstallLog>("k3s:install", (e) => {
      if (e.payload.jobId === jobId.current) setLog((l) => l + e.payload.text);
    });
    return () => void un.then((f) => f());
  }, []);
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [log]);

  const preview = useMemo(() => commandPreview(opts), [opts]);
  const host = connection.mode === "ssh" ? connection.host : "";
  const blocker = !probe.sudoOk
    ? "sudo doesn't work for this login (no passwordless sudo, and no working sudo password above). The installer needs root."
    : !probe.hasSystemd
      ? "The server has no systemd, which k3s needs to run as a service."
      : !probe.hasCurl && !probe.packageManager
        ? "curl isn't installed and there's no known package manager to install it with."
        : null;

  const install = async () => {
    const ok = await confirmDestructive(
      `Install k3s on ${host}?\n\n${opts.updateSystem ? "The system packages are updated first, then " : "Then "}the official installer runs with the options chosen. This changes the server; it can't be undone from here (k3s ships /usr/local/bin/k3s-uninstall.sh).`,
      "Install k3s",
    );
    if (!ok) return;
    const id = crypto.randomUUID();
    jobId.current = id;
    setBusy(true);
    setLog("");
    setError(null);
    try {
      const after = await ipc.installK3s(connection, profileId, opts, id);
      setLog((l) => `${l}\nDone. ${after.k3sVersion ?? "k3s"} is running.\n`);
      onInstalled(after);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      jobId.current = null;
      setBusy(false);
    }
  };

  const toggle = (key: Flag) => setOpts((o) => ({ ...o, [key]: !o[key] }));

  return (
    <div className="card flex flex-col gap-3 border p-3 text-sm">
      <div className="flex items-start gap-2">
        <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" />
        <div className="min-w-0">
          <div className="font-medium text-content">k3s isn't installed on {host}</div>
          <div className="text-xs text-content-muted">
            {probe.os}
            {probe.arch ? ` · ${probe.arch}` : ""}
            {probe.packageManager ? ` · ${probe.packageManager}` : " · no known package manager"}
            {probe.sudoOk ? " · sudo works" : " · sudo not available"}. Portside can install it over this SSH connection.
          </div>
        </div>
        {!open && (
          <button className="btn-primary ml-auto shrink-0" onClick={() => setOpen(true)} disabled={!!blocker}>
            <Download size={14} /> Install k3s…
          </button>
        )}
      </div>
      {blocker && <p className="text-xs text-critical">{blocker}</p>}

      {open && (
        <>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
            {FLAGS.map((f) => (
              <label key={f.key} className="flex cursor-pointer items-start gap-2 rounded-md bg-muted p-2">
                <input type="checkbox" className="accent-brand mt-0.5" checked={opts[f.key]} disabled={busy} onChange={() => toggle(f.key)} />
                <span className="min-w-0">
                  <span className="block text-xs font-medium text-content">
                    {f.label}
                    {f.flag && <span className="mono ml-1 text-[10px] text-content-muted">{f.flag}</span>}
                  </span>
                  <span className="block text-[11px] text-content-muted">{f.description}</span>
                </span>
              </label>
            ))}
          </div>

          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <label className="block">
              <span className="field-label">Release</span>
              <select className="field w-full" value={opts.channel} disabled={busy} onChange={(e) => setOpts((o) => ({ ...o, channel: e.target.value as "stable" | "latest" }))}>
                <option value="stable">stable (recommended)</option>
                <option value="latest">latest</option>
              </select>
              <span className="mt-1 block text-[11px] text-content-muted">Or pin a version below, e.g. v1.31.4+k3s1.</span>
            </label>
            <label className="block">
              <span className="field-label">Exact version</span>
              <input className="field mono w-full" placeholder="(channel's newest)" value={opts.version ?? ""} disabled={busy} onChange={(e) => setOpts((o) => ({ ...o, version: e.target.value || null }))} />
            </label>
            <label className="block">
              <span className="field-label">Extra TLS name</span>
              <input className="field mono w-full" placeholder="k3s.example.com" value={opts.tlsSan ?? ""} disabled={busy} onChange={(e) => setOpts((o) => ({ ...o, tlsSan: e.target.value || null }))} />
              <span className="mt-1 block text-[11px] text-content-muted">
                A hostname or IP the API certificate should also be valid for, if you'll reach the API by it. Portside's SSH tunnel doesn't need one.
              </span>
            </label>
          </div>

          <div>
            <span className="field-label">Will run on the server (with sudo)</span>
            <pre className="mono overflow-x-auto rounded-md bg-muted p-2 text-[11px] text-content-secondary">{preview}</pre>
          </div>

          {(log || busy) && (
            <pre ref={logRef} className="mono max-h-64 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-2 text-[11px] text-content-secondary">
              {log || "Connecting…"}
            </pre>
          )}
          {error && (
            <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
              <ShieldAlert size={14} className="mt-px shrink-0" />
              <span className="mono whitespace-pre-wrap break-words">{error}</span>
            </div>
          )}

          <div className="flex items-center justify-end gap-2">
            {busy && <span className="mr-auto text-xs text-content-muted">Installing; this can take several minutes. Closing Portside doesn't stop the server-side steps already running.</span>}
            <button className="btn-ghost" onClick={() => setOpen(false)} disabled={busy}>
              Cancel
            </button>
            <button className="btn-primary" onClick={() => void install()} disabled={busy || !!blocker}>
              {busy ? <Spinner size={14} /> : <Download size={14} />} Install k3s
            </button>
          </div>
        </>
      )}
    </div>
  );
}
