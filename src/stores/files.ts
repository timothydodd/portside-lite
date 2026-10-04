import { create } from "zustand";
import { confirmDestructive } from "../lib/dialog";

interface FilesState {
  /** An upload or download is running in the open file browser. */
  transferring: boolean;
  setTransferring: (v: boolean) => void;
}

export const useFilesStore = create<FilesState>((set) => ({
  transferring: false,
  setTransferring: (transferring) => set({ transferring }),
}));

/** Ask before leaving the file browser mid-transfer (leaving cancels it). True = fine to leave. */
export async function confirmLeaveFiles(): Promise<boolean> {
  if (!useFilesStore.getState().transferring) return true;
  return confirmDestructive("A file transfer is still running.\n\nClosing the file browser now stops it.", "Stop the transfer?");
}
