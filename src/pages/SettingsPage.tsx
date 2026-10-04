import { useEffect, useRef, useState, type ReactNode } from "react";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { Bell, CheckCircle2, FolderOpen, KeyRound, Laptop, Plus, PlugZap, RefreshCw, ShieldAlert, Terminal, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtAgo, fmtBytes, fmtCount } from "../lib/format";
import type { Connection, ConnectionProfile, Settings, SshConnection, StorageStats } from "../lib/types";
import { ModeIcon, connectionSummary } from "../components/ClusterSwitcher";
import { PageHeader, Spinner } from "../components/ui";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { useThemeStore, type ThemePref } from "../stores/theme";
import { toast } from "../stores/toast";

const DEFAULT_SSH: SshConnection = {
  host: "",
  port: 22,
  username: "",
  auth: { kind: "key", privateKeyPath: "~/.ssh/id_ed25519", passphrase: null },
  kubeconfigCommand: "sudo -n cat /etc/rancher/k3s/k3s.yaml",
  apiHost: "127.0.0.1",
  apiPort: 6443,
  hostKeyFingerprint: null,
  sudoPassword: null,
};

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);

/**
 * Carry edits over to settings that changed underneath them (paused from the
 * tray, a host key pinned by a background check): whatever the user hasn't
 * touched follows `next`, what they have stays.
 */
function rebase(draft: Settings, base: Settings, next: Settings): Settings {
  const out = { ...next } as Record<string, unknown>;
  for (const k of Object.keys(draft) as (keyof Settings)[]) {
    if (!same(draft[k], base[k])) out[k] = draft[k];
  }
  const merged = out as unknown as Settings;
  if (!same(draft.connections, base.connections)) {
    // Fields only the backend sets come from `next` for profiles that still exist.
    merged.connections = draft.connections.map((p) => {
      const theirs = next.connections.find((x) => x.id === p.id)?.connection;
      const ours = base.connections.find((x) => x.id === p.id)?.connection;
      if (!theirs || !ours || theirs.mode !== p.connection.mode) return p;
      const c = { ...p.connection, partition: theirs.partition };
      if (c.mode === "ssh" && theirs.mode === "ssh" && ours.mode === "ssh" && c.hostKeyFingerprint === ours.hostKeyFingerprint) {
        c.hostKeyFingerprint = theirs.hostKeyFingerprint;
      }
      return { ...p, connection: c };
    });
  }
  return merged;
}

