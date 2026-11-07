import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { listen } from "@tauri-apps/api/event";
import { DatabaseBackup, FlaskConical, HardDriveUpload, Lock, RefreshCw, Server, ShieldAlert } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtAgo, fmtBytes } from "../lib/format";
import type { ClusterBackupEntry, ClusterBackups, ConnectionProfile, FileProgress, HostProbe, ImportResult, K3sInstallLog, VolumeArchive } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { toast } from "../stores/toast";
import { connectionSummary } from "./ClusterSwitcher";
import { ImportResults } from "./ImportDialog";
import K3sInstall from "./K3sInstall";
import { Modal, Spinner } from "./ui";

const key = (b: ClusterBackupEntry) => `${b.folder}/${b.stamp}`;
const newJobId = () => `rebuild-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;

/**
 * Rebuild a cluster from a backup on this PC onto any saved connection: the
 * node's own files first (SSH targets; k3s reads them when it's installed),
 * then the objects, Secrets and volume data.
 */
export default function RebuildDialog({ onClose }: { onClose: () => void }) {
  const settings = useClusterStore((s) => s.settings);
  const [list, setList] = useState<ClusterBackups | null>(null);
  const [picked, setPicked] = useState("");
  const [targetId, setTargetId] = useState(settings?.activeConnectionId ?? "");
  const [passphrase, setPassphrase] = useState("");

  useEffect(() => {
    ipc
      .clusterBackups()
      .then((l) => {
        setList(l);
        if (l.backups[0]) setPicked(key(l.backups[0]));
      })
      .catch((e) => toast.error(errorMessage(e)));
  }, []);

  const backup = list?.backups.find((b) => key(b) === picked) ?? null;
  const target = settings?.connections.find((p) => p.id === targetId) ?? null;
  const archives = backup ? (list?.volumes[backup.folder] ?? []) : [];
  const savedPassphrase = !!settings?.backupPassphrase;

  return (
    <Modal title="Rebuild a cluster from a backup" onClose={onClose} wide explicitClose>
      <div className="flex flex-col gap-4">
        {!list ? (
          <Spinner />
        ) : list.backups.length === 0 ? (
          <p className="text-sm text-content-secondary">
            No cluster backups in <span className="mono">{list.root}</span> yet. Turn on cluster backups under Settings → Backups to this PC, or point the
            backup folder at the folder holding them (for example, copied over from the old PC).
          </p>
        ) : (
          <>
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
              <label className="block">
                <span className="field-label">Backup</span>
                <select className="field w-full" value={picked} onChange={(e) => setPicked(e.target.value)}>
                  {list.backups.map((b) => (
                    <option key={key(b)} value={key(b)}>
                      {b.index?.cluster ?? b.folder} · {b.stamp}
                      {b.takenMs ? ` (${fmtAgo(b.takenMs)})` : ""}
                    </option>
                  ))}
                </select>
              </label>
              <label className="block">
                <span className="field-label">Rebuild onto</span>
                <select className="field w-full" value={targetId} onChange={(e) => setTargetId(e.target.value)}>
                  {settings?.connections.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name} — {connectionSummary(p.connection)}
                    </option>
                  ))}
                </select>
                <span className="mt-1 block text-[11px] text-content-muted">
                  A new machine: add it under Settings as an SSH connection first. Its node files go back before k3s is installed.
                </span>
              </label>
            </div>

            {backup && <BackupSummary backup={backup} archives={archives} />}

            {backup?.sealed && (
              <label className="block">
                <span className="field-label flex items-center gap-1">
                  <Lock size={12} /> Backup passphrase
                </span>
                <input
                  className="field w-full"
                  type="password"
                  autoComplete="off"
                  value={passphrase}
                  placeholder={savedPassphrase ? "The saved one (type another if this backup used a different one)" : "The passphrase this backup was made with"}
                  onChange={(e) => setPassphrase(e.target.value)}
                />
              </label>
            )}

            {backup && target && (
              <>
                <NodeStep backup={backup} target={target} passphrase={passphrase || null} />
                <ContentsStep backup={backup} target={target} passphrase={passphrase || null} archives={archives} />
              </>
            )}
          </>
        )}
        <div className="flex justify-end">
          <button className="btn-ghost" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </Modal>
  );
}

function BackupSummary({ backup, archives }: { backup: ClusterBackupEntry; archives: VolumeArchive[] }) {
  const i = backup.index;
  const items: string[] = [];
  if (i) {
    items.push(`${i.objects} objects`);
    if (i.customResources) items.push(`${i.customResources} custom resources`);
    items.push(i.sealed ? `${i.sealedSecrets} Secrets (sealed)` : "no Secret values");
    if (i.helmReleases) items.push(`${i.helmReleases} Helm releases`);
    if (i.node) items.push(`node files from ${i.node.hostname || "the node"}`);
    if (i.images.length) items.push(`${i.images.length} node-only images`);
    if (i.datastore) items.push(`${i.datastore} snapshot`);
  } else {
    items.push("manifests only (made by an older version)");
  }
  items.push(archives.length ? `${archives.length} volume archives on this PC` : "no volume archives on this PC");
  return (
    <div className="rounded-md bg-muted px-3 py-2 text-xs text-content-secondary">
      <div className="flex items-center gap-1.5 text-content">
        <DatabaseBackup size={13} /> {items.join(" · ")}
      </div>
      {i?.node?.k3sVersion && <div className="mt-1 text-content-muted">Was running {i.node.k3sVersion}{i.node.execStart && <> · <span className="mono">{i.node.execStart}</span></>}</div>}
      {i && i.warnings.length > 0 && <div className="mt-1 text-warning">Incomplete: {i.warnings.join(" ")}</div>}
    </div>
  );
}

/** Step 1 (SSH targets): node files and images, then k3s if it's missing. */
function NodeStep({ backup, target, passphrase }: { backup: ClusterBackupEntry; target: ConnectionProfile; passphrase: string | null }) {
  const [probe, setProbe] = useState<HostProbe | null>(null);
  const [probing, setProbing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [restart, setRestart] = useState(true);
  const [log, setLog] = useState("");
  const [error, setError] = useState<string | null>(null);
  const jobId = useRef<string | null>(null);
  const conn = target.connection;
  const node = backup.index?.node;
  const hasFiles = !!node && (node.files.length > 0 || node.sealedFiles.length > 0 || (backup.index?.images.length ?? 0) > 0);

  const check = async () => {
    if (conn.mode !== "ssh") return;
    setProbing(true);
    try {
      setProbe(await ipc.probeK3sHost(conn, target.id));
    } catch (e) {
      setProbe(null);
      setError(errorMessage(e));
    } finally {
      setProbing(false);
    }
  };
  useEffect(() => {
    setProbe(null);
    setError(null);
    setLog("");
    void check();
  }, [target.id]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const un = listen<K3sInstallLog>("k3s:install", (e) => {
      if (e.payload.jobId === jobId.current) setLog((l) => l + e.payload.text);
    });
    return () => void un.then((f) => f());
  }, []);

  if (conn.mode !== "ssh") {
    return (
      <Step n={1} title="The node" icon={<Server size={14} />}>
        <p className="text-xs text-content-muted">
          {target.name} is a local kubeconfig connection, so Portside can&apos;t reach its node. {hasFiles ? "The backup's node files (in its node/ folder) have to go back by hand." : ""}
        </p>
      </Step>
    );
  }

  const putBack = async () => {
    const ok = await confirmDestructive(
      `Write this backup's k3s files onto ${conn.host}?\n\nIts config, registries and custom manifests replace any already there${probe?.k3sRunning && restart ? ", and k3s is restarted to read them" : ""}.`,
      "Put node files back",
    );
    if (!ok) return;
    const id = newJobId();
    jobId.current = id;
    setBusy(true);
    setError(null);
    setLog("");
    try {
      const done = await ipc.restoreNodeFiles({ folder: backup.folder, stamp: backup.stamp, targetProfileId: target.id, passphrase, restart, jobId: id });
      toast.success(`${done.files.length} files and ${done.images.length} images put back on ${conn.host}`);
      await check();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      jobId.current = null;
      setBusy(false);
    }
  };

  return (
    <Step n={1} title={`The node: ${conn.host}`} icon={<Server size={14} />}>
      <div className="flex flex-wrap items-center gap-2 text-xs text-content-secondary">
        {probing ? (
          <>
            <Spinner size={12} /> Checking the server…
          </>
        ) : probe ? (
          <span>{probe.k3sInstalled ? `${probe.k3sVersion ?? "k3s"} is installed${probe.k3sRunning ? " and running" : ", not running"}.` : "k3s isn't installed yet."}</span>
        ) : null}
        <button className="btn-quiet !px-1 !py-0" title="Check again" aria-label="Check again" onClick={() => void check()} disabled={probing}>
          <RefreshCw size={12} />
        </button>
      </div>
      {hasFiles ? (
        <div className="flex flex-col gap-2">
          <p className="text-xs text-content-muted">
            {[...(node?.files ?? []), ...(node?.sealedFiles ?? [])].filter((f) => !f.endsWith("/token") && !f.includes("systemd") && !f.includes("init.d")).join(", ") || "No config files"}
            {(backup.index?.images.length ?? 0) > 0 && `; ${backup.index!.images.length} images into agent/images`}. Do this before installing k3s on a new machine.
          </p>
          <div className="flex items-center gap-3">
            <button className="btn-ghost" onClick={() => void putBack()} disabled={busy}>
              {busy ? <Spinner size={14} /> : <HardDriveUpload size={14} />} Put node files back
            </button>
            {probe?.k3sRunning && (
              <label className="flex items-center gap-1.5 text-xs text-content-secondary">
                <input type="checkbox" className="accent-brand" checked={restart} onChange={(e) => setRestart(e.target.checked)} /> Restart k3s afterwards
              </label>
            )}
          </div>
        </div>
      ) : (
        <p className="text-xs text-content-muted">This backup has no node files (it was made through a local connection, or by an older version).</p>
      )}
      {log && <pre className="mono max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-muted p-2 text-[11px] text-content-secondary">{log}</pre>}
      {error && <ErrorBox text={error} />}
      {probe && !probe.k3sInstalled && (
        <K3sInstall
          connection={conn}
          profileId={target.id}
          probe={probe}
          onInstalled={(after) => {
            setProbe(after);
            toast.success(`${after.k3sVersion ?? "k3s"} installed on ${conn.host}`);
          }}
        />
      )}
    </Step>
  );
}

