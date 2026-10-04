import { useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Copy, WrapText } from "lucide-react";
import { fmtDateTime } from "../lib/format";
import type { LogLevel } from "../lib/types";
import { toast } from "../stores/toast";

export interface LogRow {
  key: string | number;
  tsMs: number;
  level: LogLevel;
  message: string;
  source?: string;
}

const LEVEL_VAR: Record<LogLevel, string> = {
  error: "var(--lvl-error)",
  warning: "var(--lvl-warning)",
  info: "var(--lvl-info)",
  debug: "var(--lvl-debug)",
  trace: "var(--lvl-debug)",
};

const LEVEL_TAG: Record<LogLevel, string> = { error: "ERR", warning: "WRN", info: "INF", debug: "DBG", trace: "TRC" };

const ROW_H = 22;

/**
 * Virtualized log list. Rows are fixed-height single lines; click one to see
 * the full message in the detail pane below.
 */
export default function LogView({
  rows,
  showSource = false,
  onEndReached,
  stickToBottom = false,
  emptyText = "No log lines.",
}: {
  rows: LogRow[];
  showSource?: boolean;
  onEndReached?: () => void;
  /** Keep scrolled to the last row (tail mode); otherwise newest is first. */
  stickToBottom?: boolean;
  emptyText?: string;
}) {
  const parentRef = useRef<HTMLDivElement>(null);
  const [selected, setSelected] = useState<LogRow | null>(null);
  const [wrap, setWrap] = useState(false);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => parentRef.current,
    estimateSize: () => ROW_H,
    overscan: 20,
  });

  useEffect(() => {
    if (stickToBottom && rows.length) virtualizer.scrollToIndex(rows.length - 1, { align: "end" });
  }, [rows, stickToBottom, virtualizer]);

  const items = virtualizer.getVirtualItems();
  useEffect(() => {
    const last = items[items.length - 1];
    if (onEndReached && last && last.index >= rows.length - 1 && rows.length > 0) onEndReached();
  }, [items, rows.length, onEndReached]);

  if (!rows.length) {
    return <div className="flex h-full items-center justify-center text-xs text-content-muted">{emptyText}</div>;
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div ref={parentRef} className="min-h-0 flex-1 overflow-auto font-mono text-[12px]">
        <div style={{ height: virtualizer.getTotalSize(), position: "relative", minWidth: "100%" }}>
          {items.map((v) => {
            const r = rows[v.index];
            const isSel = selected?.key === r.key;
            return (
              <div
                key={r.key}
                onClick={() => setSelected(isSel ? null : r)}
                className={`absolute left-0 flex w-full cursor-pointer items-center gap-2 whitespace-pre px-3 ${
                  isSel ? "bg-muted" : "hover:bg-muted"
                }`}
                style={{
                  top: v.start,
                  height: ROW_H,
                  backgroundColor:
                    !isSel && r.level === "error" ? "color-mix(in srgb, var(--lvl-error) 7%, transparent)" : undefined,
                }}
              >
                <span className="h-3 w-[3px] shrink-0 rounded-full" style={{ background: LEVEL_VAR[r.level] }} />
                <span className="shrink-0 text-content-muted">{fmtDateTime(r.tsMs)}</span>
                <span className="w-7 shrink-0 font-semibold text-content-secondary">{LEVEL_TAG[r.level]}</span>
                {showSource && r.source && (
                  <span className="max-w-[220px] shrink-0 truncate text-content-muted" title={r.source}>
                    {r.source}
                  </span>
                )}
                <span className="truncate text-content">{r.message}</span>
              </div>
            );
          })}
        </div>
      </div>
      {selected && (
        <div className="max-h-[40%] shrink-0 overflow-auto border-t border-border bg-raised px-4 py-3">
          <div className="mb-2 flex items-center gap-3 text-xs text-content-muted">
            <span>{fmtDateTime(selected.tsMs)}</span>
            <span className="font-semibold uppercase text-content-secondary">{selected.level}</span>
            {selected.source && <span className="mono">{selected.source}</span>}
            <div className="ml-auto flex gap-1">
              <button className="btn-quiet" onClick={() => setWrap((w) => !w)} title="Toggle wrap / pretty JSON">
                <WrapText size={14} />
              </button>
              <button className="btn-quiet" onClick={() => void navigator.clipboard.writeText(selected.message).catch(() => toast.error("Couldn't copy to the clipboard"))} title="Copy">
                <Copy size={14} />
              </button>
            </div>
          </div>
          <pre className={`mono text-content ${wrap ? "whitespace-pre" : "whitespace-pre-wrap break-words"}`}>
            {wrap ? prettyJson(selected.message) : selected.message}
          </pre>
        </div>
      )}
    </div>
  );
}

function prettyJson(s: string): string {
  const t = s.trim();
  if (!t.startsWith("{") && !t.startsWith("[")) return s;
  try {
    return JSON.stringify(JSON.parse(t), null, 2);
  } catch {
    return s;
  }
}
