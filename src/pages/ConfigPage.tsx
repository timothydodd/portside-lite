import { useMemo, useState } from "react";
import { Download, FileCode2, KeyRound, Lock, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { saveYamlFile } from "../lib/files";
import { errorMessage, fmtAge, fmtBytes } from "../lib/format";
import type { ClusterSnapshot, ConfigInfo } from "../lib/types";
import RegistryCredentialDialog from "../components/RegistryCredentialDialog";
import RegistryTable from "../components/RegistryTable";
import ServiceAccountTable, { isSystemAccount } from "../components/ServiceAccountTable";
import SnapshotGate from "../components/SnapshotGate";
import { ActionMenu, EmptyState, NamespaceSelect, PageHeader, SearchInput } from "../components/ui";
import { useDeleteDialog } from "../lib/actions";
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

type Tab = "ConfigMap" | "Secret" | "ServiceAccount" | "Registry";
const TABS: { tab: Tab; label: string }[] = [
  { tab: "ConfigMap", label: "ConfigMaps" },
  { tab: "Secret", label: "Secrets" },
  { tab: "ServiceAccount", label: "Service accounts" },
  { tab: "Registry", label: "Registries" },
];
const SEARCH_HINT: Record<Tab, string> = {
  ConfigMap: "Name or key…",
  Secret: "Name or key…",
  ServiceAccount: "Name, secret or role…",
  Registry: "Host, image or secret…",
};

export default function ConfigPage() {
  const [tab, setTab] = useState<Tab>("ConfigMap");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  const [hideSystem, setHideSystem] = useState(true);
  /** Add-credential dialog, prefilled with this registry host. */
  const [adding, setAdding] = useState<string | null>(null);
  const { openConfig, openEditor } = useNavStore();

  return (
    <SnapshotGate>
      {(s) => (
        <div className="flex h-full flex-col">
          <PageHeader title="Configuration">
            {tab !== "Registry" && (
              <label className="flex items-center gap-1.5 text-xs text-content-secondary">
                <input type="checkbox" className="accent-brand" checked={hideSystem} onChange={(e) => setHideSystem(e.target.checked)} />
                Hide system
              </label>
            )}
            <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />
            <SearchInput value={search} onChange={setSearch} placeholder={SEARCH_HINT[tab]} className="w-56" />
            {tab === "Registry" && (
              <button className="btn-primary" onClick={() => setAdding("")}>
                <KeyRound size={14} /> Add credential
              </button>
            )}
          </PageHeader>
          <div className="flex gap-1 border-b border-border-light px-6">
            {TABS.map((t) => (
              <button key={t.tab} className={`navtab ${tab === t.tab ? "navtab-active" : ""}`} onClick={() => setTab(t.tab)}>
                {t.label} <span className="text-content-muted">{tabCount(s, t.tab, hideSystem)}</span>
              </button>
            ))}
          </div>
          {tab === "ServiceAccount" ? (
            <ServiceAccountTable accounts={s.serviceAccounts} namespace={namespace} search={search} hideSystem={hideSystem} />
          ) : tab === "Registry" ? (
            <RegistryTable registries={s.registries} namespace={namespace} search={search} onAddCredential={setAdding} />
          ) : (
            <ConfigTable configs={s.configs} kind={tab} {...{ namespace, search, hideSystem, openConfig, openEditor }} />
          )}
          {adding != null && (
            <RegistryCredentialDialog
              host={adding}
              namespace={namespace}
              namespaces={s.namespaces}
              accounts={s.serviceAccounts}
              onClose={() => setAdding(null)}
            />
          )}
        </div>
      )}
    </SnapshotGate>
  );
}

function tabCount(s: ClusterSnapshot, tab: Tab, hideSystem: boolean): number {
  if (tab === "ServiceAccount") return s.serviceAccounts.filter((a) => !hideSystem || !isSystemAccount(a)).length;
  if (tab === "Registry") return s.registries.length;
  return s.configs.filter((c) => c.kind === tab && (!hideSystem || !isSystem(c))).length;
}

function ConfigTable(p: {
  configs: ConfigInfo[];
  kind: "ConfigMap" | "Secret";
  namespace: string;
  search: string;
  hideSystem: boolean;
  openConfig: ReturnType<typeof useNavStore.getState>["openConfig"];
  openEditor: ReturnType<typeof useNavStore.getState>["openEditor"];
}) {
  const now = Date.now();
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.configs
      .filter((c) => !p.hideSystem || !isSystem(c))
      .filter((c) => c.kind === p.kind && (!p.namespace || c.namespace === p.namespace))
      .filter((c) => !q || `${c.name} ${c.keys.join(" ")}`.toLowerCase().includes(q))
      .sort((a, b) => a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name));
  }, [p.configs, p.hideSystem, p.kind, p.namespace, p.search]);

  return (
    <>
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
                    <ActionMenu
                      label={`Actions for ${c.name}`}
                      items={[
                        { label: "Edit YAML", icon: <FileCode2 size={14} />, onSelect: () => p.openEditor({ kind: c.kind, namespace: c.namespace, name: c.name }) },
                        { label: "Export YAML", icon: <Download size={14} />, title: "Export YAML to a file", onSelect: () => void exportOne(c) },
                        {
                          label: `Delete ${c.kind}…`,
                          icon: <Trash2 size={14} />,
                          danger: true,
                          onSelect: () => useDeleteDialog.getState().open({ kind: c.kind, namespace: c.namespace, name: c.name, dependents: c.usedBy }),
                        },
                      ]}
                    />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </>
  );
}
