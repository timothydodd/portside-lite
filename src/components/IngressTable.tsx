import { useMemo } from "react";
import { AlertTriangle, FileCode2, Lock } from "lucide-react";
import { fmtAge } from "../lib/format";
import type { IngressInfo, IngressRoute } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { EmptyState } from "./ui";

function backend(r: IngressRoute): string {
  if (r.service) return `${r.service}${r.port ? `:${r.port}` : ""}`;
  return r.resource ?? "—";
}

export default function IngressTable(p: { ingresses: IngressInfo[]; namespace: string; search: string }) {
  const openEditor = useNavStore((s) => s.openEditor);
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    const broken = (i: IngressInfo) => i.routes.some((r) => r.service && !r.serviceFound);
    return p.ingresses
      .filter(
        (i) =>
          (!p.namespace || i.namespace === p.namespace) &&
          (!q || `${i.name} ${i.routes.map((r) => `${r.host}${r.path} ${r.service ?? ""}`).join(" ")}`.toLowerCase().includes(q)),
      )
      .sort((a, b) => Number(broken(b)) - Number(broken(a)) || a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name));
  }, [p.ingresses, p.namespace, p.search]);
  const now = Date.now();

  if (rows.length === 0) {
    return (
      <EmptyState title="No Ingresses match">
        {p.ingresses.length ? "Nothing fits the namespace and search filter." : "Nothing on this cluster routes outside traffic in through an Ingress."}
      </EmptyState>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Class</th>
            <th>Routes</th>
            <th>Address</th>
            <th className="text-right">Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((i) => (
            <tr key={`${i.namespace}/${i.name}`}>
              <td className="align-top">
                <div className="max-w-[240px] truncate font-medium text-content" title={i.name}>{i.name}</div>
                <div className="text-[11px] text-content-muted">{i.namespace}</div>
              </td>
              <td className="align-top text-xs text-content-secondary">{i.class ?? "—"}</td>
              <td>
                <div className="flex flex-col gap-0.5">
                  {i.routes.map((r, n) => {
                    const missing = r.service != null && !r.serviceFound;
                    return (
                      <div key={n} className="mono flex items-center gap-1.5 text-xs text-content-secondary">
                        {i.tlsHosts.includes(r.host) ? (
                          <Lock size={11} className="shrink-0 text-good" aria-label="TLS" />
                        ) : (
                          <span className="w-[11px] shrink-0" />
                        )}
                        <span className="max-w-[260px] truncate" title={`${r.host}${r.path}`}>
                          {r.host}
                          {r.path}
                        </span>
                        <span className="text-content-muted">&rarr;</span>
                        <span className={missing ? "text-critical" : ""} title={missing ? "No Service with this name in the namespace" : undefined}>
                          {backend(r)}
                        </span>
                        {missing && <AlertTriangle size={11} className="shrink-0 text-critical" aria-label="Service missing" />}
                      </div>
                    );
                  })}
                  {i.routes.length === 0 && <span className="text-xs text-content-muted">no rules</span>}
                </div>
              </td>
              <td className="mono align-top text-content-secondary">{i.address.join(", ") || "—"}</td>
              <td className="align-top text-right text-xs text-content-muted">{fmtAge(i.createdMs, now)}</td>
              <td className="align-top whitespace-nowrap text-right">
                <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: "Ingress", namespace: i.namespace, name: i.name })}>
                  <FileCode2 size={14} />
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
