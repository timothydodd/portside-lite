import { create } from "zustand";
import { listen } from "@tauri-apps/api/event";
import * as ipc from "../lib/ipc";
import type { ClusterSnapshot, ConnectionProfile, ForwardInfo, Settings, Status } from "../lib/types";

interface ClusterState {
  snapshot: ClusterSnapshot | null;
  status: Status | null;
  settings: Settings | null;
  /** Bumps whenever a log sync lands, so log views can re-query. */
  logSyncTick: number;
  /** Active Service port-forwards (live, via `forwards:changed`). */
  forwards: ForwardInfo[];
  init: () => Promise<void>;
  saveSettings: (s: Settings) => Promise<void>;
  /** Make another saved connection the monitored one. */
  switchConnection: (id: string) => Promise<void>;
  /** Pause/resume all polling (mirrors the tray toggle). */
  setPaused: (paused: boolean) => Promise<void>;
}

let initialized = false;

export function activeProfile(s: Settings | null): ConnectionProfile | null {
  return s?.connections.find((p) => p.id === s.activeConnectionId) ?? null;
}

function activeConnection(s: Settings | null) {
  return activeProfile(s)?.connection ?? null;
}

export const useClusterStore = create<ClusterState>((set, get) => ({
  snapshot: null,
  status: null,
  settings: null,
  logSyncTick: 0,
  forwards: [],

  init: async () => {
    if (initialized) return;
    initialized = true;
    await Promise.all([
      listen<ClusterSnapshot>("cluster:snapshot", (e) => set({ snapshot: e.payload })),
      listen<Status>("cluster:status", (e) => set({ status: e.payload })),
      listen<Settings>("settings:changed", (e) => set({ settings: e.payload })),
      listen<number>("logs:synced", () => set((s) => ({ logSyncTick: s.logSyncTick + 1 }))),
      listen<ForwardInfo[]>("forwards:changed", (e) => set({ forwards: e.payload })),
    ]);
    const [snapshot, status, settings, forwards] = await Promise.all([
      ipc.getSnapshot(),
      ipc.getStatus(),
      ipc.getSettings(),
      ipc.listPortForwards(),
    ]);
    set({ snapshot, status, settings, forwards });
  },

  saveSettings: async (s) => {
    const saved = await ipc.saveSettings(s);
    set((prev) => ({
      settings: saved,
      // A different (or edited) active connection invalidates what's on screen.
      snapshot:
        JSON.stringify(activeConnection(prev.settings)) === JSON.stringify(activeConnection(saved)) ? prev.snapshot : null,
    }));
  },

  setPaused: async (paused) => {
    const settings = await ipc.setMonitoringPaused(paused);
    set((s) => ({ settings, status: s.status ? { ...s.status, paused } : s.status }));
  },

  switchConnection: async (id) => {
    const cur = get().settings;
    if (!cur || cur.activeConnectionId === id) return;
    await get().saveSettings({ ...cur, activeConnectionId: id });
  },
}));
