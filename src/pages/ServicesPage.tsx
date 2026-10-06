import { Fragment, useMemo, useState } from "react";
import { ArrowRightLeft, FileCode2 } from "lucide-react";
import { ForwardsPanel } from "../components/Forwards";
import { useClusterStore } from "../stores/cluster";
import { fmtAge } from "../lib/format";
import type { ServiceInfo, ServicePort } from "../lib/types";
import IngressTable from "../components/IngressTable";
import SnapshotGate from "../components/SnapshotGate";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput, StatusPill } from "../components/ui";
import { useNavStore } from "../stores/nav";

/** Endpoint health: what traffic to this Service would actually hit. */
function endpoints(s: ServiceInfo): { label: string; tone: "good" | "warning" | "critical" | "muted"; rank: number } {
  if (s.type === "ExternalName") return { label: "External", tone: "muted", rank: 3 };
  if (Object.keys(s.selector).length === 0) return { label: "No selector", tone: "muted", rank: 3 };
  if (s.podsMatched === 0 && s.idle) return { label: "Scaled to 0", tone: "muted", rank: 3 };
  if (s.podsMatched === 0) return { label: "No pods", tone: "critical", rank: 0 };
  if (s.podsReady === 0) return { label: `0/${s.podsMatched} ready`, tone: "critical", rank: 0 };
  if (s.podsReady < s.podsMatched) return { label: `${s.podsReady}/${s.podsMatched} ready`, tone: "warning", rank: 1 };
  return { label: `${s.podsReady}/${s.podsMatched} ready`, tone: "good", rank: 2 };
}

function portLabel(p: ServicePort): string {
  const target = p.targetPort && p.targetPort !== String(p.port) ? `→${p.targetPort}` : "";
  const node = p.nodePort ? ` (node ${p.nodePort})` : "";
  return `${p.port}${target}/${p.protocol}${node}`;
}

type Tab = "services" | "ingresses";

export default function ServicesPage() {
  const [tab, setTab] = useState<Tab>("services");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  return (
    <SnapshotGate>
      {(s) => (
        <div className="flex h-full flex-col">
          <PageHeader
            title="Services"
            subtitle={tab === "services" ? "endpoint health is computed from the pods each selector matches" : "routes into the cluster and the Services they send traffic to"}
          >
            <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />
            <SearchInput value={search} onChange={setSearch} placeholder={tab === "services" ? "Name or IP…" : "Name, host or service…"} className="w-56" />
          </PageHeader>
          <div className="flex gap-1 border-b border-border-light px-6">
            <button className={`navtab ${tab === "services" ? "navtab-active" : ""}`} onClick={() => setTab("services")}>
              Services <span className="text-content-muted">{s.services.length}</span>
            </button>
            <button className={`navtab ${tab === "ingresses" ? "navtab-active" : ""}`} onClick={() => setTab("ingresses")}>
              Ingresses <span className="text-content-muted">{s.ingresses.length}</span>
            </button>
          </div>
          {tab === "services" ? (
            <ServicesTable services={s.services} namespace={namespace} search={search} />
          ) : (
            <IngressTable ingresses={s.ingresses} namespace={namespace} search={search} />
          )}
        </div>
      )}
    </SnapshotGate>
  );
}

