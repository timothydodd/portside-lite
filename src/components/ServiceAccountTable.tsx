import { useMemo } from "react";
import { AlertTriangle, Copy, Download, FileCode2, Trash2 } from "lucide-react";
import { fmtAge } from "../lib/format";
import type { ServiceAccountInfo } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { ActionMenu, EmptyState } from "./ui";
import { useDeleteDialog } from "../lib/actions";

const SYSTEM_NAMESPACES = ["kube-system", "kube-public", "kube-node-lease"];

/** Control-plane accounts, and `default` accounts nobody configured. */
export function isSystemAccount(a: ServiceAccountInfo): boolean {
  if (SYSTEM_NAMESPACES.includes(a.namespace)) return true;
  return a.name === "default" && a.imagePullSecrets.length === 0 && a.roles.length === 0;
}

function Lines({ items, empty, max = 3 }: { items: string[]; empty: string; max?: number }) {
  if (!items.length) return <span className="text-content-muted">{empty}</span>;
  return (
    <div className="flex max-w-[300px] flex-col gap-0.5" title={items.join("\n")}>
      {items.slice(0, max).map((i) => (
        <span key={i} className="truncate text-content-secondary">{i}</span>
      ))}
      {items.length > max && <span className="text-content-muted">+{items.length - max} more</span>}
    </div>
  );
}

export default function ServiceAccountTable(p: { accounts: ServiceAccountInfo[]; namespace: string; search: string; hideSystem: boolean }) {
  const { openEditor, openExport, openCopy } = useNavStore();
  const rows = useMemo(() => {
    const q = p.search.toLowerCase();
    return p.accounts
      .filter((a) => (!p.hideSystem || !isSystemAccount(a)) && (!p.namespace || a.namespace === p.namespace))
      .filter((a) => !q || `${a.name} ${a.imagePullSecrets.join(" ")} ${a.roles.join(" ")} ${a.usedBy.join(" ")}`.toLowerCase().includes(q))
      .sort(
        (a, b) =>
          Number(b.missingPullSecrets.length > 0) - Number(a.missingPullSecrets.length > 0) ||
          a.namespace.localeCompare(b.namespace) ||
          a.name.localeCompare(b.name),
      );
  }, [p.accounts, p.namespace, p.search, p.hideSystem]);
  const now = Date.now();

  if (rows.length === 0) {
    return (
      <EmptyState title="No ServiceAccounts">
        {p.hideSystem && "System accounts (kube-system, and default accounts nobody configured) are hidden; untick “Hide system” to see them."}
      </EmptyState>
    );
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Pull secrets</th>
            <th>Permissions</th>
            <th>Used by</th>
            <th className="text-right">Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {rows.map((a) => {
            const ref = { kind: "ServiceAccount", namespace: a.namespace, name: a.name };
            return (
              <tr key={`${a.namespace}/${a.name}`}>
                <td className="align-top">
                  <div className="max-w-[240px] truncate font-medium text-content" title={a.name}>{a.name}</div>
                  <div className="text-[11px] text-content-muted">
                    {a.namespace}
                    {a.automountToken === false && " · token not mounted"}
                  </div>
                </td>
                <td className="align-top text-xs">
                  {a.imagePullSecrets.length ? (
                    <div className="flex flex-col gap-0.5">
                      {a.imagePullSecrets.map((s) => {
                        const missing = a.missingPullSecrets.includes(s);
                        return (
                          <span key={s} className={`mono flex items-center gap-1 ${missing ? "text-critical" : "text-content-secondary"}`}>
                            {s}
                            {missing && <AlertTriangle size={11} className="shrink-0" aria-label="Secret missing" />}
                            {missing && <span className="text-[11px]">missing</span>}
                          </span>
                        );
                      })}
                    </div>
                  ) : (
                    <span className="text-content-muted">none</span>
                  )}
                </td>
                <td className="align-top text-xs">
                  <Lines items={a.roles} empty="no bindings" />
                </td>
                <td className="align-top text-xs">
                  <Lines items={a.usedBy} empty="unused" />
                </td>
                <td className="align-top text-right text-xs text-content-muted">{fmtAge(a.createdMs, now)}</td>
                <td className="align-top text-right">
                  <ActionMenu
                    items={[
                      { label: "Edit YAML", icon: <FileCode2 size={14} />, onSelect: () => openEditor(ref) },
                      { label: "Export…", icon: <Download size={14} />, title: "Export with its pull secrets, RoleBindings and Roles", onSelect: () => openExport(ref) },
                      { label: "Copy to…", icon: <Copy size={14} />, title: "Copy to another cluster or namespace", onSelect: () => openCopy(ref) },
                      {
                        label: "Delete ServiceAccount…",
                        icon: <Trash2 size={14} />,
                        danger: true,
                        disabled: a.name === "default",
                        title: a.name === "default" ? "The cluster recreates the default account; remove its bindings or pull secrets instead" : undefined,
                        onSelect: () => useDeleteDialog.getState().open({ ...ref, dependents: a.usedBy }),
                      },
                    ]}
                  />
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
