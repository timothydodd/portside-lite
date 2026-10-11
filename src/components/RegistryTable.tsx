import { useMemo } from "react";
import { AlertTriangle, KeyRound } from "lucide-react";
import type { RegistryInfo } from "../lib/types";
import { EmptyState } from "./ui";

/** "namespace/Kind/name" → in `namespace`. */
const inNamespace = (ref: string, ns: string) => ref.startsWith(`${ns}/`);

export default function RegistryTable(p: { registries: RegistryInfo[]; namespace: string; search: string; onAddCredential: (host: string) => void }) {
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    const ns = p.namespace;
    return p.registries
      .map((r) =>
        ns
          ? {
              ...r,
              usedBy: r.usedBy.filter((u) => inNamespace(u, ns)),
              withoutCredentials: r.withoutCredentials.filter((u) => inNamespace(u, ns)),
              pullErrors: r.pullErrors.filter((u) => inNamespace(u, ns)),
              credentials: r.credentials.filter((c) => c.namespace === ns),
            }
          : r,
      )
      .filter((r) => !ns || r.usedBy.length > 0 || r.credentials.length > 0)
      .filter((r) => !q || `${r.host} ${r.images.join(" ")} ${r.credentials.map((c) => c.secret).join(" ")}`.toLowerCase().includes(q))
      .sort((a, b) => b.pullErrors.length - a.pullErrors.length || b.usedBy.length - a.usedBy.length || a.host.localeCompare(b.host));
  }, [p.registries, p.namespace, p.search]);

  if (rows.length === 0) {
    return (
      <EmptyState title="No registries">
        {p.registries.length ? "Nothing fits the namespace and search filter." : "No images in use and no registry credentials found."}
      </EmptyState>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <p className="px-6 pt-3 text-[11px] text-content-muted">
        Built from the images in use and the pull secrets (<span className="mono">kubernetes.io/dockerconfigjson</span>) on the cluster.
        Credentials set on the nodes (k3s <span className="mono">registries.yaml</span>) aren't visible here.
      </p>
      <table className="table">
        <thead>
          <tr>
            <th>Registry</th>
            <th>Images</th>
            <th>Used by</th>
            <th>Credentials</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((r) => {
            const failing = r.pullErrors.length > 0;
            return (
              <tr key={r.host}>
                <td className="align-top">
                  <div className="mono font-medium text-content">{r.host}</div>
                  {failing && (
                    <div className="mt-0.5 flex items-center gap-1 text-[11px] text-critical" title={r.pullErrors.join("\n")}>
                      <AlertTriangle size={11} className="shrink-0" />
                      {r.pullErrors.length} pull error{r.pullErrors.length > 1 ? "s" : ""}
                    </div>
                  )}
                </td>
                <td className="align-top text-xs">
                  {r.images.length ? (
                    <div className="max-w-[300px] truncate text-content-secondary" title={r.images.join("\n")}>
                      <span className="text-content">{r.images.length}</span>
                      <span className="mono text-content-muted"> · {r.images.slice(0, 2).join(", ")}{r.images.length > 2 ? ", …" : ""}</span>
                    </div>
                  ) : (
                    <span className="text-content-muted">none in use</span>
                  )}
                </td>
                <td className="align-top text-xs">
                  {r.usedBy.length ? (
                    <div className="max-w-[240px] truncate text-content-secondary" title={r.usedBy.join("\n")}>
                      {r.usedBy.length} workload{r.usedBy.length > 1 ? "s" : ""}
                    </div>
                  ) : (
                    <span className="text-content-muted">unused</span>
                  )}
                </td>
                <td className="align-top text-xs">
                  <div className="flex flex-col gap-0.5">
                    {r.credentials.map((c) => (
                      <span
                        key={`${c.namespace}/${c.secret}`}
                        className="flex items-center gap-1 text-content-secondary"
                        title={c.serviceAccounts.length ? `Given to pods by ServiceAccount ${c.serviceAccounts.join(", ")}` : "Not on any ServiceAccount; pods must list it in imagePullSecrets"}
                      >
                        <KeyRound size={11} className="shrink-0 text-content-muted" />
                        <span className="mono">{c.namespace}/{c.secret}</span>
                        {c.username && <span className="text-content-muted">as {c.username}</span>}
                        {c.serviceAccounts.length > 0 && <span className="text-content-muted">· via {c.serviceAccounts.join(", ")}</span>}
                      </span>
                    ))}
                    {r.withoutCredentials.length > 0 && (
                      <span
                        className={failing ? "flex items-center gap-1 text-warning" : "text-content-muted"}
                        title={r.withoutCredentials.join("\n")}
                      >
                        {failing && <AlertTriangle size={11} className="shrink-0" />}
                        {r.withoutCredentials.length} workload{r.withoutCredentials.length > 1 ? "s pull" : " pulls"} without one
                        {!failing && " (fine if it's public)"}
                      </span>
                    )}
                    {r.credentials.length === 0 && r.withoutCredentials.length === 0 && <span className="text-content-muted">none</span>}
                  </div>
                </td>
                <td className="align-top text-right">
                  <button className="btn-quiet" title="Add a credential for this registry" aria-label="Add credential" onClick={() => p.onAddCredential(r.host)}>
                    <KeyRound size={14} />
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
