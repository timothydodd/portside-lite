# Features

## Problems

Every poll runs a set of rules and lists what needs attention, worst first. Each problem comes with
a plain-language next step and, where possible, a button for it.

- CrashLoopBackOff, ImagePullBackOff and container config errors
- OOMKills, unschedulable or stuck Pending pods, not-ready pods, pods stuck Terminating
- NotReady nodes, memory/disk/PID pressure, high CPU or memory
- Degraded or down Deployments, StatefulSets and DaemonSets, and failed Jobs
- Pending or Lost PVCs (including ones that name a StorageClass that doesn't exist); Failed
  PersistentVolumes, Released ones that weren't cleaned up, and Released ones kept by a Retain policy
- Autoscalers that can't scale (usually missing metrics or resource requests) or are stuck at their
  maximum
- Probe failures (readiness failures while a pod is still starting are ignored), mount failures and evictions (from events)
- Services with no endpoints, and Ingress routes that point at a Service that doesn't exist
- Error-log spikes per pod

**History.** The app records when each problem started and resolved, and which ones keep coming
back.

## Actions

- Restart (delete) a pod; rollout-restart, scale or delete a workload; cordon or uncordon a node.
- **Scale to 0** remembers the replica count in an annotation on the object, so **Start** brings the
  same count back.
- **Delete** asks you to type the name and offers a YAML backup first. Every destructive action asks
  before running.
- View YAML and events for any object.

## YAML: edit, export, copy, import

- **Edit** a workload's YAML in the app, with a dry-run check before applying. Saving works like
  `kubectl edit`: if the object changed on the cluster in the meantime, the save is refused.
- **Export** clean, re-appliable YAML. Choose a single workload, a whole kind, or a namespace. A
  single workload can bring the objects it needs, and you pick which: its ConfigMaps, Secrets,
  ServiceAccount, PVCs, the Services that select its pods, the Ingresses that route to those
  Services, and HPAs. PVCs are picked by default. Tick **Include the files** to also save each
  volume's contents next to the YAML as `<claim>.data.tar.gz`, copied through the same helper pod
  as the file browser. Stop the app first if you want a consistent copy.
- **Copy** a workload to any saved cluster or namespace, optionally with the ConfigMaps and Secrets it
  references. The target namespace is created if needed.
- **Import** one or many YAML files, or pasted YAML (multi-document and `kind: List` included), into
  any saved cluster. Objects are applied in dependency order, with a dry-run preview first.

## Services and configuration

- **Services** show live endpoint health: how many pods each selector matches and how many of them are
  Ready. They also list the Ingress routes that point at each Service.
- **Ingresses** (a tab on Services) list every host/path with the Service and port it routes to,
  which hosts have TLS, and the address the ingress controller published. Routes to a missing
  Service are marked.
- **Autoscalers** (a tab on Workloads) show each HPA's target, current replicas within its min–max,
  and each metric's current value against its target. Workloads with an HPA show "auto min–max"
  under their replica count.
- **ConfigMaps and Secrets** list their key names and the workloads that use them. The key/value
  editor decodes Secret values and masks them. After you save, it offers to restart the workloads
  that use the object.

## Port-forwarding

Forward any Service port to `localhost` to test an app or open a site from your machine. The default
local port follows the Service port (80→8080, 443→8443); you can pick another.

- It works over SSH connections too.
- It listens on 127.0.0.1 only.
- **It survives restarts.** Unlike `kubectl port-forward`, every new connection goes to a Ready
  pod, so forwards keep working through pod restarts and rollouts.

## Storage and volume files

The **Storage** page lists every PersistentVolumeClaim with:

- its status, capacity, access mode and storage class
- the workloads and pods that use it
- whether its files can be changed right now

Two more tabs cover what's behind the claims:

- **Volumes**: every PersistentVolume with its claim, reclaim policy, and where the data lives (node
  and host path for local-path). A Released volume with a Retain policy still holds its data.
- **Storage classes**: provisioner, reclaim policy, binding mode, whether volumes can grow, which
  class is the default, and how many volumes and claims use each.

**Browsing.** Use **Browse** on the Storage page, or the **Files** tab on a workload that mounts a
volume. You can:

- move through folders and see free space
- download files, or whole folders as `.tar.gz`, with progress and cancel

**Changing files.** You can upload files and folders (drag and drop works), create folders, rename
and delete. This is only allowed while nothing else is using the volume:

- no other pod mounts it, and
- every workload that mounts it is scaled to 0. DaemonSets and unsuspended CronJobs that mount it
  always block changes.

Otherwise the browser is read-only and lists what's in the way. The check runs again right before
every change, so scaling the app back up turns writing off immediately.

**Deleting storage.** The row menu on the Storage page has **Delete storage**. It follows the same
rule as changing files: the claim must not be in use. With most storage classes (k3s `local-path`
included) the volume and its files are deleted with the claim, so this can't be undone.

**How it works.** The app starts a small helper pod that mounts the volume. Close the browser before
scaling the app back up: while the browser is open the volume stays attached to that pod, and on
storage that can only attach to one node at a time, this can block the app from starting. See
[Data and privacy](data-and-privacy.md#on-your-cluster) for the details of the helper pod.

## Logs

- Container logs are pulled into local storage incrementally, with full-text search.
- When a container restarts, the output of the crashed run is captured too.
- **Log explorer:** a volume histogram you can click to zoom, plus filters by namespace, pod,
  workload, level and time.
- **Analytics:** the noisiest pods, recurring error messages (numbers are collapsed so repeats group
  together), and the pods that restart most.
- **Logs outlive their pods.** They stay searchable by workload after the pods are gone (scaled to
  0, redeployed, deleted or archived).

## Workload overview and archives

**Workload overview.** Click any workload to see its health, images and running pods, every pod
that still has stored logs, its events and YAML. It opens even when nothing is running.

**Archive** saves a workload, the objects it needs and (optionally) its stored logs to a local
folder, then removes it from the cluster.

- The objects it can save: config, Secrets, Services, Ingresses, autoscalers and PVC claims. The
  claim is saved, not the data on the volume.
- Objects that another workload still uses are flagged and left on the cluster.
- The folder is plain YAML, so `kubectl apply -f` works on it too.

**Restore.** Archived workloads have their own tab under **Workloads**. **Restore** deploys an
archived workload again, to its own cluster or any other, with a dry run first.

## Background mode

- **Tray.** Closing the window hides it to the system tray, and monitoring continues.
- **Notifications.** You get a desktop notification when any saved cluster has a new critical
  problem or becomes unreachable, once per problem.
- **Checks.** The active cluster is polled at the normal rate. The other saved clusters are checked
  every 15 minutes (configurable).
- **Pause.** Pause and resume all monitoring from the tray menu or Settings. Quit from the tray menu.

## Metrics history

Node and pod CPU and memory from metrics-server are recorded for the retention window and charted
per node, per pod and for the whole cluster.