/** Step 2: objects (CRDs first), Secrets, volume data, then the workloads. */
function ContentsStep({ backup, target, passphrase, archives }: { backup: ClusterBackupEntry; target: ConnectionProfile; passphrase: string | null; archives: VolumeArchive[] }) {
  const [volumes, setVolumes] = useState(archives.length > 0);
  const [busy, setBusy] = useState<"dry" | "run" | null>(null);
  const [results, setResults] = useState<{ dryRun: boolean; rows: ImportResult[]; warnings: string[] } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<FileProgress | null>(null);
  const jobId = useRef<string | null>(null);
  const totalBytes = useMemo(() => archives.reduce((n, a) => n + a.bytes, 0), [archives]);

  useEffect(() => {
    setResults(null);
    setError(null);
    setVolumes(archives.length > 0);
  }, [backup.folder, backup.stamp, target.id]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const un = listen<FileProgress>("files:progress", (e) => {
      if (e.payload.transferId === jobId.current) setProgress(e.payload);
    });
    return () => void un.then((f) => f());
  }, []);

  const run = async (dryRun: boolean) => {
    if (!dryRun) {
      const ok = await confirmDestructive(
        `Rebuild onto ${target.name}?\n\nEvery object in the backup is created or overwritten there${volumes ? ", and each backed-up volume's newest archive is unpacked into its claim (files with the same names are replaced)" : ""}. Objects already there that aren't in the backup are left alone.`,
        "Rebuild",
      );
      if (!ok) return;
    }
    const id = newJobId();
    jobId.current = id;
    setBusy(dryRun ? "dry" : "run");
    setError(null);
    setProgress(null);
    try {
      const out = await ipc.rebuildCluster({ folder: backup.folder, stamp: backup.stamp, targetProfileId: target.id, passphrase, volumes, dryRun, jobId: id });
      setResults({ dryRun, rows: out.results, warnings: out.warnings });
      const failed = out.results.filter((r) => r.outcome === "error").length;
      if (!dryRun) (failed ? toast.error : toast.success)(failed ? `Rebuilt with ${failed} problems; see the list` : `Rebuilt onto ${target.name}`);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      jobId.current = null;
      setBusy(null);
      setProgress(null);
    }
  };

  return (
    <Step n={2} title={`Cluster contents onto ${target.name}`} icon={<DatabaseBackup size={14} />}>
      <p className="text-xs text-content-muted">
        CRDs first, then namespaces, accounts, config and Secrets, claims, Services and custom resources; then the volume data; the workloads last, so
        nothing has a claim mounted while it&apos;s written. What k3s installs itself (CoreDNS, Traefik through its HelmChart, …) is left to it.
      </p>
      <label className="flex items-start gap-2 text-sm text-content">
        <input type="checkbox" className="accent-brand mt-0.5" checked={volumes} disabled={archives.length === 0} onChange={(e) => setVolumes(e.target.checked)} />
        <span>
          Restore volume data from this PC
          <span className="block text-xs text-content-muted">
            {archives.length
              ? `The newest archive of each volume backup: ${archives.length} (${fmtBytes(totalBytes)}).`
              : "No volume archives on this PC for this cluster (turn on copying volume backups to this PC)."}
          </span>
        </span>
      </label>
      {busy === "run" && progress && (
        <div className="text-xs text-content-secondary">
          {progress.label}: {fmtBytes(progress.done)}
          {progress.total ? ` of ${fmtBytes(progress.total)}` : ""}
        </div>
      )}
      {error && <ErrorBox text={error} />}
      {results && results.warnings.length > 0 && (
        <div className="tint-warning rounded-md px-3 py-2 text-xs">{results.warnings.join(" ")}</div>
      )}
      {results && <ImportResults dryRun={results.dryRun} rows={results.rows} tall />}
      <div className="flex justify-end gap-2">
        <button className="btn-ghost" disabled={!!busy} onClick={() => void run(true)}>
          {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
        </button>
        <button className="btn-primary" disabled={!!busy} onClick={() => void run(false)}>
          {busy === "run" ? <Spinner size={14} /> : null} Rebuild
        </button>
      </div>
    </Step>
  );
}

function Step({ n, title, icon, children }: { n: number; title: string; icon: ReactNode; children: ReactNode }) {
  return (
    <section className="card flex flex-col gap-3 p-3">
      <h3 className="flex items-center gap-2 text-sm font-medium text-content">
        <span className="flex h-5 w-5 items-center justify-center rounded-full bg-muted text-[11px] text-content-secondary">{n}</span>
        {icon}
        {title}
      </h3>
      {children}
    </section>
  );
}

function ErrorBox({ text }: { text: string }) {
  return (
    <div className="tint-critical flex items-start gap-2 rounded-md px-3 py-2 text-xs">
      <ShieldAlert size={14} className="mt-px shrink-0" />
      <span className="mono whitespace-pre-wrap break-words">{text}</span>
    </div>
  );
}
