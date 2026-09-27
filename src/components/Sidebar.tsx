import {
  Activity,
  AlertTriangle,
  BarChart3,
  Boxes,
  FileKey,
  Network,
  Layers,
  LayoutDashboard,
  Moon,
  ScrollText,
  Server,
  Settings,
  Sun,
  type LucideIcon,
} from "lucide-react";
import ClusterSwitcher from "./ClusterSwitcher";
import { useClusterStore } from "../stores/cluster";
import { useNavStore, type Page } from "../stores/nav";
import { useThemeStore } from "../stores/theme";

const ITEMS: { page: Page; label: string; Icon: LucideIcon }[] = [
  { page: "overview", label: "Overview", Icon: LayoutDashboard },
  { page: "problems", label: "Problems", Icon: AlertTriangle },
  { page: "pods", label: "Pods", Icon: Boxes },
  { page: "workloads", label: "Workloads", Icon: Layers },
  { page: "services", label: "Services", Icon: Network },
  { page: "config", label: "Config", Icon: FileKey },
  { page: "nodes", label: "Nodes", Icon: Server },
  { page: "events", label: "Events", Icon: Activity },
  { page: "logs", label: "Logs", Icon: ScrollText },
  { page: "analytics", label: "Analytics", Icon: BarChart3 },
];

export default function Sidebar() {
  const { page, go } = useNavStore();
  const issues = useClusterStore((s) => s.snapshot?.issues);
  const { resolved, setPref } = useThemeStore();
  const critical = issues?.filter((i) => i.severity === "critical").length ?? 0;
  const warning = issues?.filter((i) => i.severity === "warning").length ?? 0;

  const item = (p: Page, label: string, Icon: LucideIcon) => {
    const active = page === p;
    return (
      <button
        key={p}
        onClick={() => go(p)}
        className={`group flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm transition-colors ${
          active ? "bg-muted font-medium text-content" : "text-content-secondary hover:bg-muted hover:text-content"
        }`}
      >
        <Icon size={16} className={active ? "text-accent" : "text-content-muted group-hover:text-content-secondary"} />
        <span className="flex-1 text-left">{label}</span>
        {p === "problems" && critical + warning > 0 && (
          <span
            className={`rounded px-1.5 text-[11px] font-semibold tabular-nums ${critical ? "tint-critical" : "tint-warning"}`}
            title={`${critical} critical, ${warning} warning`}
          >
            {critical + warning}
          </span>
        )}
      </button>
    );
  };

  return (
    <nav className="flex w-52 shrink-0 flex-col border-r border-border-light bg-surface px-2 py-3">
      <div className="mb-4 flex items-center gap-2 px-2.5">
        <img src="/icon.png" alt="" className="h-6 w-6" />
        <div className="leading-tight">
          <div className="text-sm font-semibold text-content">Portside</div>
          <div className="text-[10px] font-medium uppercase tracking-widest text-brand-pink">lite</div>
        </div>
      </div>
      <ClusterSwitcher />
      <div className="flex flex-col gap-0.5">{ITEMS.map((i) => item(i.page, i.label, i.Icon))}</div>
      <div className="mt-auto flex flex-col gap-0.5 border-t border-border-light pt-2">
        {item("settings", "Settings", Settings)}
        <button
          onClick={() => setPref(resolved === "dark" ? "light" : "dark")}
          className="flex w-full items-center gap-2.5 rounded-md px-2.5 py-1.5 text-sm text-content-secondary hover:bg-muted hover:text-content"
        >
          {resolved === "dark" ? <Sun size={16} className="text-content-muted" /> : <Moon size={16} className="text-content-muted" />}
          {resolved === "dark" ? "Light theme" : "Dark theme"}
        </button>
      </div>
    </nav>
  );
}
