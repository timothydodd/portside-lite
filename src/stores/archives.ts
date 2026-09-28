import { create } from "zustand";
import * as ipc from "../lib/ipc";
import type { ArchiveMeta } from "../lib/types";

interface ArchivesState {
  /** Every archive on disk, newest first; null until first loaded. */
  archives: ArchiveMeta[] | null;
  error: string | null;
  load: () => Promise<void>;
}

/** Archived workloads (read from the archive folder, not the cluster). */
export const useArchivesStore = create<ArchivesState>((set) => ({
  archives: null,
  error: null,
  load: async () => {
    try {
      set({ archives: await ipc.listArchives(), error: null });
    } catch (e) {
      set({ archives: [], error: String(e) });
    }
  },
}));

/** The archive of one workload on one cluster, if there is one. */
export function findArchive(
  archives: ArchiveMeta[] | null,
  clusterId: string | null | undefined,
  t: { kind: string; namespace: string; name: string },
): ArchiveMeta | undefined {
  return archives?.find((a) => a.clusterId === clusterId && a.kind === t.kind && a.namespace === t.namespace && a.name === t.name);
}
