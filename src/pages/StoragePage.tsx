import { useMemo, useState } from "react";
import { FileCode2, FolderOpen, Lock, ShieldCheck, Trash2 } from "lucide-react";
import { deleteVolumeClaim } from "../lib/actions";
import { fmtAge } from "../lib/format";
import type { VolumeClaimInfo } from "../lib/types";
import SnapshotGate from "../components/SnapshotGate";
import { PersistentVolumeTable, SHORT_MODES, StorageClassTable } from "../components/StorageTables";
import { ActionMenu, EmptyState, NamespaceSelect, PageHeader, SearchInput, StatusPill } from "../components/ui";
import { useNavStore } from "../stores/nav";

type Tab = "claims" | "volumes" | "classes";

const SUBTITLES: Record<Tab, string> = {
  claims: "browse files on any claim; changing files or deleting one needs the apps using it scaled to 0",
  volumes: "the disks behind claims; a Released volume with a Retain policy still holds data",
  classes: "how new claims get their volumes",
};

export default function StoragePage() {
  const [tab, setTab] = useState<Tab>("claims");
  const [namespace, setNamespace] = useState("");
  const [search, setSearch] = useState("");
  return (
    <SnapshotGate>
      {(s) => {
        const tabs: [Tab, string, number][] = [
          ["claims", "Claims", s.volumes.length],
          ["volumes", "Volumes", s.persistentVolumes.length],
          ["classes", "Storage classes", s.storageClasses.length],
        ];
        return (
          <div className="flex h-full flex-col">
            <PageHeader title="Storage" subtitle={SUBTITLES[tab]}>
              {tab !== "classes" && <NamespaceSelect namespaces={s.namespaces} value={namespace} onChange={setNamespace} />}
              <SearchInput value={search} onChange={setSearch} placeholder={tab === "classes" ? "Name or provisioner…" : "Name, claim or class…"} className="w-56" />
            </PageHeader>
            <div className="flex gap-1 border-b border-border-light px-6">
              {tabs.map(([t, label, count]) => (
                <button key={t} className={`navtab ${tab === t ? "navtab-active" : ""}`} onClick={() => setTab(t)}>
                  {label} <span className="text-content-muted">{count}</span>
                </button>
              ))}
            </div>
            {tab === "claims" ? (
              <ClaimTable volumes={s.volumes} namespace={namespace} search={search} />
            ) : tab === "volumes" ? (
              <PersistentVolumeTable volumes={s.persistentVolumes} namespace={namespace} search={search} />
            ) : (
              <StorageClassTable classes={s.storageClasses} search={search} />
            )}
          </div>
        );
      }}
    </SnapshotGate>
  );
}

function ClaimTable(p: { volumes: VolumeClaimInfo[]; namespace: string; search: string }) {
  const { openFiles, openWorkload, openPod, openEditor } = useNavStore();
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.volumes
      .filter((v) => (!p.namespace || v.namespace === p.namespace) && (!q || `${v.name} ${v.usedBy.join(" ")} ${v.storageClass ?? ""}`.toLowerCase().includes(q)))
      .sort((a, b) => a.namespace.localeCompare(b.namespace) || a.name.localeCompare(b.name));
  }, [p.volumes, p.namespace, p.search]);
  const now = Date.now();

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        {rows.length === 0 ? (
          <EmptyState title="No PersistentVolumeClaims">{p.volumes.length ? "Nothing matches the filter." : "Nothing on this cluster asks for persistent storage."}</EmptyState>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Status</th>
                <th>Capacity</th>
                <th>Access</th>
                <th>Class</th>
                <th>Used by</th>
                <th>Files</th>
                <th className="text-right">Age</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {rows.map((v) => {
                const bound = v.phase === "Bound";
                return (
                  <tr key={`${v.namespace}/${v.name}`}>
                    <td>
                      <div className="max-w-[260px] truncate font-medium text-content" title={v.name}>{v.name}</div>
                      <div className="text-[11px] text-content-muted">{v.namespace}</div>
                    </td>
                    <td><StatusPill status={v.phase} /></td>
                    <td className="tabular-nums text-content-secondary">{v.capacity ?? "—"}</td>
                    <td className="mono text-xs text-content-secondary" title={v.accessModes.join(", ")}>
                      {v.accessModes.map((m) => SHORT_MODES[m] ?? m).join(", ") || "—"}
                    </td>
                    <td className="text-xs text-content-secondary">{v.storageClass ?? "—"}</td>
                    <td>
                      <div className="flex max-w-[280px] flex-wrap gap-1">
                        {v.usedBy.map((u) => {
                          const [kind, name] = u.split("/");
                          return (
                            <button key={u} className="btn-chip" title={u} onClick={() => openWorkload({ kind, namespace: v.namespace, name })}>
                              {name}
                            </button>
                          );
                        })}
                        {/* Pods without a workload (bare pods); workload pods show as their workload. */}
                        {v.usedBy.length === 0 &&
                          v.mountedBy.map((pod) => (
                            <button key={pod} className="btn-chip" title="Pod" onClick={() => openPod(v.namespace, pod)}>
                              {pod}
                            </button>
                          ))}
                        {v.usedBy.length === 0 && v.mountedBy.length === 0 && <span className="text-xs text-content-muted">nothing</span>}
                      </div>
                    </td>
                    <td>
                      {v.writeBlockers.length === 0 ? (
                        <span className="tint-good inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px]" title="Nothing uses it, so files can be changed">
                          <ShieldCheck size={12} /> Writable
                        </span>
                      ) : (
                        <span className="tint-muted inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px]" title={v.writeBlockers.join("\n")}>
                          <Lock size={12} /> Read-only
                        </span>
                      )}
                    </td>
                    <td className="text-right text-xs text-content-muted">{fmtAge(v.createdMs, now)}</td>
                    <td className="whitespace-nowrap text-right">
                      <ActionMenu
                        label={`Actions for ${v.name}`}
                        items={[
                          {
                            label: "Browse files",
                            icon: <FolderOpen size={14} />,
                            disabled: !bound,
                            title: bound ? undefined : "Not bound to a volume yet",
                            onSelect: () => openFiles(v.namespace, v.name),
                          },
                          {
                            label: "Edit YAML",
                            icon: <FileCode2 size={14} />,
                            onSelect: () => openEditor({ kind: "PersistentVolumeClaim", namespace: v.namespace, name: v.name }),
                          },
                          {
                            label: "Delete storage…",
                            icon: <Trash2 size={14} />,
                            danger: true,
                            disabled: v.writeBlockers.length > 0,
                            title: v.writeBlockers.length ? `Still in use:\n${v.writeBlockers.join("\n")}` : "Delete the claim and its volume",
                            onSelect: () => void deleteVolumeClaim(v),
                          },
                        ]}
                      />
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}
