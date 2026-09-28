// Dev-only stand-in for the Tauri backend so the UI can be viewed in a plain
// browser (`npm run dev`, open http://localhost:1420). Loaded by main.tsx only
// when running under Vite dev without a Tauri runtime. Fixture data is fake.

import type { ClusterSnapshot, Issue, LogLevel, PodInfo, Settings } from "../lib/types";

const now = Date.now();
const H = 3600_000;
const GiB = 1024 ** 3;

function pod(ns: string, name: string, extra: Partial<PodInfo> = {}): PodInfo {
  return {
    namespace: ns,
    name,
    uid: `${ns}-${name}`,
    node: "k3s-worker-1",
    phase: "Running",
    status: "Running",
    readyContainers: 1,
    totalContainers: 1,
    restarts: 0,
    lastRestartMs: null,
    createdMs: now - 3 * 86_400_000,
    deletingSinceMs: null,
    ownerKind: "Deployment",
    ownerName: name.replace(/-[a-z0-9]+-[a-z0-9]+$/, ""),
    podIp: "10.42.1.17",
    qosClass: "Burstable",
    cpuUsage: 0.02 + Math.random() * 0.2,
    memUsage: (60 + Math.random() * 300) * 1024 ** 2,
    cpuRequests: 0.1,
    memRequests: 128 * 1024 ** 2,
    memLimits: 512 * 1024 ** 2,
    containers: [
      {
        name: "app",
        image: `ghcr.io/example/${name.split("-")[0]}:1.4.2`,
        init: false,
        ready: true,
        restarts: 0,
        state: "running",
        reason: null,
        message: null,
        startedMs: now - 2 * 86_400_000,
        lastTerminatedReason: null,
        lastTerminatedExitCode: null,
        lastTerminatedMs: null,
      },
    ],
    conditions: [{ type: "Ready", status: "True", reason: null, message: null, lastTransitionMs: now - 2 * 86_400_000 }],
    ...extra,
  };
}

const crashing = pod("apps", "billing-api-7d9f8b6c5-x2k4p", {
  status: "CrashLoopBackOff",
  readyContainers: 0,
  restarts: 23,
  lastRestartMs: now - 4 * 60_000,
  containers: [
    {
      name: "app",
      image: "ghcr.io/example/billing-api:2.0.0",
      init: false,
      ready: false,
      restarts: 23,
      state: "waiting",
      reason: "CrashLoopBackOff",
      message: "back-off 5m0s restarting failed container=app pod=billing-api-7d9f8b6c5-x2k4p",
      startedMs: null,
      lastTerminatedReason: "Error",
      lastTerminatedExitCode: 1,
      lastTerminatedMs: now - 4 * 60_000,
    },
  ],
});

const pending = pod("data", "postgres-0", {
  phase: "Pending",
  status: "Pending",
  node: null,
  readyContainers: 0,
  ownerKind: "StatefulSet",
  ownerName: "postgres",
  cpuUsage: null,
  memUsage: null,
  createdMs: now - 40 * 60_000,
  conditions: [
    { type: "PodScheduled", status: "False", reason: "Unschedulable", message: "0/3 nodes are available: 3 Insufficient memory. preemption: 0/3 nodes are available.", lastTransitionMs: now - 40 * 60_000 },
  ],
});

const pods: PodInfo[] = [
  crashing,
  pending,
  pod("apps", "web-frontend-5c8d7f9b4-abcde"),
  pod("apps", "web-frontend-5c8d7f9b4-fghij", { node: "k3s-server" }),
  pod("apps", "worker-6b7c8d9e0-klmno", { restarts: 7, lastRestartMs: now - 5 * H }),
  pod("monitoring", "grafana-7f8e9d0c1-pqrst", { node: "k3s-server" }),
  pod("kube-system", "coredns-ccb96694c-uvwxy", { node: "k3s-server", ownerName: "coredns" }),
  pod("kube-system", "traefik-5d45fc8cc9-zabcd", { node: "k3s-server", ownerName: "traefik" }),
  pod("kube-system", "metrics-server-5985cbc9d7-efghi", { node: "k3s-server", ownerName: "metrics-server" }),
  pod("kube-system", "local-path-provisioner-5cf85fd84d-jklmn", { ownerName: "local-path-provisioner" }),
];