function ServicesTable(p: { services: ServiceInfo[]; namespace: string; search: string }) {
  const { openPod, openEditor, openForward } = useNavStore();
  const forwards = useClusterStore((s) => s.forwards);
  const [expanded, setExpanded] = useState<string | null>(null);
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.services
      .filter((s) => (!p.namespace || s.namespace === p.namespace) && (!q || `${s.name} ${s.clusterIp ?? ""} ${s.external.join(" ")}`.toLowerCase().includes(q)))
      .sort((a, b) => endpoints(a).rank - endpoints(b).rank || a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name));
  }, [p.services, p.namespace, p.search]);
  const now = Date.now();

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ForwardsPanel />
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Type</th>
              <th>Endpoints</th>
              <th>Cluster IP</th>
              <th>External</th>
              <th>Ports</th>
              <th>Routes</th>
              <th className="text-right">Age</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((s) => {
              const key = `${s.namespace}/${s.name}`;
              const ep = endpoints(s);
              const open = expanded === key;
              return (
                <Fragment key={key}>
                  <tr className="cursor-pointer" onClick={() => setExpanded(open ? null : key)}>
                    <td>
                      <div className="max-w-[260px] truncate font-medium text-content" title={s.name}>{s.name}</div>
                      <div className="text-[11px] text-content-muted">{s.namespace}</div>
                    </td>
                    <td className="text-xs text-content-secondary">{s.type}</td>
                    <td><StatusPill status={ep.label} tone={ep.tone} /></td>
                    <td className="mono text-content-secondary">{s.clusterIp ?? "—"}</td>
                    <td className="mono text-content-secondary">
                      <div className="max-w-[180px] truncate" title={s.external.join(", ")}>{s.external.join(", ") || "—"}</div>
                    </td>
                    <td className="mono text-content-secondary">
                      <div className="max-w-[220px] truncate" title={s.ports.map(portLabel).join(", ")}>{s.ports.map(portLabel).join(", ") || "—"}</div>
                    </td>
                    <td className="text-xs text-content-secondary">{s.routes.length || "—"}</td>
                    <td className="text-right text-xs text-content-muted">{fmtAge(s.createdMs, now)}</td>
                    <td className="whitespace-nowrap text-right" onClick={(e) => e.stopPropagation()}>
                      {(() => {
                        const active = forwards.find((f) => f.namespace === s.namespace && f.service === s.name);
                        const forwardable = s.type !== "ExternalName" && Object.keys(s.selector).length > 0;
                        return (
                          <button
                            className={`btn-quiet ${active ? "!text-accent" : ""}`}
                            disabled={!forwardable}
                            title={active ? `Forwarded to localhost:${active.localPort}` : forwardable ? "Forward a port to this computer" : "Nothing to forward to (no selector)"}
                            onClick={() => openForward(s.namespace, s.name)}
                          >
                            <ArrowRightLeft size={14} />
                          </button>
                        );
                      })()}
                      <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: "Service", namespace: s.namespace, name: s.name })}>
                        <FileCode2 size={14} />
                      </button>
                    </td>
                  </tr>
                  {open && (
                    <tr>
                      <td colSpan={9} className="bg-raised">
                        <div className="grid gap-3 py-1 text-xs md:grid-cols-3">
                          <div>
                            <div className="field-label">Selector</div>
                            <div className="flex flex-wrap gap-1">
                              {Object.entries(s.selector).map(([k, v]) => (
                                <span key={k} className="mono rounded bg-muted px-1.5 py-0.5 text-content-secondary">{k}={v}</span>
                              ))}
                              {Object.keys(s.selector).length === 0 && <span className="text-content-muted">none (endpoints managed by hand)</span>}
                            </div>
                          </div>
                          <div>
                            <div className="field-label">Backing pods</div>
                            <div className="flex flex-wrap gap-1">
                              {s.podNames.map((n) => (
                                <button key={n} className="btn-chip" onClick={() => openPod(s.namespace, n)}>{n}</button>
                              ))}
                              {s.podNames.length === 0 && <span className="text-content-muted">none</span>}
                            </div>
                          </div>
                          <div>
                            <div className="field-label">Ingress routes</div>
                            {s.routes.map((r) => (
                              <div key={r} className="mono truncate text-content-secondary" title={r}>{r}</div>
                            ))}
                            {s.routes.length === 0 && <span className="text-content-muted">none</span>}
                          </div>
                        </div>
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </table>
        {rows.length === 0 && <EmptyState title="No Services match">{p.services.length ? "Nothing fits the namespace and search filter." : "The cluster has no Services."}</EmptyState>}
      </div>
    </div>
  );
}
