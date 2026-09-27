import { useMemo, useState } from "react";
import { Download, FileCode2, Lock } from "lucide-react";
import * as ipc from "../lib/ipc";
import { saveYamlFile } from "../lib/files";
import { errorMessage, fmtAge, fmtBytes } from "../lib/format";
import type { ConfigInfo } from "../lib/types";
import SnapshotGate from "../components/SnapshotGate";
import { EmptyState, NamespaceSelect, PageHeader, SearchInput } from "../components/ui";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";

/** Machinery rather than configuration: Helm release records, service-account tokens, the CA bundle. */
function isSystem(c: ConfigInfo): boolean {
  if (c.kind === "ConfigMap") return c.name === "kube-root-ca.crt" || c.name.startsWith("sh.helm.release.");
  return c.secretType === "helm.sh/release.v1" || c.secretType === "kubernetes.io/service-account-token" || c.secretType === "bootstrap.kubernetes.io/token";
}

async function exportOne(c: ConfigInfo) {
  try {
    const y = await ipc.exportManifests(c.kind, c.namespace, c.name);
    const path = await saveYamlFile(`${c.name}.${c.kind.toLowerCase()}.yaml`, y);
    if (path) toast.success(`Exported to ${path}`);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

export default function ConfigPage() {
  const [kind, setKind] = useState<"ConfigMap" | "Secret">("ConfigMap");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [hideSystem, setHideSystem] = useState(true);
  const { openConfig, openEditor } = useNavStore();

  return (
    <SnapshotGate>
      {(s) => (
        <ConfigTable
          configs={s.configs}
          namespaces={s.namespaces}
          {...{ kind, setKind, namespace, setNamespace, search, setSearch, hideSystem, setHideSystem, openConfig, openEditor }}
        />
      )}
    </SnapshotGate>
  );
}

function ConfigTable(p: {
  configs: ConfigInfo[];
  namespaces: string[];
  kind: "ConfigMap" | "Secret";
  setKind: (k: "ConfigMap" | "Secret") => void;
  namespace: string;
  setNamespace: (v: string) => void;
  search: string;
  setSearch: (v: string) => void;
  hideSystem: boolean;
  setHideSystem: (v: boolean) => void;
  openConfig: ReturnType<typeof useNavStore.getState>["openConfig"];
  openEditor: ReturnType<typeof useNavStore.getState>["openEditor"];
}) {
  const now = Date.now();
  const visible = useMemo(() => p.configs.filter((c) => !p.hideSystem || !isSystem(c)), [p.configs, p.hideSystem]);
  const count = (k: string) => visible.filter((c) => c.kind === k).length;
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return visible
      .filter((c) => c.kind === p.kind && (!p.namespace || c.namespace === p.namespace))
      .filter((c) => !q || `${c.name} ${c.keys.join(" ")}`.toLowerCase().includes(q))
      .sort((a, b) => a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name));
  }, [visible, p.kind, p.namespace, p.search]);

  return (
    <div className="flex h-full flex-col">
      <PageHeader title="Configuration">
        <label className="flex items-center gap-1.5 text-xs text-content-secondary">
          <input type="checkbox" className="accent-brand" checked={p.hideSystem} onChange={(e) => p.setHideSystem(e.target.checked)} />
          Hide system
        </label>
        <NamespaceSelect namespaces={p.namespaces} value={p.namespace} onChange={p.setNamespace} />
        <SearchInput value={p.search} onChange={p.setSearch} placeholder="Name or key…" className="w-56" />
      </PageHeader>
      <div className="flex gap-1 border-b border-border-light px-6">
        {(["ConfigMap", "Secret"] as const).map((k) => (
          <button key={k} className={`navtab ${p.kind === k ? "navtab-active" : ""}`} onClick={() => p.setKind(k)}>
            {k}s <span className="text-content-muted">{count(k)}</span>
          </button>
        ))}
      </div>
      {rows.length === 0 ? (
        <EmptyState title={`No ${p.kind}s`}>{p.hideSystem && "System entries are hidden; untick “Hide system” to see them."}</EmptyState>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                {p.kind === "Secret" && <th>Type</th>}
                <th>Keys</th>
                <th className="text-right">Size</th>
                <th>Used by</th>
                <th className="text-right">Age</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((c) => (
                <tr key={`${c.namespace}/${c.name}`} className="cursor-pointer" onClick={() => p.openConfig({ kind: c.kind, namespace: c.namespace, name: c.name })}>
                  <td>
                    <div className="flex max-w-[280px] items-center gap-1.5">
                      <span className="truncate font-medium text-content" title={c.name}>{c.name}</span>
                      {c.immutable && <Lock size={12} className="shrink-0 text-content-muted" aria-label="immutable" />}
                    </div>
                    <div className="text-[11px] text-content-muted">{c.namespace}</div>
                  </td>
                  {p.kind === "Secret" && <td className="text-xs text-content-secondary">{c.secretType ?? "Opaque"}</td>}
                  <td className="text-xs text-content-secondary">
                    <div className="max-w-[280px] truncate" title={c.keys.join(", ")}>
                      <span className="text-content">{c.keys.length}</span>
                      {c.keys.length > 0 && <span className="mono text-content-muted"> · {c.keys.slice(0, 3).join(", ")}{c.keys.length > 3 ? ", …" : ""}</span>}
                    </div>
                  </td>
                  <td className="text-right text-xs tabular-nums text-content-secondary">{fmtBytes(c.sizeBytes)}</td>
                  <td className="text-xs">
                    {c.usedBy.length ? (
                      <div className="max-w-[240px] truncate text-content-secondary" title={c.usedBy.join("\n")}>{c.usedBy.join(", ")}</div>
                    ) : (
                      <span className="text-content-muted">unused</span>
                    )}
                  </td>
                  <td className="text-right text-xs text-content-muted">{fmtAge(c.createdMs, now)}</td>
                  <td className="text-right" onClick={(e) => e.stopPropagation()}>
                    <button className="btn-quiet" title="Edit YAML" onClick={() => p.openEditor({ kind: c.kind, namespace: c.namespace, name: c.name })}>
                      <FileCode2 size={14} />
                    </button>
                    <button className="btn-quiet" title="Export YAML to a file" onClick={() => void exportOne(c)}>
                      <Download size={14} />
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
