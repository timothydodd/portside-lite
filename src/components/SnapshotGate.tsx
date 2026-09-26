import type { ReactNode } from "react";
import { PlugZap, ServerCrash } from "lucide-react";
import { useClusterStore } from "../stores/cluster";
import { useNavStore } from "../stores/nav";
import type { ClusterSnapshot } from "../lib/types";
import { EmptyState, Spinner } from "./ui";

/** Renders children once a snapshot exists; otherwise the connection state. */
export default function SnapshotGate({ children }: { children: (s: ClusterSnapshot) => ReactNode }) {
  const snapshot = useClusterStore((s) => s.snapshot);
  const status = useClusterStore((s) => s.status);
  const go = useNavStore((s) => s.go);

  if (snapshot) return <>{children(snapshot)}</>;

  if (status?.state === "unconfigured") {
    return (
      <EmptyState icon={<PlugZap size={32} />} title="No cluster connected">
        <p className="mb-3">Point Portside Lite at a local kubeconfig or SSH into a k3s server.</p>
        <button className="btn-primary" onClick={() => go("settings")}>
          Open settings
        </button>
      </EmptyState>
    );
  }
  if (status?.state === "error") {
    return (
      <EmptyState icon={<ServerCrash size={32} className="text-critical" />} title="Can't reach the cluster">
        <p className="mono mb-3 whitespace-pre-wrap break-words text-left">{status.message}</p>
        <p className="mb-3">Retrying every poll interval.</p>
        <button className="btn-ghost" onClick={() => go("settings")}>
          Check connection settings
        </button>
      </EmptyState>
    );
  }
  return (
    <EmptyState icon={<Spinner size={28} />} title="Connecting to cluster…">
      {status?.clusterId}
    </EmptyState>
  );
}
