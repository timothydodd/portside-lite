import { useMemo, useState } from "react";
import { CheckCircle2, FlaskConical, KeyRound } from "lucide-react";
import * as ipc from "../lib/ipc";
import { errorMessage } from "../lib/format";
import type { ServiceAccountInfo } from "../lib/types";
import { toast } from "../stores/toast";
import { Modal, Spinner } from "./ui";

/** "ghcr.io" → "ghcr-io-pull"; a valid Secret name. */
const defaultName = (host: string) =>
  `${host.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "registry"}-pull`;

/**
 * Create a registry pull secret (like `kubectl create secret docker-registry`)
 * and optionally add it to a ServiceAccount so its pods use it. The password
 * goes straight to the cluster.
 */
export default function RegistryCredentialDialog(p: {
  host: string;
  namespace: string;
  namespaces: string[];
  accounts: ServiceAccountInfo[];
  onClose: () => void;
}) {
  const [server, setServer] = useState(p.host);
  const [namespace, setNamespace] = useState(p.namespace || (p.namespaces.includes("default") ? "default" : p.namespaces[0] ?? ""));
  const [name, setName] = useState(defaultName(p.host));
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [email, setEmail] = useState("");
  const [account, setAccount] = useState("default");
  const [busy, setBusy] = useState<null | "dry" | "create">(null);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);

  const accounts = useMemo(() => p.accounts.filter((a) => a.namespace === namespace).map((a) => a.name).sort(), [p.accounts, namespace]);
  const ready = server.trim() && namespace.trim() && name.trim() && username && password;

  const run = async (dryRun: boolean) => {
    setBusy(dryRun ? "dry" : "create");
    setResult(null);
    try {
      const done = await ipc.addRegistryCredential(
        { namespace: namespace.trim(), name: name.trim(), server: server.trim(), username, password, email: email.trim() || null },
        account || null,
        dryRun,
      );
      if (dryRun) {
        setResult({ ok: true, text: done.join("; ") });
      } else {
        toast.success(done.join("; "));
        p.onClose();
      }
    } catch (e) {
      setResult({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <Modal title="Add registry credential" onClose={p.onClose} wide explicitClose>
      <div className="flex flex-col gap-3 text-sm">
        <p className="text-xs text-content-muted">
          Creates a <span className="mono">kubernetes.io/dockerconfigjson</span> Secret, the same as{" "}
          <span className="mono">kubectl create secret docker-registry</span>. Pods use it when it's on their ServiceAccount or in their{" "}
          <span className="mono">imagePullSecrets</span>. The password is sent to the cluster and not kept by Portside.
        </p>
        <div className="grid grid-cols-2 gap-3">
          <label className="block">
            <span className="field-label">Registry server</span>
            <input className="field w-full" value={server} placeholder="ghcr.io" onChange={(e) => setServer(e.target.value)} />
          </label>
          <label className="block">
            <span className="field-label">Namespace</span>
            <input className="field w-full" list="cred-ns" value={namespace} onChange={(e) => setNamespace(e.target.value)} />
            <datalist id="cred-ns">
              {p.namespaces.map((n) => <option key={n} value={n} />)}
            </datalist>
          </label>
          <label className="block">
            <span className="field-label">Username</span>
            <input className="field w-full" value={username} autoComplete="off" onChange={(e) => setUsername(e.target.value)} />
          </label>
          <label className="block">
            <span className="field-label">Password or token</span>
            <input className="field w-full" type="password" value={password} autoComplete="new-password" onChange={(e) => setPassword(e.target.value)} />
          </label>
          <label className="block">
            <span className="field-label">Secret name</span>
            <input className="field mono w-full" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="block">
            <span className="field-label">Email (optional)</span>
            <input className="field w-full" value={email} onChange={(e) => setEmail(e.target.value)} />
          </label>
          <label className="col-span-2 block">
            <span className="field-label">Give it to pods running as</span>
            <select className="field w-full" value={account} onChange={(e) => setAccount(e.target.value)}>
              <option value="">No ServiceAccount (only pods that list it themselves)</option>
              {!accounts.includes("default") && <option value="default">default</option>}
              {accounts.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </select>
            <span className="mt-1 block text-[11px] text-content-muted">
              Added to the account's <span className="mono">imagePullSecrets</span>. Pods pick it up when they're next created (restart the workload).
            </span>
          </label>
        </div>
        {result && (
          <div className={`${result.ok ? "tint-good" : "tint-critical"} flex items-start gap-2 rounded-md px-3 py-2 text-xs`}>
            {result.ok && <CheckCircle2 size={14} className="mt-px shrink-0" />}
            {result.text}
          </div>
        )}
        <div className="flex justify-end gap-2">
          <button className="btn-ghost" onClick={p.onClose}>
            Cancel
          </button>
          <button className="btn-ghost" disabled={!!busy || !ready} onClick={() => void run(true)}>
            {busy === "dry" ? <Spinner size={14} /> : <FlaskConical size={14} />} Dry run
          </button>
          <button className="btn-primary" disabled={!!busy || !ready} onClick={() => void run(false)}>
            {busy === "create" ? <Spinner size={14} /> : <KeyRound size={14} />} Create
          </button>
        </div>
      </div>
    </Modal>
  );
}
