import { create } from "zustand";
import type { LogLevel, ManifestTarget } from "../lib/types";

export type Page =
  | "overview"
  | "problems"
  | "pods"
  | "workloads"
  | "nodes"
  | "events"
  | "logs"
  | "analytics"
  | "settings";

export type PodTab = "overview" | "logs" | "previous" | "events" | "yaml";

export interface LogPreset {
  namespace?: string;
  pod?: string;
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
  /** YAML editor drawer. */
  editor: ManifestTarget | null;
  /** Copy-to-cluster dialog. */
  copy: ManifestTarget | null;
  go: (page: Page) => void;
  openLogs: (preset: LogPreset) => void;
  consumeLogPreset: () => LogPreset | null;
  openPod: (namespace: string, name: string, tab?: PodTab) => void;
  closePod: () => void;
  openNode: (name: string) => void;
  closeNode: () => void;
  openEditor: (t: ManifestTarget) => void;
  closeEditor: () => void;
  openCopy: (t: ManifestTarget) => void;
  closeCopy: () => void;
}

export const useNavStore = create<NavState>((set, get) => ({
  page: "overview",
  logPreset: null,
  pod: null,
  node: null,
  editor: null,
  copy: null,
  go: (page) => set({ page }),
  openLogs: (preset) => set({ page: "logs", logPreset: preset, pod: null, node: null }),
  consumeLogPreset: () => {
    const p = get().logPreset;
    if (p) set({ logPreset: null });
    return p;
  },
  openPod: (namespace, name, tab = "overview") => set({ pod: { namespace, name, tab }, node: null }),
  closePod: () => set({ pod: null }),
  openNode: (name) => set({ node: name, pod: null }),
  closeNode: () => set({ node: null }),
  openEditor: (t) => set({ editor: t, pod: null, node: null }),
  closeEditor: () => set({ editor: null }),
  openCopy: (t) => set({ copy: t }),
  closeCopy: () => set({ copy: null }),
}));