const issues: Issue[] = [
  {
    key: "crashloopbackoff:Pod/apps/billing-api-7d9f8b6c5-x2k4p/app",
    severity: "critical",
    category: "pod",
    rule: "crashloopbackoff",
    kind: "Pod",
    namespace: "apps",
    name: crashing.name,
    title: `CrashLoopBackOff: ${crashing.name}`,
    detail: "Container 'app' last exited with code 1 (Error); 23 restarts.",
    hint: "The container keeps crashing on start. Check the previous container's logs for the crash reason.",
    sinceMs: now - 4 * 60_000,
    actions: ["viewPreviousLogs", "viewLogs", "deletePod"],
    firstSeenMs: now - 2 * H,
  },
  {
    key: "unschedulable:Pod/data/postgres-0",
    severity: "critical",
    category: "pod",
    rule: "unschedulable",
    kind: "Pod",
    namespace: "data",
    name: "postgres-0",
    title: "Unschedulable: postgres-0",
    detail: "0/3 nodes are available: 3 Insufficient memory. preemption: 0/3 nodes are available.",
    hint: "Not enough free CPU/memory, or node selectors/taints/affinity exclude every node.",
    sinceMs: now - 40 * 60_000,
    actions: ["deletePod"],
    firstSeenMs: now - 40 * 60_000,
  },
  {
    key: "workload-degraded:Deployment/apps/billing-api",
    severity: "warning",
    category: "workload",
    rule: "workload-degraded",
    kind: "Deployment",
    namespace: "apps",
    name: "billing-api",
    title: "Deployment degraded: billing-api",
    detail: "1/2 ready.",
    hint: "Its pods are listed under Problems too — fix those first.",
    sinceMs: null,
    actions: ["rolloutRestart", "scale"],
    firstSeenMs: now - 2 * H,
  },
  {
    key: "node-high-memory:Node/k3s-worker-1",
    severity: "warning",
    category: "node",
    rule: "node-high-memory",
    kind: "Node",
    namespace: null,
    name: "k3s-worker-1",
    title: "High memory: k3s-worker-1",
    detail: "Memory at 93% of allocatable.",
    hint: "Find the top consumers on the Pods page (sort by memory). Evictions start under pressure.",
    sinceMs: null,
    actions: [],
    firstSeenMs: now - 25 * 60_000,
  },
  {
    key: "log-errors:Pod/apps/worker-6b7c8d9e0-klmno",
    severity: "warning",
    category: "logs",
    rule: "log-errors",
    kind: "Pod",
    namespace: "apps",
    name: "worker-6b7c8d9e0-klmno",
    title: "Error log spike: worker-6b7c8d9e0-klmno",
    detail: "184 error-level log lines in the last hour.",
    hint: "Open its logs filtered to errors to see what's failing.",
    sinceMs: null,
    actions: ["viewLogs"],
    firstSeenMs: now - 50 * 60_000,
  },
  {
    key: "restarts:Pod/apps/worker-6b7c8d9e0-klmno",
    severity: "info",
    category: "pod",
    rule: "restarts",
    kind: "Pod",
    namespace: "apps",
    name: "worker-6b7c8d9e0-klmno",
    title: "7 restarts: worker-6b7c8d9e0-klmno",
    detail: "Pod is running now but has restarted repeatedly.",
    hint: "Look at the previous container's logs around the last restart.",
    sinceMs: now - 5 * H,
    actions: ["viewPreviousLogs", "viewLogs"],
    firstSeenMs: now - 5 * H,
  },
];

function node(name: string, roles: string[], cpu: number, mem: number, memTotal: number) {
  return {
    name,
    ready: true,
    unschedulable: false,
    roles,
    kubeletVersion: "v1.33.4+k3s1",
    osImage: "Ubuntu 24.04.2 LTS",
    kernelVersion: "6.8.0-60-generic",
    internalIp: name === "k3s-server" ? "192.168.1.10" : "192.168.1.11",
    cpuCapacity: 4,
    cpuAllocatable: 4,
    memCapacity: memTotal,
    memAllocatable: memTotal,
    cpuUsage: cpu,
    memUsage: mem,
    cpuRequests: 1.2,
    memRequests: memTotal * 0.6,
    podCount: pods.filter((p) => p.node === name).length,
    podCapacity: 110,
    conditions: [
      { type: "MemoryPressure", status: "False", reason: null, message: "kubelet has sufficient memory available", lastTransitionMs: now - 9 * 86_400_000 },
      { type: "DiskPressure", status: "False", reason: null, message: "kubelet has no disk pressure", lastTransitionMs: now - 9 * 86_400_000 },
      { type: "Ready", status: "True", reason: null, message: "kubelet is posting ready status", lastTransitionMs: now - 9 * 86_400_000 },
    ],
    createdMs: now - 90 * 86_400_000,
  };
}

const nodes = [node("k3s-server", ["control-plane", "master"], 1.1, 5.2 * GiB, 8 * GiB), node("k3s-worker-1", [], 2.6, 7.4 * GiB, 8 * GiB)];

