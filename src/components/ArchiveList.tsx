import { useEffect, useMemo, useState } from "react";
import { ArchiveRestore, FolderOpen, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import { confirmDestructive } from "../lib/dialog";
import { errorMessage, fmtAgo, fmtCount, fmtDateTime } from "../lib/format";
import type { ArchiveMeta } from "../lib/types";
import { useArchivesStore } from "../stores/archives";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { toast } from "../stores/toast";
import { ActionMenu, EmptyState, Spinner, StatusPill } from "./ui";

/** Archived workloads, read from the archive folder. */
export default function ArchiveList({ namespace, search }: { namespace: string; search: string }) {
  const archives = useArchivesStore((s) => s.archives);
  const error = useArchivesStore((s) => s.error);
  const load = useArchivesStore((s) => s.load);
  const clusterId = useClusterStore((s) => s.status?.clusterId ?? s.snapshot?.clusterId);
  const workloads = useClusterStore((s) => s.snapshot?.workloads);
  const { openWorkload, openRestore } = useNavStore();
  const [allClusters, setAllClusters] = useState(false);

  useEffect(() => void load(), [load]);

  const rows = useMemo(
    () =>
      (archives ?? []).filter(
        (a) =>
          (allClusters || a.clusterId === clusterId) &&
          (!namespace || a.namespace === namespace) &&
          (!search || a.name.toLowerCase().includes(search.toLowerCase())),
      ),
    [archives, allClusters, clusterId, namespace, search],
  );
  const elsewhere = (archives ?? []).filter((a) => a.clusterId !== clusterId).length;

  const status = (a: ArchiveMeta): { label: string; tone: "good" | "muted" | "warning" } => {
    const live = a.clusterId === clusterId && workloads?.some((w) => w.kind === a.kind && w.namespace === a.namespace && w.name === a.name);
    if (live) return { label: a.restoredMs ? "Restored" : "On the cluster", tone: "good" };
    if (a.objects.some((o) => o.error)) return { label: "Archived (partly removed)", tone: "warning" };
    return { label: "Archived", tone: "muted" };
  };

  const remove = async (a: ArchiveMeta) => {
    const ok = await confirmDestructive(
      `Delete the archive of ${a.kind} ${a.namespace}/${a.name}?\n\nIts folder (manifest.yaml${a.logLines ? ", logs.txt" : ""}) is deleted from disk. This can't be undone and doesn't touch the cluster.`,
      "Delete archive",
    );
    if (!ok) return;
    try {
      await ipc.deleteArchive(a.id);
      toast.success(`Deleted the archive of ${a.name}`);
    } catch (e) {
      toast.error(errorMessage(e));
    }
    void load();
  };

  if (archives == null) return <EmptyState icon={<Spinner />} title="Reading archives…" />;

  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <div className="flex items-center gap-3 px-6 py-2 text-xs text-content-muted">
        <span className="flex-1">
          Saved to a local folder and removed from the cluster. Restore deploys one again; the archive stays until you delete it.
        </span>
        {elsewhere > 0 && (
          <label className="flex cursor-pointer items-center gap-1.5">
            <input type="checkbox" className="accent-brand" checked={allClusters} onChange={(e) => setAllClusters(e.target.checked)} />
            Show other clusters' ({elsewhere})
          </label>
        )}
      </div>
      {error && <p className="px-6 text-xs text-critical">{error}</p>}
      {rows.length === 0 ? (
        (archives.length > 0 && (namespace || search)) || elsewhere > 0 ? (
          <EmptyState title="No archives match">
            {namespace || search ? "Nothing fits the namespace or search filter." : "There are archives from other clusters; tick the box above to see them."}
          </EmptyState>
        ) : (
          <EmptyState title="Nothing archived">
            Use Archive in a workload's actions menu (or in its overview) to save it locally and take it off the cluster.
          </EmptyState>
        )
      ) : (
        <table className="table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Kind</th>
              <th>Namespace</th>
              {allClusters && <th>Cluster</th>}
              <th>Status</th>
              <th className="text-right">Objects</th>
              <th className="text-right">Log lines</th>
              <th className="text-right">Archived</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map((a) => {
              const st = status(a);
              return (
                <tr key={a.id} className="cursor-pointer" onClick={() => openWorkload({ kind: a.kind, namespace: a.namespace, name: a.name, archiveId: a.id })}>
                  <td className="font-medium text-content">{a.name}</td>
                  <td className="text-content-secondary">{a.kind}</td>
                  <td className="text-content-secondary">{a.namespace}</td>
                  {allClusters && <td className="text-content-secondary">{a.connectionName}</td>}
                  <td>
                    <StatusPill status={st.label} tone={st.tone} />
                    {a.restoredMs && (
                      <div className="text-[11px] text-content-muted">
                        restored {fmtAgo(a.restoredMs)}
                        {a.restoredTo && ` to ${a.restoredTo}`}
                      </div>
                    )}
                  </td>
                  <td className="text-right tabular-nums" title={a.objects.map((o) => `${o.kind}/${o.name}`).join("\n")}>
                    {a.objects.length}
                  </td>
                  <td className="text-right tabular-nums text-content-muted">{a.logLines ? fmtCount(a.logLines) : "—"}</td>
                  <td className="whitespace-nowrap text-right text-xs text-content-muted" title={fmtDateTime(a.archivedMs)}>
                    {fmtAgo(a.archivedMs)}
                  </td>
                  <td className="whitespace-nowrap text-right" onClick={(e) => e.stopPropagation()}>
                    <ActionMenu
                      label={`Actions for ${a.name}`}
                      items={[
                        { label: "Restore…", icon: <ArchiveRestore size={14} />, title: "Deploy it again", onSelect: () => openRestore(a) },
                        {
                          label: "Open archive folder",
                          icon: <FolderOpen size={14} />,
                          onSelect: () => void ipc.openArchiveFolder(a.id).catch((e) => toast.error(errorMessage(e))),
                        },
                        { label: "Delete archive…", icon: <Trash2 size={14} />, danger: true, title: "Delete the archive from disk", onSelect: () => void remove(a) },
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
  );
}
