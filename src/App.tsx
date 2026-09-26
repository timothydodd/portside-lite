import { useEffect } from "react";
import TitleBar from "./components/TitleBar";
import Sidebar from "./components/Sidebar";
import StatusBar from "./components/StatusBar";
import { DeleteDialog, ScaleDialog, Toasts } from "./components/Dialogs";
import ErrorBoundary from "./components/ErrorBoundary";
import PodDrawer from "./components/PodDrawer";
import NodeDrawer from "./components/NodeDrawer";
import ManifestEditor from "./components/ManifestEditor";
import CopyDialog from "./components/CopyDialog";
import ImportDialog from "./components/ImportDialog";
import OverviewPage from "./pages/OverviewPage";
import ProblemsPage from "./pages/ProblemsPage";
import PodsPage from "./pages/PodsPage";
import WorkloadsPage from "./pages/WorkloadsPage";
import NodesPage from "./pages/NodesPage";
import EventsPage from "./pages/EventsPage";
import LogsPage from "./pages/LogsPage";
import AnalyticsPage from "./pages/AnalyticsPage";
import SettingsPage from "./pages/SettingsPage";
import { useClusterStore } from "./stores/cluster";
import { useNavStore } from "./stores/nav";
import { Pause } from "lucide-react";

/** Shown while monitoring is paused so stale data is never mistaken for live. */
function PausedBanner() {
  const paused = useClusterStore((s) => s.settings?.monitoringPaused);
  const setPaused = useClusterStore((s) => s.setPaused);
  if (!paused) return null;
  return (
    <div className="tint-warning sticky top-0 z-20 flex items-center gap-2 px-6 py-2 text-xs">
      <Pause size={14} />
      <span className="flex-1">Monitoring is paused: nothing is being polled, pulled or checked, and what you see isn't updating.</span>
      <button className="btn-chip !border-warning !text-warning" onClick={() => void setPaused(false)}>
        Resume monitoring
      </button>
    </div>
  );
}

export default function App() {
  const init = useClusterStore((s) => s.init);
  const settings = useClusterStore((s) => s.settings);
  const { page, go, pod, node, editor, copy, closePod, closeNode, closeEditor, closeCopy } = useNavStore();

  useEffect(() => {
    void init();
  }, [init]);

  // First run: nothing configured yet → straight to Settings.
  useEffect(() => {
    if (settings && settings.connections.length === 0) go("settings");
  }, [settings, go]);

  return (
    <div className="flex h-full flex-col">
      <TitleBar />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="min-w-0 flex-1 overflow-auto">
          <PausedBanner />
          <ErrorBoundary resetKey={page}>
            {page === "overview" && <OverviewPage />}
            {page === "problems" && <ProblemsPage />}
            {page === "pods" && <PodsPage />}
            {page === "workloads" && <WorkloadsPage />}
            {page === "nodes" && <NodesPage />}
            {page === "events" && <EventsPage />}
            {page === "logs" && <LogsPage />}
            {page === "analytics" && <AnalyticsPage />}
            {page === "settings" && <SettingsPage />}
          </ErrorBoundary>
        </main>
      </div>
      <StatusBar />
      {pod && (
        <ErrorBoundary resetKey={`${pod.namespace}/${pod.name}/${pod.tab}`} onReset={closePod} className="fixed inset-y-0 right-0 z-40 w-[min(920px,92vw)] border-l border-border bg-surface shadow-[var(--shadow-md)]">
          <PodDrawer />
        </ErrorBoundary>
      )}
      {node && (
        <ErrorBoundary resetKey={node} onReset={closeNode} className="fixed inset-y-0 right-0 z-40 w-[min(920px,92vw)] border-l border-border bg-surface shadow-[var(--shadow-md)]">
          <NodeDrawer />
        </ErrorBoundary>
      )}
      {editor && (
        <ErrorBoundary resetKey={`${editor.kind}/${editor.namespace}/${editor.name}`} onReset={closeEditor} className="fixed inset-y-0 right-0 z-40 w-[min(920px,92vw)] border-l border-border bg-surface shadow-[var(--shadow-md)]">
          <ManifestEditor />
        </ErrorBoundary>
      )}
      {copy && (
        <ErrorBoundary resetKey={`${copy.kind}/${copy.namespace}/${copy.name}`} onReset={closeCopy} className="fixed left-1/2 top-1/2 z-50 w-[min(640px,94vw)] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-surface shadow-[var(--shadow-md)]">
          <CopyDialog />
        </ErrorBoundary>
      )}
      <ScaleDialog />
      <DeleteDialog />
      <ErrorBoundary resetKey="import">
        <ImportDialog />
      </ErrorBoundary>
      <Toasts />
    </div>
  );
}