export default function SettingsPage() {
  const saved = useClusterStore((s) => s.settings);
  const saveSettings = useClusterStore((s) => s.saveSettings);
  const setLeaveGuard = useNavStore((s) => s.setLeaveGuard);
  const [draft, setDraft] = useState<Settings | null>(saved);
  const [saving, setSaving] = useState(false);

  // Follow backend-side changes (e.g. a newly pinned host key) without losing edits in progress.
  const base = useRef(saved);
  useEffect(() => {
    const was = base.current;
    base.current = saved;
    setDraft((d) => (d && was && saved ? rebase(d, was, saved) : saved));
  }, [saved]);

  const dirty = !!draft && !same(draft, saved);
  useEffect(() => {
    setLeaveGuard(dirty ? () => confirmDestructive("You have unsaved settings.\n\nLeave without saving them?", "Discard changes") : null);
    return () => setLeaveGuard(null);
  }, [dirty, setLeaveGuard]);
  if (!draft) return <Spinner />;

  const set = (patch: Partial<Settings>) => setDraft({ ...draft, ...patch });

  const save = async () => {
    setSaving(true);
    try {
      await saveSettings(draft);
      toast.success("Settings saved");
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="pb-10">
      <PageHeader title="Settings">
        {dirty && (
          <button className="btn-ghost" onClick={() => setDraft(saved)}>
            Discard
          </button>
        )}
        <button className="btn-primary" disabled={!dirty || saving} onClick={() => void save()}>
          {saving ? <Spinner size={14} /> : null} Save
        </button>
      </PageHeader>
      <div className="flex max-w-4xl flex-col gap-6 p-6">
        <ConnectionsSection draft={draft} set={set} />
        <MonitoringSection draft={draft} set={set} />
        <BackgroundSection draft={draft} set={set} />
        <DataSection />
        <ArchivesSection draft={draft} set={set} saved={saved} />
        <VolumeFilesSection draft={draft} set={set} />
        <AppearanceSection />
      </div>
    </div>
  );
}

function Section({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section className="card p-5">
      <h2 className="text-sm font-semibold text-content">{title}</h2>
      {description && <p className="mb-4 mt-0.5 text-xs text-content-muted">{description}</p>}
      <div className="flex flex-col gap-4">{children}</div>
    </section>
  );
}

function Labeled({ label, hint, children, className = "" }: { label: string; hint?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <label className={`block ${className}`}>
      <span className="field-label">{label}</span>
      {children}
      {hint && <span className="mt-1 block text-[11px] text-content-muted">{hint}</span>}
    </label>
  );
}

function newId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `c-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

function ConnectionsSection({ draft, set }: { draft: Settings; set: (p: Partial<Settings>) => void }) {
  const [selectedId, setSelectedId] = useState<string | null>(draft.activeConnectionId ?? draft.connections[0]?.id ?? null);
  const selected = draft.connections.find((p) => p.id === selectedId) ?? draft.connections[0] ?? null;

  const add = () => {
    const profile: ConnectionProfile = {
      id: newId(),
      name: `Cluster ${draft.connections.length + 1}`,
      connection: { mode: "local", kubeconfigPath: null, context: null },
    };
    set({ connections: [...draft.connections, profile], activeConnectionId: draft.activeConnectionId ?? profile.id });
    setSelectedId(profile.id);
  };

  const update = (profile: ConnectionProfile) =>
    set({ connections: draft.connections.map((p) => (p.id === profile.id ? profile : p)) });

  const remove = async (profile: ConnectionProfile) => {
    const ok = await confirmDestructive(
      `Remove the "${profile.name}" connection?\n\nIts stored logs and history stay in the local database until pruned or cleared.`,
      "Remove connection",
    );
    if (!ok) return;
    const rest = draft.connections.filter((p) => p.id !== profile.id);
    set({
      connections: rest,
      activeConnectionId: draft.activeConnectionId === profile.id ? (rest[0]?.id ?? null) : draft.activeConnectionId,
    });
    setSelectedId(rest[0]?.id ?? null);
  };

  return (
    <Section
      title="Cluster connections"
      description="Save as many clusters as you like, local or over SSH. Only the active one is monitored; each keeps its own logs and history."
    >
      <div className="grid gap-4 md:grid-cols-[220px_1fr]">
        <div className="flex flex-col gap-1">
          {draft.connections.map((p) => {
            const isSel = p.id === selected?.id;
            const isActive = p.id === draft.activeConnectionId;
            return (
              <button
                key={p.id}
                onClick={() => setSelectedId(p.id)}
                className={`flex items-center gap-2 rounded-md border px-2.5 py-2 text-left transition-colors ${
                  isSel ? "border-accent bg-muted" : "border-transparent hover:bg-muted"
                }`}
              >
                <ModeIcon connection={p.connection} className="text-content-muted" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-sm text-content">{p.name || "Untitled"}</span>
                  <span className="block truncate text-[11px] text-content-muted">{connectionSummary(p.connection)}</span>
                </span>
                {isActive && <span className="tint-good shrink-0 rounded px-1.5 text-[10px] font-semibold">Active</span>}
              </button>
            );
          })}
          <button className="btn-ghost mt-1 justify-start" onClick={add}>
            <Plus size={14} /> Add connection
          </button>
        </div>

        {selected ? (
          <div key={selected.id} className="flex min-w-0 flex-col gap-4">
            <div className="flex flex-wrap items-end gap-3">
              <Labeled label="Name" className="min-w-[200px] flex-1">
                <input className="field w-full" value={selected.name} onChange={(e) => update({ ...selected, name: e.target.value })} placeholder="e.g. Homelab" />
              </Labeled>
              {selected.id === draft.activeConnectionId ? (
                <span className="tint-good inline-flex items-center gap-1 rounded-md px-2.5 py-1.5 text-xs font-semibold">
                  <CheckCircle2 size={14} /> Active
                </span>
              ) : (
                <button className="btn-ghost" onClick={() => set({ activeConnectionId: selected.id })} title="Monitor this cluster (applies when you save)">
                  Use this cluster
                </button>
              )}
              <button className="btn-quiet hover:!text-critical" onClick={() => void remove(selected)} title="Remove connection">
                <Trash2 size={15} />
              </button>
            </div>
            <ConnectionEditor connection={selected.connection} onChange={(connection) => update({ ...selected, connection })} />
          </div>
        ) : (
          <div className="flex flex-col items-start justify-center gap-2 rounded-md bg-muted p-5 text-sm text-content-secondary">
            No clusters yet.
            <button className="btn-primary" onClick={add}>
              <Plus size={14} /> Add your first cluster
            </button>
          </div>
        )}
      </div>
    </Section>
  );
}

function ConnectionEditor({ connection, onChange }: { connection: Connection; onChange: (c: Connection) => void }) {
  const mode = connection.mode;
  const [testing, setTesting] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const test = async () => {
    setTesting(true);
    setResult(null);
    try {
      const r = await ipc.testConnection(connection);
      setResult({ ok: true, text: `Connected — Kubernetes ${r.serverVersion}${r.hostKeyFingerprint ? ` · host key ${r.hostKeyFingerprint}` : ""}` });
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setTesting(false);
    }
  };

  const modeButton = (m: "local" | "ssh", Icon: typeof Laptop, title: string, desc: string) => (
    <button
      className={`card flex flex-1 items-start gap-3 border p-3 text-left transition-colors ${mode === m ? "border-accent" : "hover:border-border"}`}
      onClick={() => {
        if (m === mode) return;
        setResult(null);
        if (m === "local") onChange({ mode: "local", kubeconfigPath: null, context: null });
        else onChange({ mode: "ssh", ...DEFAULT_SSH });
      }}
    >
      <Icon size={18} className={mode === m ? "text-accent" : "text-content-muted"} />
      <span>
        <span className="block text-sm font-medium text-content">{title}</span>
        <span className="block text-xs text-content-muted">{desc}</span>
      </span>
    </button>
  );

  return (
    <>
      <div className="flex gap-3">
        {modeButton("local", Laptop, "Local kubeconfig", "Use ~/.kube/config (or a file you pick) on this machine.")}
        {modeButton("ssh", Terminal, "SSH to k3s server", "Read the node's k3s.yaml and tunnel the API over SSH.")}
      </div>

      {connection.mode === "local" && <LocalForm c={connection} onChange={onChange} />}
      {connection.mode === "ssh" && <SshForm c={connection} onChange={onChange} />}

      <div className="flex flex-wrap items-center gap-3">
        <button className="btn-ghost" onClick={() => void test()} disabled={testing}>
          {testing ? <Spinner size={14} /> : <PlugZap size={14} />} Test connection
        </button>
        {result && (
          <span className={`flex min-w-0 flex-1 items-start gap-1.5 text-xs ${result.ok ? "text-content-secondary" : "text-critical"}`}>
            {result.ok ? <CheckCircle2 size={14} className="shrink-0 text-good" /> : <ShieldAlert size={14} className="shrink-0" />}
            <span className="mono break-all">{result.text}</span>
          </span>
        )}
      </div>
    </>
  );
}

function LocalForm({ c, onChange }: { c: Extract<Connection, { mode: "local" }>; onChange: (c: Connection) => void }) {
  const [contexts, setContexts] = useState<{ contexts: string[]; current: string | null } | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true; // the path changes with every keystroke; only the last answer counts
    ipc
      .listKubeContexts(c.kubeconfigPath)
      .then((r) => {
        if (!current) return;
        setContexts(r);
        setError(null);
      })
      .catch((e) => {
        if (!current) return;
        setContexts(null);
        setError(errorMessage(e));
      });
    return () => void (current = false);
  }, [c.kubeconfigPath]);

  return (
    <div className="grid grid-cols-2 gap-4">
      <Labeled label="Kubeconfig file" hint="Leave empty to use $KUBECONFIG or ~/.kube/config." className="col-span-2">
        <div className="flex gap-2">
          <input
            className="field flex-1"
            value={c.kubeconfigPath ?? ""}
            placeholder="Default"
            onChange={(e) => onChange({ ...c, kubeconfigPath: e.target.value || null })}
          />
          <button
            className="btn-ghost"
            onClick={async () => {
              const f = await openFile({ multiple: false, directory: false });
              if (typeof f === "string") onChange({ ...c, kubeconfigPath: f });
            }}
            aria-label="Browse"
          >
            <FolderOpen size={14} />
          </button>
        </div>
      </Labeled>
      <Labeled label="Context" hint={error ? <span className="text-critical">{error}</span> : undefined}>
        <select className="field w-full" value={c.context ?? ""} onChange={(e) => onChange({ ...c, context: e.target.value || null })}>
          <option value="">Current context{contexts?.current ? ` (${contexts.current})` : ""}</option>
          {contexts?.contexts.map((x) => (
            <option key={x} value={x}>
              {x}
            </option>
          ))}
        </select>
      </Labeled>
    </div>
  );
}

function SshForm({ c, onChange }: { c: Extract<Connection, { mode: "ssh" }>; onChange: (c: Connection) => void }) {
  const up = (patch: Partial<SshConnection>) => onChange({ ...c, ...patch });
  return (
    <div className="grid grid-cols-6 gap-4">
      <Labeled label="Host" className="col-span-3">
        <input className="field w-full" value={c.host} placeholder="k3s-server.lan or 192.168.1.10" onChange={(e) => up({ host: e.target.value.trim() })} />
      </Labeled>
      <Labeled label="Port" className="col-span-1">
        <NumberInput className="field w-full" value={c.port} min={1} max={65535} onChange={(port) => up({ port })} />
      </Labeled>
      <Labeled label="Username" className="col-span-2">
        <input className="field w-full" value={c.username} onChange={(e) => up({ username: e.target.value.trim() })} />
      </Labeled>

      <div className="col-span-6 flex gap-4 text-sm">
        {(["key", "password"] as const).map((k) => (
          <label key={k} className="flex items-center gap-1.5 text-content-secondary">
            <input
              type="radio"
              className="accent-brand"
              checked={c.auth.kind === k}
              onChange={() =>
                up({
                  auth: k === "key" ? { kind: "key", privateKeyPath: "~/.ssh/id_ed25519", passphrase: null } : { kind: "password", password: "" },
                })
              }
            />
            {k === "key" ? "Private key" : "Password"}
          </label>
        ))}
      </div>

      {c.auth.kind === "key" ? (
        <>
          <Labeled label="Private key file" className="col-span-4">
            <div className="flex gap-2">
              <input
                className="field flex-1"
                value={c.auth.privateKeyPath}
                onChange={(e) => up({ auth: { ...(c.auth as Extract<SshConnection["auth"], { kind: "key" }>), privateKeyPath: e.target.value } })}
              />
              <button
                className="btn-ghost"
                onClick={async () => {
                  const f = await openFile({ multiple: false, directory: false });
                  if (typeof f === "string") up({ auth: { ...(c.auth as Extract<SshConnection["auth"], { kind: "key" }>), privateKeyPath: f } });
                }}
                aria-label="Browse"
              >
                <FolderOpen size={14} />
              </button>
            </div>
          </Labeled>
          <Labeled label="Passphrase" className="col-span-2">
            <input
              className="field w-full"
              type="password"
              value={c.auth.passphrase ?? ""}
              placeholder="None"
              onChange={(e) => up({ auth: { ...(c.auth as Extract<SshConnection["auth"], { kind: "key" }>), passphrase: e.target.value || null } })}
            />
          </Labeled>
        </>
      ) : (
        <Labeled label="Password" className="col-span-3" hint="Stored unencrypted in the local app database. Prefer a key.">
          <input className="field w-full" type="password" value={c.auth.password} onChange={(e) => up({ auth: { kind: "password", password: e.target.value } })} />
        </Labeled>
      )}

      <Labeled
        label="Sudo password"
        className="col-span-3"
        hint={
          c.auth.kind === "password"
            ? "Leave blank to reuse the SSH password. Needed because k3s makes its kubeconfig root-only."
            : "Needed if sudo asks for a password on this server (k3s makes its kubeconfig root-only). Leave blank for passwordless sudo."
        }
      >
        <input
          className="field w-full"
          type="password"
          autoComplete="off"
          value={c.sudoPassword ?? ""}
          placeholder={c.auth.kind === "password" ? "Same as SSH password" : "Not needed"}
          onChange={(e) => up({ sudoPassword: e.target.value || null })}
        />
      </Labeled>
      <p className="col-span-3 self-end pb-1 text-[11px] text-content-muted">
        Sent to sudo over the SSH session, never on the command line. Stored unencrypted in the local app database.
      </p>

      <details className="col-span-6">
        <summary className="cursor-pointer text-xs font-medium text-content-secondary">Advanced</summary>
        <div className="mt-3 grid grid-cols-6 gap-4">
          <Labeled
            label="Kubeconfig command"
            className="col-span-6"
            hint="Runs on the server; its output must be a kubeconfig. k3s.yaml is root-only, so the default uses passwordless sudo (or chmod it / set write-kubeconfig-mode)."
          >
            <input className="field mono w-full" value={c.kubeconfigCommand} onChange={(e) => up({ kubeconfigCommand: e.target.value })} />
          </Labeled>
          <Labeled label="API server host (from the server)" className="col-span-4">
            <input className="field w-full" value={c.apiHost} onChange={(e) => up({ apiHost: e.target.value.trim() })} />
          </Labeled>
          <Labeled label="API port" className="col-span-2">
            <NumberInput className="field w-full" value={c.apiPort} min={1} max={65535} onChange={(apiPort) => up({ apiPort })} />
          </Labeled>
        </div>
      </details>

      <div className="col-span-6 flex items-center gap-2 rounded-md bg-muted px-3 py-2 text-xs text-content-secondary">
        <KeyRound size={14} className="shrink-0 text-content-muted" />
        {c.hostKeyFingerprint ? (
          <>
            <span>
              Pinned host key <span className="mono">{c.hostKeyFingerprint}</span>
            </span>
            <button className="ml-auto text-accent hover:underline" onClick={() => up({ hostKeyFingerprint: null })}>
              Forget
            </button>
          </>
        ) : (
          <span>The server's host key is pinned on first successful connect; a changed key blocks the connection.</span>
        )}
      </div>
    </div>
  );
}

/**
 * A number you can type normally: the text is free while typing (so "30"
 * isn't forced up to the minimum at "3"), a value in range is passed on as
 * you go, and anything else snaps back into range when you leave the field.
 */
function NumberInput({ value, onChange, min = 1, max = Number.MAX_SAFE_INTEGER, className }: { value: number; onChange: (n: number) => void; min?: number; max?: number; className?: string }) {
  const [text, setText] = useState(String(value));
  const focused = useRef(false);
  useEffect(() => {
    if (!focused.current || Number(text) !== value) setText(String(value));
  }, [value]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <input
      className={className}
      type="number"
      min={min}
      max={max === Number.MAX_SAFE_INTEGER ? undefined : max}
      value={text}
      onFocus={() => (focused.current = true)}
      onChange={(e) => {
        setText(e.target.value);
        const n = Number(e.target.value);
        if (e.target.value.trim() !== "" && Number.isFinite(n) && n >= min && n <= max) onChange(n);
      }}
      onBlur={() => {
        focused.current = false;
        const n = Number(text);
        const fixed = text.trim() === "" || !Number.isFinite(n) ? value : Math.min(max, Math.max(min, n));
        setText(String(fixed));
        if (fixed !== value) onChange(fixed);
      }}
    />
  );
}

function NumberField({ label, value, onChange, min = 1, hint, suffix }: { label: string; value: number; onChange: (n: number) => void; min?: number; hint?: string; suffix?: string }) {
  return (
    <Labeled label={label} hint={hint}>
      <div className="flex items-center gap-2">
        <NumberInput className="field w-28" min={min} value={value} onChange={onChange} />
        {suffix && <span className="text-xs text-content-muted">{suffix}</span>}
      </div>
    </Labeled>
  );
}

function MonitoringSection({ draft, set }: { draft: Settings; set: (p: Partial<Settings>) => void }) {
  const parseNamespaces = (text: string) => text.split(",").map((s) => s.trim()).filter(Boolean);
  const [excluded, setExcluded] = useState(draft.excludedNamespaces.join(", "));
  // Follow outside changes (Discard, a reload), but leave what's being typed alone ("a, " is still "a").
  useEffect(() => {
    if (!same(parseNamespaces(excluded), draft.excludedNamespaces)) setExcluded(draft.excludedNamespaces.join(", "));
  }, [draft.excludedNamespaces]); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <Section title="Monitoring" description="How often Portside polls, and when it calls something a problem.">
      <div className="grid grid-cols-3 gap-4">
        <NumberField label="State poll interval" value={draft.pollIntervalSecs} min={5} suffix="seconds" onChange={(n) => set({ pollIntervalSecs: n })} />
        <NumberField label="High usage threshold" value={draft.highUsagePercent} suffix="%" onChange={(n) => set({ highUsagePercent: Math.min(100, n) })} />
        <NumberField label="Restart warning at" value={draft.restartWarningThreshold} suffix="restarts" onChange={(n) => set({ restartWarningThreshold: n })} />
      </div>
      <label className="flex items-center gap-2 text-sm text-content">
        <input type="checkbox" className="accent-brand" checked={draft.collectLogs} onChange={(e) => set({ collectLogs: e.target.checked })} />
        Collect container logs locally for search and analytics
      </label>
      {draft.collectLogs && (
        <div className="grid grid-cols-3 gap-4">
          <NumberField label="Log pull interval" value={draft.logIntervalSecs} min={10} suffix="seconds" onChange={(n) => set({ logIntervalSecs: n })} />
          <NumberField label="First-pull lookback" value={draft.initialLogLookbackHours} suffix="hours" onChange={(n) => set({ initialLogLookbackHours: n })} />
          <NumberField label="Error spike threshold" value={draft.errorLogSpikePerHour} suffix="errors / hour" onChange={(n) => set({ errorLogSpikePerHour: n })} />
          <Labeled label="Skip namespaces" hint="Comma-separated. Still monitored, logs just aren't pulled." className="col-span-3">
            <input
              className="field w-full"
              value={excluded}
              placeholder="e.g. kube-system"
              onChange={(e) => {
                setExcluded(e.target.value);
                set({ excludedNamespaces: parseNamespaces(e.target.value) });
              }}
              onBlur={() => setExcluded(draft.excludedNamespaces.join(", "))}
            />
          </Labeled>
        </div>
      )}
      <NumberField label="Keep data for" value={draft.retentionDays} suffix="days (logs, metrics, resolved problems)" onChange={(n) => set({ retentionDays: n })} />
    </Section>
  );
}

function BackgroundSection({ draft, set }: { draft: Settings; set: (p: Partial<Settings>) => void }) {
  const [testing, setTesting] = useState(false);
  const others = draft.connections.length - (draft.activeConnectionId ? 1 : 0);
  return (
    <Section
      title="Background & notifications"
      description="Keep watching your clusters from the system tray and get a desktop notification when something needs you."
    >
      <label className="flex items-start gap-2 text-sm text-content">
        <input
          type="checkbox"
          className="accent-brand mt-0.5"
          checked={draft.monitoringPaused}
          onChange={(e) => set({ monitoringPaused: e.target.checked })}
        />
        <span>
          Pause all monitoring
          <span className="block text-xs text-content-muted">Stops polling, log pulls and background checks. Also in the tray icon's menu.</span>
        </span>
      </label>
      <label className="flex items-start gap-2 text-sm text-content">
        <input type="checkbox" className="accent-brand mt-0.5" checked={draft.closeToTray} onChange={(e) => set({ closeToTray: e.target.checked })} />
        <span>
          Keep running in the system tray when the window is closed
          <span className="block text-xs text-content-muted">Quit from the tray icon's menu. Launching the app again reopens this window.</span>
        </span>
      </label>
      <label className="flex items-start gap-2 text-sm text-content">
        <input type="checkbox" className="accent-brand mt-0.5" checked={draft.notifyCritical} onChange={(e) => set({ notifyCritical: e.target.checked })} />
        <span>
          Notify me about new critical problems and unreachable clusters
          <span className="block text-xs text-content-muted">Once per problem: it notifies again only if it clears and comes back.</span>
        </span>
      </label>
      {draft.notifyCritical && (
        <div className="flex flex-col gap-4 pl-6">
          <label className="flex items-center gap-2 text-sm text-content">
            <input type="checkbox" className="accent-brand" checked={draft.notifyWarnings} onChange={(e) => set({ notifyWarnings: e.target.checked })} />
            Also notify about warnings (noisier)
          </label>
          <NumberField
            label="Check other connections every"
            value={draft.backgroundCheckMinutes}
            min={5}
            suffix={`minutes · ${others} other connection${others === 1 ? "" : "s"}; the active cluster is checked every ${draft.pollIntervalSecs}s`}
            onChange={(n) => set({ backgroundCheckMinutes: n })}
          />
        </div>
      )}
      <div className="flex gap-2">
        <button
          className="btn-ghost"
          disabled={testing}
          onClick={async () => {
            setTesting(true);
            try {
              await ipc.testNotification();
            } catch (e) {
              toast.error(`Notifications are blocked: ${errorMessage(e)}`);
            } finally {
              setTesting(false);
            }
          }}
        >
          <Bell size={14} /> Send test notification
        </button>
        <button className="btn-ghost" onClick={() => void ipc.checkAllNow().then(() => toast.info("Checking all clusters…"))}>
          <RefreshCw size={14} /> Check all clusters now
        </button>
      </div>
    </Section>
  );
}

function DataSection() {
  const [stats, setStats] = useState<StorageStats | null>(null);
  const clusterId = useClusterStore((s) => s.status?.clusterId);
  const refresh = () => ipc.storageStats().then(setStats).catch(() => setStats(null));
  useEffect(() => void refresh(), [clusterId]);

  return (
    <Section title="Local data" description={clusterId ? <>For <span className="mono">{clusterId}</span></> : "No cluster selected."}>
      {stats && (
        <div className="grid grid-cols-4 gap-3 text-sm">
          <Stat label="Log lines" value={fmtCount(stats.logLines)} sub={stats.oldestLogMs ? `oldest ${fmtAgo(stats.oldestLogMs)}` : undefined} />
          <Stat label="Metric samples" value={fmtCount(stats.nodeSamples + stats.podSamples)} />
          <Stat label="Problems tracked" value={fmtCount(stats.issuesTracked)} />
          <Stat label="Database size" value={fmtBytes(stats.dbBytes)} sub="all clusters" />
        </div>
      )}
      <div className="flex gap-2">
        <button
          className="btn-ghost"
          onClick={async () => {
            try {
              const n = await ipc.pruneNow();
              toast.success(`Pruned ${n.toLocaleString()} old log lines`);
            } catch (e) {
              toast.error(errorMessage(e));
            }
            void refresh();
          }}
        >
          Prune now
        </button>
        <button
          className="btn-ghost hover:!border-critical"
          disabled={!clusterId}
          onClick={async () => {
            if (!(await confirmDestructive("Delete all stored logs, metrics and problem history for this cluster?", "Clear data"))) return;
            try {
              await ipc.clearClusterData();
              toast.success("Cleared");
            } catch (e) {
              toast.error(errorMessage(e));
            }
            void refresh();
          }}
        >
          Clear this cluster's data
        </button>
      </div>
    </Section>
  );
}

function ArchivesSection({ draft, set, saved }: { draft: Settings; set: (p: Partial<Settings>) => void; saved: Settings | null }) {
  // The folder in use; only the default when no custom one is saved.
  const [root, setRoot] = useState("");
  useEffect(() => {
    ipc.archiveRoot().then(setRoot).catch(() => setRoot(""));
  }, [saved?.archiveDir]);

  return (
    <Section
      title="Archives"
      description="Archiving a workload saves its YAML (and stored logs) to a folder here, then removes it from the cluster. Restore it from Workloads → Archived."
    >
      <Labeled label="Archive folder" hint={<>In use: <span className="mono">{root || "…"}</span>. Existing archives aren't moved when you change it.</>}>
        <div className="flex gap-2">
          <input
            className="field flex-1"
            value={draft.archiveDir ?? ""}
            placeholder={saved?.archiveDir ? "Default: archives in the app data folder" : root || "Default"}
            onChange={(e) => set({ archiveDir: e.target.value.trim() ? e.target.value : null })}
          />
          <button
            className="btn-ghost"
            title="Choose a folder"
            onClick={async () => {
              const d = await openFile({ multiple: false, directory: true });
              if (typeof d === "string") set({ archiveDir: d });
            }}
            aria-label="Browse"
          >
            <FolderOpen size={14} />
          </button>
          <button className="btn-ghost" title="Open the folder in use" onClick={() => void ipc.openArchiveFolder(null).catch((e) => toast.error(errorMessage(e)))}>
            Open
          </button>
        </div>
      </Labeled>
    </Section>
  );
}

function VolumeFilesSection({ draft, set }: { draft: Settings; set: (p: Partial<Settings>) => void }) {
  return (
    <Section
      title="Volume files"
      description="Browsing a volume (Storage, or a workload's Files tab) starts a small pod that mounts it, and removes it when you're done."
    >
      <Labeled
        label="Helper image"
        hint="Needs sh, stat, head, tar and gzip. Point it at a mirror if the cluster can't reach Docker Hub."
      >
        <input
          className="field w-full"
          value={draft.filesHelperImage}
          placeholder="busybox:1.37"
          onChange={(e) => set({ filesHelperImage: e.target.value })}
        />
      </Labeled>
    </Section>
  );
}

function Stat({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-md bg-muted px-3 py-2">
      <div className="text-[11px] text-content-muted">{label}</div>
      <div className="font-semibold tabular-nums text-content">{value}</div>
      {sub && <div className="text-[11px] text-content-muted">{sub}</div>}
    </div>
  );
}

function AppearanceSection() {
  const { pref, setPref } = useThemeStore();
  return (
    <Section title="Appearance">
      <div className="flex gap-2">
        {(["dark", "light", "system"] as ThemePref[]).map((p) => (
          <button key={p} className={`btn-chip capitalize ${pref === p ? "!border-accent !text-content" : ""}`} onClick={() => setPref(p)}>
            {p}
          </button>
        ))}
      </div>
    </Section>
  );
}
