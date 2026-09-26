import { create } from "zustand";
import * as ipc from "./ipc";
import { confirmDestructive } from "./dialog";
import { errorMessage } from "./format";
import { toast } from "../stores/toast";
import { useNavStore } from "../stores/nav";
import type { ActionKind } from "./types";

export interface ActionTarget {
  kind: string;
  namespace: string | null;
  name: string;
  /** Current replica count, for the scale dialog. */
  replicas?: number;
  /** Count remembered by "Scale to 0", offered as Restore. */
  remembered?: number | null;
}

export const ACTION_LABELS: Record<ActionKind, string> = {
  viewLogs: "Logs",
  viewPreviousLogs: "Crash logs",
  deletePod: "Restart pod",
  rolloutRestart: "Rollout restart",
  scale: "Scale",
  cordon: "Cordon",
  uncordon: "Uncordon",
};

interface ScaleDialogState {
  target: ActionTarget | null;
  open: (t: ActionTarget) => void;
  close: () => void;
}

export const useScaleDialog = create<ScaleDialogState>((set) => ({
  target: null,
  open: (target) => set({ target }),
  close: () => set({ target: null }),
}));

async function attempt(label: string, fn: () => Promise<void>) {
  try {
    await fn();
    toast.success(label);
  } catch (e) {
    toast.error(errorMessage(e));
  }
}

/** Execute an issue/row action. Destructive ones confirm first. */
export async function runAction(action: ActionKind, t: ActionTarget): Promise<void> {
  const nav = useNavStore.getState();
  const ns = t.namespace ?? "";
  switch (action) {
    case "viewLogs":
      nav.openPod(ns, t.name, "logs");
      return;
    case "viewPreviousLogs":
      nav.openPod(ns, t.name, "previous");
      return;
    case "deletePod":
      if (await confirmDestructive(`Delete pod ${ns}/${t.name}?\n\nIts controller will create a replacement. A pod with no controller is gone for good.`, "Restart pod")) {
        await attempt(`Deleted ${t.name}`, () => ipc.deletePod(ns, t.name));
      }
      return;
    case "rolloutRestart":
      if (await confirmDestructive(`Rollout restart ${t.kind} ${ns}/${t.name}?\n\nPods are replaced one by one following its update strategy.`, "Rollout restart")) {
        await attempt(`Restarting ${t.name}`, () => ipc.rolloutRestart(t.kind, ns, t.name));
      }
      return;
    case "scale":
      useScaleDialog.getState().open(t);
      return;
    case "cordon":
      if (await confirmDestructive(`Cordon node ${t.name}? New pods won't schedule on it.`, "Cordon")) {
        await attempt(`Cordoned ${t.name}`, () => ipc.setCordon(t.name, true));
      }
      return;
    case "uncordon":
      await attempt(`Uncordoned ${t.name}`, () => ipc.setCordon(t.name, false));
      return;
  }
}

interface DeleteDialogState {
  target: ActionTarget | null;
  open: (t: ActionTarget) => void;
  close: () => void;
}

export const useDeleteDialog = create<DeleteDialogState>((set) => ({
  target: null,
  open: (target) => set({ target }),
  close: () => set({ target: null }),
}));

/** Scale to 0 and remember the current count on the object. */
export async function stopWorkload(t: ActionTarget) {
  await attempt(`Scaled ${t.name} to 0 (will restore ${t.replicas})`, async () => {
    await ipc.setWorkloadDisabled(t.kind, t.namespace ?? "", t.name, true);
  });
}

/** Bring back the remembered replica count. */
export async function restoreWorkload(t: ActionTarget) {
  await attempt(`Restored ${t.name} to ${t.remembered ?? 1}`, async () => {
    await ipc.setWorkloadDisabled(t.kind, t.namespace ?? "", t.name, false);
  });
}

export async function deleteWorkload(t: ActionTarget) {
  await attempt(`Deleted ${t.kind} ${t.name}`, () => ipc.deleteWorkload(t.kind, t.namespace ?? "", t.name));
}

export async function scaleTo(t: ActionTarget, replicas: number) {
  if (replicas === 0 && !(await confirmDestructive(`Scale ${t.name} to 0? It will stop serving entirely.`, "Scale to zero"))) {
    return;
  }
  await attempt(`Scaled ${t.name} to ${replicas}`, () => ipc.scaleWorkload(t.kind, t.namespace ?? "", t.name, replicas));
}
