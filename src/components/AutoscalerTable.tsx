import { useMemo } from "react";
import { FileCode2 } from "lucide-react";
import { fmtAge, fmtAgo } from "../lib/format";
import type { AutoscalerInfo } from "../lib/types";
import { useNavStore } from "../stores/nav";
import { EmptyState, StatusPill } from "./ui";

function state(h: AutoscalerInfo): { label: string; tone: "good" | "warning" | "critical" | "muted"; rank: number } {
  if (h.problem) return { label: "Can't scale", tone: "critical", rank: 0 };
  if (h.atMax) return { label: "At maximum", tone: "warning", rank: 1 };
  return { label: "Scaling", tone: "good", rank: 2 };
}

export default function AutoscalerTable({ rows }: { rows: AutoscalerInfo[] }) {
  const { openEditor, openWorkload } = useNavStore();
  const sorted = useMemo(() => [...rows].sort((a, b) => state(a).rank - state(b).rank || a.name.localeCompare(b.name)), [rows]);
  const now = Date.now();

  if (sorted.length === 0) {
    return <EmptyState title="No HorizontalPodAutoscalers">Nothing matches the namespace and search filter, or no workload scales on its own.</EmptyState>;
  }
  return (
    <div className="min-h-0 flex-1 overflow-auto">
      <table className="table">
        <thead>
          <tr>
            <th>Name</th>
            <th>Namespace</th>
            <th>Target</th>
            <th>State</th>
            <th>Replicas</th>
            <th>Metrics (now / target)</th>
            <th className="text-right">Last scaled</th>
            <th className="text-right">Age</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {sorted.map((h) => {
            const st = state(h);
            return (
              <tr key={`${h.namespace}/${h.name}`}>
                <td className="font-medium text-content">{h.name}</td>
                <td className="text-content-secondary">{h.namespace}</td>
                <td>
                  <button className="btn-chip" title={`${h.targetKind}/${h.targetName}`} onClick={() => openWorkload({ kind: h.targetKind, namespace: h.namespace, name: h.targetName })}>
                    {h.targetName}
                  </button>
                </td>
                <td>
                  <StatusPill status={st.label} tone={st.tone} />
                  {h.problem && <div className="max-w-xs truncate text-[11px] text-content-muted" title={h.problem}>{h.problem}</div>}
                </td>
                <td className="tabular-nums" title={`Wants ${h.desiredReplicas}`}>
                  {h.currentReplicas} <span className="text-xs text-content-muted">({h.minReplicas}–{h.maxReplicas})</span>
                </td>
                <td className="mono text-xs text-content-secondary">
                  {h.metrics.map((m) => (
                    <div key={m}>{m}</div>
                  ))}
                  {h.metrics.length === 0 && "—"}
                </td>
                <td className="text-right text-xs text-content-muted">{fmtAgo(h.lastScaleMs)}</td>
                <td className="text-right text-xs text-content-muted">{fmtAge(h.createdMs, now)}</td>
                <td className="whitespace-nowrap text-right">
                  <button className="btn-quiet" title="Edit YAML" onClick={() => openEditor({ kind: "HorizontalPodAutoscaler", namespace: h.namespace, name: h.name })}>
                    <FileCode2 size={14} />
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
