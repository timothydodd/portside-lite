import { useMemo } from "react";
import { FileCode2 } from "lucide-react";
import { fmtAge } from "../lib/format";
import type { PersistentVolumeInfo, StorageClassInfo } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { EmptyState, StatusPill } from "./ui";

export const SHORT_MODES: Record<string, string> = {
  ReadWriteOnce: "RWO",
  ReadOnlyMany: "ROX",
  ReadWriteMany: "RWX",
  ReadWriteOncePod: "RWOP",
};

function pvTone(v: PersistentVolumeInfo): "good" | "warning" | "critical" | "muted" {
  if (v.phase === "Bound") return "good";
  if (v.phase === "Failed") return "critical";
  if (v.phase === "Released") return "warning";
  return "muted";
}

const PV_RANK: Record<string, number> = { Failed: 0, Released: 1, Pending: 2, Available: 3, Bound: 4 };

/** PersistentVolumes. They're cluster-wide; the namespace filter matches the claim's namespace. */
export function PersistentVolumeTable(p: { volumes: PersistentVolumeInfo[]; namespace: string; search: string }) {
  const openEditor = useNavStore((s) => s.openEditor);
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.volumes
      .filter(
        (v) =>
          (!p.namespace || v.claim?.startsWith(`${p.namespace}/`)) &&
          (!q || `${v.name} ${v.claim ?? ""} ${v.storageClass ?? ""} ${v.node ?? ""}`.toLowerCase().includes(q)),
      )
      .sort((a, b) => (PV_RANK[a.phase] ?? 2) - (PV_RANK[b.phase] ?? 2) || (a.claim ?? a.name).localeCompare(b.claim ?? b.name));
  }, [p.volumes, p.namespace, p.search]);
  const now = Date.now();

  if (rows.length === 0) {
    return <EmptyState title="No PersistentVolumes">{p.volumes.length ? "Nothing matches the filter." : "No volumes exist, or this connection isn't allowed to list them."}</EmptyState>;
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Status</th>
            <th>Claim</th>
            <th>Capacity</th>
            <th>Access</th>
            <th>Class</th>
            <th>Reclaim</th>
            <th>Where</th>
            <th className="text-right">Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((v) => (
            <tr key={v.name}>
              <td>
                <div className="mono max-w-[220px] truncate text-xs text-content" title={v.name}>{v.name}</div>
              </td>
              <td>
                <StatusPill status={v.phase} tone={pvTone(v)} />
                {v.message && <div className="max-w-xs truncate text-[11px] text-content-muted" title={v.message}>{v.message}</div>}
              </td>
              <td className="text-xs text-content-secondary">
                <div className="max-w-[220px] truncate" title={v.claim ?? undefined}>{v.claim ?? "—"}</div>
              </td>
              <td className="tabular-nums text-content-secondary">{v.capacity ?? "—"}</td>
              <td className="mono text-xs text-content-secondary" title={v.accessModes.join(", ")}>
                {v.accessModes.map((m) => SHORT_MODES[m] ?? m).join(", ") || "—"}
              </td>
              <td className="text-xs text-content-secondary">{v.storageClass ?? "—"}</td>
              <td className="text-xs text-content-secondary">{v.reclaimPolicy ?? "—"}</td>
              <td className="text-xs text-content-secondary">
                {v.node && <div>{v.node}</div>}
                <div className="mono max-w-[260px] truncate text-[11px] text-content-muted" title={v.source ?? undefined}>{v.source ?? "—"}</div>
              </td>
              <td className="text-right text-xs text-content-muted">{fmtAge(v.createdMs, now)}</td>
              <td className="whitespace-nowrap text-right">
                <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: "PersistentVolume", namespace: "", name: v.name })}>
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

export function StorageClassTable(p: { classes: StorageClassInfo[]; search: string }) {
  const openEditor = useNavStore((s) => s.openEditor);
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.classes
      .filter((c) => !q || `${c.name} ${c.provisioner}`.toLowerCase().includes(q))
      .sort((a, b) => Number(b.isDefault) - Number(a.isDefault) || a.name.localeCompare(b.name));
  }, [p.classes, p.search]);
  const now = Date.now();

  if (rows.length === 0) {
    return <EmptyState title="No StorageClasses">{p.classes.length ? "Nothing matches the filter." : "No classes exist, or this connection isn't allowed to list them."}</EmptyState>;
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Provisioner</th>
            <th>Reclaim</th>
            <th>Binding</th>
            <th>Expandable</th>
            <th>Volumes</th>
            <th>Claims</th>
            <th className="text-right">Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((c) => (
            <tr key={c.name}>
              <td>
                <span className="font-medium text-content">{c.name}</span>
                {c.isDefault && <span className="tint-info ml-2 rounded px-1.5 py-0.5 text-[11px]">default</span>}
              </td>
              <td className="mono text-xs text-content-secondary">{c.provisioner}</td>
              <td className="text-xs text-content-secondary">{c.reclaimPolicy ?? "Delete"}</td>
              <td className="text-xs text-content-secondary">{c.bindingMode ?? "Immediate"}</td>
              <td className="text-xs text-content-secondary">{c.allowExpansion ? "Yes" : "No"}</td>
              <td className="tabular-nums text-content-secondary">{c.volumes}</td>
              <td className="tabular-nums text-content-secondary">{c.claims}</td>
              <td className="text-right text-xs text-content-muted">{fmtAge(c.createdMs, now)}</td>
              <td className="whitespace-nowrap text-right">
                <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: "StorageClass", namespace: "", name: c.name })}>
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