const snapshot: ClusterSnapshot = {
  collectedAtMs: now,
  clusterId: "ssh:ops@k3s-server:22",
  serverVersion: "v1.33.4+k3s1",
  metricsAvailable: true,
  totals: {
    nodes: 2,
    nodesReady: 2,
    pods: pods.length,
    podsRunning: pods.filter((p) => p.phase === "Running").length,
    podsPending: 1,
    podsFailed: 0,
    cpuAllocatable: 8,
    memAllocatable: 16 * GiB,
    cpuUsage: 3.7,
    memUsage: 12.6 * GiB,
    cpuRequests: 2.4,
    memRequests: 9.6 * GiB,
  },
  nodes,
  pods,
  workloads: [
    { kind: "Deployment", namespace: "apps", name: "billing-api", desired: 2, ready: 1, available: 1, updated: 2, failed: 0, images: ["ghcr.io/example/billing-api:2.0.0"], paused: false, disabledReplicas: null, conditionMessage: null, createdMs: now - 30 * 86_400_000, schedule: null, lastScheduleMs: null },
    { kind: "Deployment", namespace: "apps", name: "web-frontend", desired: 2, ready: 2, available: 2, updated: 2, failed: 0, images: ["ghcr.io/example/web:1.4.2"], paused: false, disabledReplicas: null, conditionMessage: null, createdMs: now - 30 * 86_400_000, schedule: null, lastScheduleMs: null },
    { kind: "StatefulSet", namespace: "data", name: "postgres", desired: 1, ready: 0, available: 0, updated: 1, failed: 0, images: ["postgres:16"], paused: false, disabledReplicas: null, conditionMessage: null, createdMs: now - 1 * H, schedule: null, lastScheduleMs: null },
    { kind: "Deployment", namespace: "apps", name: "legacy-api", desired: 0, ready: 0, available: 0, updated: 0, failed: 0, images: ["ghcr.io/example/legacy:0.9"], paused: false, disabledReplicas: 3, conditionMessage: null, createdMs: now - 90 * 86_400_000, schedule: null, lastScheduleMs: null },
    { kind: "CronJob", namespace: "apps", name: "nightly-report", desired: 0, ready: 0, available: 0, updated: 0, failed: 0, images: ["ghcr.io/example/report:1.0"], paused: false, disabledReplicas: null, conditionMessage: null, createdMs: now - 60 * 86_400_000, schedule: "0 3 * * *", lastScheduleMs: now - 9 * H },
  ],
  volumes: [],
  services: [
    { namespace: "apps", name: "web-frontend", type: "LoadBalancer", clusterIp: "10.43.12.7", external: ["192.168.1.240"], ports: [{ name: "http", port: 80, targetPort: "8080", nodePort: 31080, protocol: "TCP" }], selector: { app: "web-frontend" }, podsMatched: 2, podsReady: 2, podNames: ["web-frontend-5c8d7f9b4-abcde", "web-frontend-5c8d7f9b4-fghij"], routes: ["shop.lan/ (web)"], createdMs: now - 30 * 86_400_000 },
    { namespace: "apps", name: "billing-api", type: "ClusterIP", clusterIp: "10.43.40.2", external: [], ports: [{ name: null, port: 8080, targetPort: null, nodePort: null, protocol: "TCP" }], selector: { app: "billing-api" }, podsMatched: 2, podsReady: 1, podNames: ["billing-api-7d9f8b6c5-x2k4p", "billing-api-7d9f8b6c5-q9z1m"], routes: ["shop.lan/api (web)"], createdMs: now - 30 * 86_400_000 },
    { namespace: "data", name: "postgres", type: "ClusterIP", clusterIp: "10.43.9.9", external: [], ports: [{ name: "pg", port: 5432, targetPort: null, nodePort: null, protocol: "TCP" }], selector: { app: "postgres" }, podsMatched: 0, podsReady: 0, podNames: [], routes: [], createdMs: now - 1 * H },
    { namespace: "apps", name: "reports", type: "ClusterIP", clusterIp: "10.43.3.3", external: [], ports: [{ name: null, port: 80, targetPort: "8000", nodePort: null, protocol: "TCP" }], selector: { app: "reportz" }, podsMatched: 0, podsReady: 0, podNames: [], routes: [], createdMs: now - 3 * 86_400_000 },
  ],
  configs: [
    { kind: "ConfigMap", namespace: "apps", name: "billing-api-config", secretType: null, keys: ["DB_HOST", "LOG_LEVEL", "app.properties"], sizeBytes: 812, immutable: false, usedBy: ["Deployment/billing-api"], createdMs: now - 30 * 86_400_000 },
    { kind: "ConfigMap", namespace: "apps", name: "web-frontend-config", secretType: null, keys: ["nginx.conf"], sizeBytes: 2048, immutable: false, usedBy: ["Deployment/web-frontend"], createdMs: now - 30 * 86_400_000 },
    { kind: "ConfigMap", namespace: "kube-system", name: "kube-root-ca.crt", secretType: null, keys: ["ca.crt"], sizeBytes: 570, immutable: false, usedBy: [], createdMs: now - 90 * 86_400_000 },
    { kind: "Secret", namespace: "apps", name: "billing-api-secrets", secretType: "Opaque", keys: ["DB_PASSWORD", "STRIPE_KEY"], sizeBytes: 64, immutable: false, usedBy: ["Deployment/billing-api"], createdMs: now - 30 * 86_400_000 },
    { kind: "Secret", namespace: "apps", name: "sh.helm.release.v1.web.v3", secretType: "helm.sh/release.v1", keys: ["release"], sizeBytes: 9200, immutable: false, usedBy: [], createdMs: now - 9 * 86_400_000 },
  ],
  events: [
    { namespace: "kube-system", objectKind: "Pod", objectName: "svclb-traefik-4c2d9a7f-very-long-generated-name-xk2p9", reason: "FailedCreatePodSandBox", message: "Failed to create pod sandbox: rpc error: code = Unknown desc = failed to setup network for sandbox: plugin type=\"flannel\" failed (add): open /run/flannel/subnet.env: no such file or directory", type: "Warning", count: 31, firstMs: now - 20 * 60_000, lastMs: now - 30_000, source: "kubelet" },
    { namespace: "data", objectKind: "PersistentVolumeClaim", objectName: "data-postgres-0", reason: "ProvisioningFailed", message: "storageclass.storage.k8s.io \"fast-ssd\" not found", type: "Warning", count: 4, firstMs: now - 40 * 60_000, lastMs: now - 2 * 60_000, source: "persistentvolume-controller" },
    { namespace: "apps", objectKind: "Pod", objectName: crashing.name, reason: "BackOff", message: "Back-off restarting failed container app in pod " + crashing.name, type: "Warning", count: 112, firstMs: now - 2 * H, lastMs: now - 60_000, source: "kubelet" },
    { namespace: "data", objectKind: "Pod", objectName: "postgres-0", reason: "FailedScheduling", message: "0/3 nodes are available: 3 Insufficient memory.", type: "Warning", count: 9, firstMs: now - 40 * 60_000, lastMs: now - 3 * 60_000, source: "default-scheduler" },
    { namespace: "apps", objectKind: "Pod", objectName: "worker-6b7c8d9e0-klmno", reason: "Unhealthy", message: "Readiness probe failed: HTTP probe failed with statuscode: 503", type: "Warning", count: 14, firstMs: now - H, lastMs: now - 8 * 60_000, source: "kubelet" },
  ],
  issues,
  namespaces: ["apps", "data", "kube-system", "monitoring"],
};

