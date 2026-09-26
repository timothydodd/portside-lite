import { Clock, Lightbulb } from "lucide-react";
import { ACTION_LABELS, runAction } from "../lib/actions";
import { fmtAgo } from "../lib/format";
import type { Issue } from "../lib/types";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import { SeverityIcon } from "./ui";

export default function IssueCard({ issue, compact = false }: { issue: Issue; compact?: boolean }) {
  const { openPod, openNode, openLogs } = useNavStore();
  const workloads = useClusterStore((s) => s.snapshot?.workloads);

  const open = () => {
    if (issue.kind === "Pod" && issue.namespace) openPod(issue.namespace, issue.name);
    else if (issue.kind === "Node") openNode(issue.name);
  };
  const clickable = issue.kind === "Pod" || issue.kind === "Node";
  const replicas = workloads?.find(
    (w) => w.kind === issue.kind && w.namespace === issue.namespace && w.name === issue.name,
  )?.desired;
  const border =
    issue.severity === "critical" ? "border-l-critical" : issue.severity === "warning" ? "border-l-warning" : "border-l-info";

  return (
    <div className={`card border-l-[3px] ${border} px-4 py-3`}>
      <div className="flex items-start gap-3">
        <SeverityIcon severity={issue.severity} size={18} />
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-baseline gap-x-2">
            <button
              className={`text-left text-sm font-semibold text-content ${clickable ? "hover:text-accent" : "cursor-default"}`}
              onClick={clickable ? open : undefined}
            >
              {issue.title}
            </button>
            <span className="text-xs text-content-muted">
              {issue.kind}
              {issue.namespace && ` · ${issue.namespace}`}
            </span>
          </div>
          <p className="mt-1 break-words text-[13px] text-content-secondary">{issue.detail}</p>
          {!compact && issue.hint && (
            <p className="mt-1.5 flex items-start gap-1.5 text-xs text-content-muted">
              <Lightbulb size={13} className="mt-px shrink-0" />
              {issue.hint}
            </p>
          )}
          <div className="mt-2 flex flex-wrap items-center gap-1.5">
            {issue.actions.map((a) => (
              <button
                key={a}
                className="btn-chip"
                onClick={() => {
                  if (a === "viewLogs" && issue.rule === "log-errors" && issue.namespace) {
                    openLogs({ namespace: issue.namespace, pod: issue.name, levels: ["error"] });
                  } else {
                    void runAction(a, { kind: issue.kind, namespace: issue.namespace, name: issue.name, replicas });
                  }
                }}
              >
                {ACTION_LABELS[a]}
              </button>
            ))}
            <span className="ml-auto inline-flex items-center gap-1 text-[11px] text-content-muted" title="When Portside first saw this problem">
              <Clock size={11} />
              {issue.sinceMs ? `since ${fmtAgo(issue.sinceMs)}` : issue.firstSeenMs ? `seen ${fmtAgo(issue.firstSeenMs)}` : ""}
            </span>
          </div>
        </div>
      </div>
    </div>
  );
}
