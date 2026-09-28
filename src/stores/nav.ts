import { create } from "zustand";
import type { ArchiveMeta, LogLevel, ManifestTarget, WorkloadRef } from "../lib/types";

export type Page =
  | "overview"
  | "problems"
  | "pods"
  | "workloads"
  | "services"
  | "config"
  | "nodes"
  | "events"
  | "logs"
  | "analytics"
  | "settings";

export type PodTab = "overview" | "logs" | "previous" | "events" | "yaml";
export type WorkloadTab = "overview" | "logs" | "events" | "yaml";

export interface LogPreset {
  namespace?: string;
  pod?: string;
  /** All pods of a workload, gone ones included (needs `namespace`). */
  workload?: WorkloadRef;
  levels?: LogLevel[];
  search?: string;
}

interface NavState {
  page: Page;
  /** Filters handed to the Logs page by a deep link; consumed on mount. */
  logPreset: LogPreset | null;
  /** Pod detail drawer. */
  pod: { namespace: string; name: string; tab: PodTab } | null;
  /** Node detail drawer. */
  node: string | null;
  /** Workload detail drawer; works for scaled-down, deleted and archived ones too. */
  workload: (ManifestTarget & { tab: WorkloadTab; archiveId?: string }) | null;
  /** Archive-a-workload dialog. */
  archiving: ManifestTarget | null;
  /** Restore-an-archive dialog. */
  restoring: ArchiveMeta | null;
  /** YAML editor drawer. */
  editor: ManifestTarget | null;
  /** Copy-to-cluster dialog. */
  copy: ManifestTarget | null;
  /** Export-with-related dialog. */
  exporting: ManifestTarget | null;
  /** ConfigMap / Secret key-value editor. */
  config: ManifestTarget | null;
  /** Start-port-forward dialog for a Service. */
  forward: { namespace: string; service: string } | null;
  go: (page: Page) => void;
  openLogs: (preset: LogPreset) => void;
  consumeLogPreset: () => LogPreset | null;
  openPod: (namespace: string, name: string, tab?: PodTab) => void;
  closePod: () => void;
  openNode: (name: string) => void;
  closeNode: () => void;
  /** `archiveId` pins a specific archive (e.g. one from another cluster). */
  openWorkload: (t: ManifestTarget & { archiveId?: string }, tab?: WorkloadTab) => void;
  closeWorkload: () => void;
  openArchive: (t: ManifestTarget) => void;
  closeArchive: () => void;
  openRestore: (a: ArchiveMeta) => void;
  closeRestore: () => void;
  openEditor: (t: ManifestTarget) => void;
  closeEditor: () => void;
  openCopy: (t: ManifestTarget) => void;
  closeCopy: () => void;
  openExport: (t: ManifestTarget) => void;
  closeExport: () => void;
  openConfig: (t: ManifestTarget) => void;
  closeConfig: () => void;
  openForward: (namespace: string, service: string) => void;
  closeForward: () => void;
}

export const useNavStore = create<NavState>((set, get) => ({
  page: "overview",
  logPreset: null,
  pod: null,
  node: null,
  workload: null,
  archiving: null,
  restoring: null,
  editor: null,
  copy: null,
  exporting: null,
  config: null,
  forward: null,
  go: (page) => set({ page }),
  openLogs: (preset) => set({ page: "logs", logPreset: preset, pod: null, node: null, workload: null }),
  consumeLogPreset: () => {
    const p = get().logPreset;
    if (p) set({ logPreset: null });
    return p;
  },
  openPod: (namespace, name, tab = "overview") => set({ pod: { namespace, name, tab }, node: null, workload: null }),
  closePod: () => set({ pod: null }),
  openNode: (name) => set({ node: name, pod: null, workload: null }),
  closeNode: () => set({ node: null }),
  openWorkload: (t, tab = "overview") =>
    set({ workload: { kind: t.kind, namespace: t.namespace, name: t.name, archiveId: t.archiveId, tab }, pod: null, node: null }),
  closeWorkload: () => set({ workload: null }),
  openArchive: (t) => set({ archiving: t }),
  closeArchive: () => set({ archiving: null }),
  openRestore: (a) => set({ restoring: a }),
  closeRestore: () => set({ restoring: null }),
  openEditor: (t) => set({ editor: t, pod: null, node: null, workload: null }),
  closeEditor: () => set({ editor: null }),
  openCopy: (t) => set({ copy: t }),
  closeCopy: () => set({ copy: null }),
  openExport: (t) => set({ exporting: t }),
  closeExport: () => set({ exporting: null }),
  openConfig: (t) => set({ config: t, pod: null, node: null, editor: null, workload: null }),
  closeConfig: () => set({ config: null }),
  openForward: (namespace, service) => set({ forward: { namespace, service } }),
  closeForward: () => set({ forward: null }),
}));