let settings: Settings = {
  connections: [
    {
      id: "homelab",
      name: "Homelab",
      connection: { mode: "ssh", host: "k3s-server", port: 22, username: "ops", auth: { kind: "key", privateKeyPath: "~/.ssh/id_ed25519", passphrase: null }, kubeconfigCommand: "sudo -n cat /etc/rancher/k3s/k3s.yaml", apiHost: "127.0.0.1", apiPort: 6443, hostKeyFingerprint: "SHA256:q3Zg7rN1…demo", sudoPassword: null },
    },
    { id: "local", name: "Local dev", connection: { mode: "local", kubeconfigPath: null, context: "homelab-dev" } },
  ],
  activeConnectionId: "homelab",
  pollIntervalSecs: 15,
  collectLogs: true,
  logIntervalSecs: 60,
  initialLogLookbackHours: 24,
  maxLogBytesPerPull: 4 * 1024 * 1024,
  retentionDays: 7,
  excludedNamespaces: [],
  highUsagePercent: 90,
  restartWarningThreshold: 5,
  errorLogSpikePerHour: 50,
  closeToTray: true,
  notifyCritical: true,
  notifyWarnings: false,
  backgroundCheckMinutes: 15,
  monitoringPaused: false,
  archiveDir: null,
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let mockArchives: any[] = [
  {
    id: "ssh_ops_k3s-server_22/apps/deployment-legacy-reports",
    format: 1,
    kind: "Deployment",
    namespace: "apps",
    name: "legacy-reports",
    clusterId: "ssh:ops@k3s-server:22",
    profileId: "homelab",
    connectionName: "Homelab",
    archivedMs: now - 3 * 86_400_000,
    replicas: 2,
    images: ["ghcr.io/example/legacy-reports:0.9.1"],
    objects: [
      { kind: "ConfigMap", name: "legacy-reports-config", removed: true, error: null },
      { kind: "Service", name: "legacy-reports", removed: true, error: null },
      { kind: "Deployment", name: "legacy-reports", removed: true, error: null },
    ],
    logLines: 18_422,
    restoredMs: null,
    restoredTo: null,
  },
];

const MESSAGES: [LogLevel, string][] = [
  ["info", "GET /api/invoices 200 12ms"],
  ["info", "processed job batch size=50 duration=812ms"],
  ["debug", "cache hit key=user:4821"],
  ["warning", "slow query took 1843ms: SELECT * FROM invoices WHERE customer_id = $1"],
  ["error", "failed to connect to postgres:5432: connection refused"],
  ["error", 'unhandled exception: {"error":"timeout after 3012ms","op":"charge"}'],
  ["info", "health check ok"],
];

function samples(since: number, bucket: number, cpuBase: number, memBase: number) {
  const out = [];
  for (let t = Math.ceil(since / bucket) * bucket; t < now; t += bucket) {
    const k = (t - since) / (now - since);
    out.push({ tsMs: t, cpu: cpuBase * (0.7 + 0.3 * Math.sin(k * 12) + Math.random() * 0.1), mem: memBase * (0.85 + k * 0.1 + Math.random() * 0.02) });
  }
  return out;
}

function histogram(since: number, until: number, bucket: number) {
  const out = [];
  for (let t = Math.ceil(since / bucket) * bucket; t < until; t += bucket) {
    const spike = Math.abs(t - (now - 2 * H)) < 45 * 60_000 ? 6 : 1;
    out.push({ bucketMs: t, trace: 0, debug: Math.round(80 * Math.random()), info: Math.round(400 + 200 * Math.random()), warning: Math.round(20 * Math.random() * spike), error: Math.round(6 * Math.random() * spike * spike) });
  }
  return out;
}

type Args = Record<string, unknown>;
const handlers: Record<string, (a: Args) => unknown> = {
  get_settings: () => settings,
  save_settings: (a) => {
    settings = a.settings as Settings;
    return settings;
  },
  get_status: () => ({ state: "connected", message: null, clusterId: snapshot.clusterId, serverVersion: snapshot.serverVersion, lastPollMs: now - 4000, lastPollDurationMs: 212, lastLogSyncMs: now - 21_000, lastLogSyncLines: 1834, logSyncErrors: 0, paused: settings.monitoringPaused }),
  get_snapshot: () => snapshot,
  test_connection: (a) => {
    const c = a.connection as Settings["connections"][number]["connection"];
    return { serverVersion: "v1.33.4+k3s1", hostKeyFingerprint: c.mode === "ssh" ? c.hostKeyFingerprint : null };
  },
  list_kube_contexts: () => ({ contexts: ["default", "homelab"], current: "default" }),
  cluster_history: (a) => samples(a.sinceMs as number, a.bucketMs as number, 3.5, 12 * GiB),
  node_history: (a) => samples(a.sinceMs as number, a.bucketMs as number, 1.6, 6 * GiB),
  pod_history: (a) => samples(a.sinceMs as number, a.bucketMs as number, 0.15, 300 * 1024 ** 2),
  log_histogram: (a) => {
    const q = a.query as { sinceMs: number | null; untilMs?: number | null };
    return histogram(q.sinceMs ?? now - 6 * 86_400_000, q.untilMs ?? now, a.bucketMs as number);
  },
  query_logs: (a) => {
    const q = a.query as { beforeId?: number | null; levels?: LogLevel[]; workload?: { name: string } | null; namespace?: string | null };
    const start = (q.beforeId ?? 100_000) - 1;
    const rows = [];
    for (let i = 0; i < 500 && rows.length < 500; i++) {
      const [level, message] = MESSAGES[(start - i) % MESSAGES.length];
      if (q.levels?.length && !q.levels.includes(level)) continue;
      const p = q.workload
        ? { namespace: q.namespace ?? "apps", name: `${q.workload.name}-${(start - i) % 3 ? "7d9f8b6c5-x2k4p" : "5b8c9d7f6-old01"}` }
        : pods[(start - i) % 5];
      rows.push({ id: start - i, tsMs: now - i * 1700, namespace: p.namespace, pod: p.name, container: "app", level, message });
    }
    return rows;
  },
  pod_log_counts: () => [
    { namespace: "apps", pod: "worker-6b7c8d9e0-klmno", errors: 1204, warnings: 311, total: 22000 },
    { namespace: "apps", pod: crashing.name, errors: 388, warnings: 12, total: 900 },
    { namespace: "kube-system", pod: "traefik-5d45fc8cc9-zabcd", errors: 3, warnings: 41, total: 5000 },
  ],
  top_error_pods: () => handlers.pod_log_counts({}),
  error_patterns: () => [
    { pattern: "failed to connect to postgres:#: connection refused", sample: "failed to connect to postgres:5432: connection refused", count: 1022, pods: ["apps/worker-6b7c8d9e0-klmno", "apps/" + crashing.name], lastMs: now - 30_000 },
    { pattern: 'unhandled exception: {"error":"timeout after #ms","op":"charge"}', sample: 'unhandled exception: {"error":"timeout after 3012ms","op":"charge"}', count: 402, pods: ["apps/" + crashing.name], lastMs: now - 4 * 60_000 },
  ],
  issue_history: () => [
    ...issues.map((i, n) => ({ id: n + 1, key: i.key, severity: i.severity, category: i.category, kind: i.kind, namespace: i.namespace, name: i.name, title: i.title, detail: i.detail, firstSeenMs: i.firstSeenMs!, lastSeenMs: now, resolvedMs: null })),
    { id: 99, key: "diskpressure:Node/k3s-worker-1", severity: "critical", category: "node", kind: "Node", namespace: null, name: "k3s-worker-1", title: "DiskPressure: k3s-worker-1", detail: "kubelet has disk pressure", firstSeenMs: now - 30 * H, lastSeenMs: now - 28 * H, resolvedMs: now - 28 * H },
  ],
  storage_stats: () => ({ logLines: 482_113, oldestLogMs: now - 6.5 * 86_400_000, nodeSamples: 80_000, podSamples: 410_000, issuesTracked: 57, dbBytes: 214 * 1024 ** 2 }),
  live_logs: () => MESSAGES.concat(MESSAGES).map(([level, message], i) => ({ tsMs: now - (20 - i) * 900, level, message })),
  object_events: () => snapshot.events,
  export_manifests: (a) =>
    `apiVersion: apps/v1\nkind: ${a.kind}\nmetadata:\n  name: ${a.name ?? "web-frontend"}\n  namespace: ${a.namespace ?? "apps"}\n  labels:\n    app: web\nspec:\n  replicas: 2\n`,
  edit_manifest: (a) =>
    `apiVersion: apps/v1\nkind: ${a.kind}\nmetadata:\n  name: ${a.name}\n  namespace: ${a.namespace}\n  resourceVersion: "48213"\n  labels:\n    app: ${a.name}\nspec:\n  replicas: 2\n  selector:\n    matchLabels:\n      app: ${a.name}\n  template:\n    metadata:\n      labels:\n        app: ${a.name}\n    spec:\n      containers:\n      - name: app\n        image: ghcr.io/example/${a.name}:2.0.0\n        envFrom:\n        - configMapRef:\n            name: ${a.name}-config\n`,
  apply_manifest: (a) => `deployments/${a.name} ${a.dryRun ? "validated (dry run, nothing changed)" : "configured"}`,
  save_text_file: () => null,
  related_objects: (a) => [
    { kind: "ConfigMap", name: `${a.name}-config`, reason: "used by its pods", exists: true, sensitive: false, defaultSelected: true },
    { kind: "Secret", name: `${a.name}-secrets`, reason: "used by its pods (contains credentials)", exists: true, sensitive: true, defaultSelected: false },
    { kind: "PersistentVolumeClaim", name: `${a.name}-data`, reason: "mounted by its pods; data isn't included, only the claim", exists: true, sensitive: false, defaultSelected: false },
    { kind: "Service", name: a.name as string, reason: "selects its pods", exists: true, sensitive: false, defaultSelected: true },
    { kind: "Ingress", name: "web", reason: `routes to Service ${a.name}`, exists: true, sensitive: false, defaultSelected: true },
  ],
  export_bundle: (a) => `# ${(a.extras as unknown[]).length + 1} objects\napiVersion: v1\nkind: Service\nmetadata:\n  name: ${a.name}\n---\napiVersion: apps/v1\nkind: ${a.kind}\nmetadata:\n  name: ${a.name}\n`,
  get_config: (a) => ({
    kind: a.kind,
    namespace: a.namespace,
    name: a.name,
    resourceVersion: "5501",
    secretType: a.kind === "Secret" ? "Opaque" : null,
    immutable: false,
    entries:
      a.kind === "Secret"
        ? [
            { key: "DB_PASSWORD", value: "hunter2-not-real", binary: false, size: 16 },
            { key: "STRIPE_KEY", value: "sk_test_demo", binary: false, size: 12 },
            { key: "keystore.p12", value: null, binary: true, size: 2412 },
          ]
        : [
            { key: "DB_HOST", value: "postgres.data.svc", binary: false, size: 17 },
            { key: "LOG_LEVEL", value: "info", binary: false, size: 4 },
            { key: "app.properties", value: "retries=3\ntimeout=30s\n", binary: false, size: 22 },
          ],
  }),
  save_config: (a) => ({
    kind: a.kind,
    namespace: a.namespace,
    name: a.name,
    resourceVersion: "5502",
    secretType: a.kind === "Secret" ? "Opaque" : null,
    immutable: false,
    entries: [
      ...Object.entries(a.text as Record<string, string>).map(([key, value]) => ({ key, value, binary: false, size: value.length })),
      ...(a.keepBinary as string[]).map((key) => ({ key, value: null, binary: true, size: 2412 })),
    ].sort((x, y) => x.key.localeCompare(y.key)),
  }),
  copy_to_cluster: (a) => [
    ...(a.extras as { kind: string; name: string }[]).map((e) => ({ ...e, outcome: "created", message: null })),
    { kind: a.kind, name: a.name, outcome: "created", message: null },
  ],
  set_workload_disabled: (a) => (a.disabled ? 0 : 3),
  delete_workload: () => null,
  test_notification: () => null,
  check_all_now: () => null,
  set_monitoring_paused: (a) => {
    settings = { ...settings, monitoringPaused: a.paused as boolean };
    return settings;
  },
  read_text_files: (a) =>
    (a.paths as string[]).map((p) => ({
      name: p.split(/[\\/]/).pop()!,
      content: p.includes("bad")
        ? "apiVersion: v1\nkind: ConfigMap\nmetadata: {}\n"
        : "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: shop-config\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: shop\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: shop\n  namespace: shop\n",
    })),
  parse_manifests: (a) => mockParse(a.sources as { name: string; content: string }[]),
  import_manifests: (a) => {
    const docs = mockParse(a.sources as { name: string; content: string }[]).filter((d) => (a.include as number[]).includes(d.index) && !d.error);
    const order = (k: string) => ({ Namespace: 0, ConfigMap: 3, Secret: 3, Service: 4, Deployment: 5 })[k] ?? 6;
    return docs
      .sort((x, y) => order(x.kind!) - order(y.kind!))
      .map((d) => ({ index: d.index, source: d.source, kind: d.kind, name: d.name, namespace: (a.namespaceOverride as string) || d.namespace || "default", outcome: "created", message: null }));
  },
  list_port_forwards: () => mockForwards,
  start_port_forward: (a) => {
    if (mockForwards.some((f) => f.service === a.service && f.servicePort === a.servicePort))
      throw `${a.service}:${a.servicePort} is already forwarded`;
    const sp = a.servicePort as number;
    const f = {
      id: mockForwards.length + 1, profileId: "homelab", clusterName: "Homelab", namespace: a.namespace, service: a.service, servicePort: sp,
      localPort: (a.localPort as number | null) ?? (sp === 80 ? 8080 : sp === 443 ? 8443 : sp >= 1024 ? sp : 10000 + sp),
      pod: `${a.service}-5c8d7f9b4-abcde`, targetPort: 8080, activeConnections: 0, totalConnections: 0, lastError: null, startedMs: Date.now(),
    };
    mockForwards = [...mockForwards, f];
    emitMock("forwards:changed", mockForwards);
    return f;
  },
  stop_port_forward: (a) => {
    mockForwards = mockForwards.filter((f) => f.id !== a.id);
    emitMock("forwards:changed", mockForwards);
    return null;
  },
  log_sources: (a) => {
    const w = a.workload as { kind: string; name: string } | null;
    const all = [
      ...snapshot.pods.map((p) => ({ namespace: p.namespace, pod: p.name, ownerKind: p.ownerKind, ownerName: p.ownerName, lines: 4200, errors: p.restarts ? 310 : 4, warnings: 22, firstMs: now - 5 * 86_400_000, lastMs: now - 60_000 })),
      { namespace: "apps", pod: "legacy-reports-6c9f7d8b5-q2w3e", ownerKind: "Deployment", ownerName: "legacy-reports", lines: 18_422, errors: 40, warnings: 120, firstMs: now - 6 * 86_400_000, lastMs: now - 3 * 86_400_000 },
      { namespace: "apps", pod: "billing-api-5b8c9d7f6-old01", ownerKind: "Deployment", ownerName: "billing-api", lines: 9100, errors: 12, warnings: 30, firstMs: now - 6 * 86_400_000, lastMs: now - 2 * 86_400_000 },
    ];
    return all.filter((s) => (!a.namespace || s.namespace === a.namespace) && (!w || (s.ownerKind === w.kind && s.ownerName === w.name)));
  },
  archive_root: () => "C:\\Users\\demo\\AppData\\Roaming\\portside-lite\\archives",
  archive_plan: (a) => [
    { kind: "ConfigMap", name: `${a.name}-config`, reason: "used by its pods", exists: true, sensitive: false, defaultSelected: true, usedBy: [] },
    { kind: "Secret", name: `${a.name}-secrets`, reason: "used by its pods (contains credentials)", exists: true, sensitive: true, defaultSelected: false, usedBy: [] },
    { kind: "Secret", name: "ghcr-pull", reason: "used by its pods (contains credentials)", exists: true, sensitive: true, defaultSelected: false, usedBy: ["Deployment/storefront", "Deployment/worker"] },
    { kind: "PersistentVolumeClaim", name: `${a.name}-data`, reason: "mounted by its pods; data isn't included, only the claim", exists: true, sensitive: false, defaultSelected: false, usedBy: [] },
    { kind: "Service", name: a.name, reason: "selects its pods", exists: true, sensitive: false, defaultSelected: true, usedBy: [] },
    { kind: "Ingress", name: "public", reason: `routes to Service ${a.name}`, exists: true, sensitive: false, defaultSelected: true, usedBy: ["Service/storefront"] },
  ],
  archive_workload: (a) => {
    const keep = a.keep as { kind: string; name: string }[];
    const remove = a.remove as { kind: string; name: string }[];
    const archive = {
      id: `ssh_ops_k3s-server_22/${a.namespace}/${(a.kind as string).toLowerCase()}-${a.name}`,
      format: 1, kind: a.kind, namespace: a.namespace, name: a.name, clusterId: snapshot.clusterId, profileId: "homelab", connectionName: "Homelab",
      archivedMs: Date.now(), replicas: 2, images: [`ghcr.io/example/${a.name}:1.4.2`],
      objects: [...keep, { kind: a.kind as string, name: a.name as string }].map((o) => ({ ...o, removed: o.name === a.name || remove.some((r) => r.kind === o.kind && r.name === o.name), error: null })),
      logLines: a.includeLogs ? 12_345 : 0, restoredMs: null, restoredTo: null,
    };
    mockArchives = [archive, ...mockArchives.filter((x) => x.id !== archive.id)];
    snapshot.workloads = snapshot.workloads.filter((w) => !(w.kind === a.kind && w.namespace === a.namespace && w.name === a.name));
    emitMock("cluster:snapshot", { ...snapshot });
    return { archive, results: [{ kind: a.kind, name: a.name, outcome: "removed", message: null }, ...remove.map((r) => ({ ...r, outcome: "removed", message: null }))] };
  },
  list_archives: () => mockArchives,
  archive_manifest: (a) => {
    const m = mockArchives.find((x) => x.id === a.id);
    return (m?.objects ?? []).map((o: { kind: string; name: string }) => `apiVersion: v1\nkind: ${o.kind}\nmetadata:\n  name: ${o.name}\n  namespace: ${m.namespace}\n`).join("---\n");
  },
  restore_archive: (a) => {
    const m = mockArchives.find((x) => x.id === a.id);
    if (!a.dryRun) mockArchives = mockArchives.map((x) => (x.id === a.id ? { ...x, restoredMs: Date.now(), restoredTo: "Homelab" } : x));
    return (m?.objects ?? []).map((o: { kind: string; name: string }, i: number) => ({ index: i, source: "manifest.yaml", kind: o.kind, name: o.name, namespace: (a.namespaceOverride as string) || m.namespace, outcome: "created", message: null }));
  },
  delete_archive: (a) => {
    mockArchives = mockArchives.filter((x) => x.id !== a.id);
    return null;
  },
  open_archive_folder: () => null,
  get_manifest: () => "apiVersion: v1\nkind: Pod\nmetadata:\n  name: demo\n  namespace: apps\nspec:\n  containers:\n  - name: app\n    image: ghcr.io/example/app:1.0\n",
};

/** Tiny stand-in for the Rust parser: split on `---`, read kind/name/namespace. */
function mockParse(sources: { name: string; content: string }[]) {
  const out: { index: number; source: string; apiVersion: string | null; kind: string | null; name: string | null; namespace: string | null; error: string | null }[] = [];
  for (const src of sources) {
    const docs = src.content.split(/^---\s*$/m).filter((d) => d.trim());
    docs.forEach((d, i) => {
      const get = (re: RegExp) => d.match(re)?.[1] ?? null;
      const name = get(/^\s{2}name:\s*(\S+)/m);
      out.push({
        index: out.length,
        source: i === 0 ? src.name : `${src.name} #${i + 1}`,
        apiVersion: get(/^apiVersion:\s*(\S+)/m),
        kind: get(/^kind:\s*(\S+)/m),
        name,
        namespace: get(/^\s{2}namespace:\s*(\S+)/m),
        error: name ? null : "missing metadata.name",
      });
    });
  }
  return out;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let mockForwards: any[] = [];
/** Minimal event bus so mock commands can push events like the backend does. */
const mockListeners = new Map<string, number[]>();
const mockCallbacks = new Map<number, (e: unknown) => void>();
function emitMock(event: string, payload: unknown) {
  for (const id of mockListeners.get(event) ?? []) mockCallbacks.get(id)?.({ event, id, payload });
}

let cbId = 0;
const w = window as unknown as Record<string, unknown>;
w.__TAURI_INTERNALS__ = {
  transformCallback: (cb: (e: unknown) => void) => {
    const id = ++cbId;
    mockCallbacks.set(id, cb);
    return id;
  },
  unregisterCallback: () => undefined,
  invoke: async (cmd: string, args: Args) => {
    if (cmd === "plugin:event|listen") {
      const { event, handler } = args as { event: string; handler: number };
      mockListeners.set(event, [...(mockListeners.get(event) ?? []), handler]);
      return handler;
    }
    if (cmd.startsWith("plugin:event|")) return ++cbId;
    if (cmd.startsWith("plugin:opener|")) return null;
    // plugin-dialog's confirm() sends `message` and treats "Ok" as confirmed.
    if (cmd === "plugin:dialog|message") return window.confirm(String(args.message)) ? "Ok" : "Cancel";
    if (cmd === "plugin:dialog|open") return ["C:\\demo\\shop.yaml", "C:\\demo\\bad.yaml"];
    if (cmd === "plugin:dialog|save") return `C:\\Users\\demo\\${(args.options as { defaultPath?: string })?.defaultPath ?? "export.yaml"}`;
    if (cmd.startsWith("plugin:dialog|")) return null;
    const h = handlers[cmd];
    if (!h) {
      console.info("[mock] no-op", cmd, args);
      return null;
    }
    return h(args);
  },
  metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main", windowLabel: "main" } },
};
w.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => undefined };
