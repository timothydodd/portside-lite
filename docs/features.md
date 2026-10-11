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
- ServiceAccounts in use that list a pull secret that doesn't exist
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
  ServiceAccount (with that account's pull secrets, RoleBindings and Roles), PVCs, the Services
  that select its pods, the Ingresses that route to those Services, and HPAs. A ServiceAccount can
  be exported or copied the same way, with its pull secrets and permissions. PVCs are picked by default. Tick **Include the files** to also save each
  volume's contents next to the YAML as `<claim>.data.tar.gz`, copied through the same helper pod
  as the file browser. Stop the app first if you want a consistent copy.
- **Copy** a workload or ServiceAccount to any saved cluster or namespace, optionally with the objects
  it references. The target namespace is created if needed, and copied RoleBindings point at the
  copied account.
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
- **Service accounts** (a tab on Config) show each account's pull secrets (missing ones are marked),
  the Roles and ClusterRoles bound to it, and the workloads that run as it.
- **Registries** (a tab on Config) list every registry images are pulled from, plus any registry a
  pull secret has credentials for: the images in use, the workloads pulling, which pull secrets
  cover it (and through which ServiceAccount), workloads pulling without a credential, and current
  pull errors. **Add credential** creates a pull secret (like `kubectl create secret
  docker-registry`) and can add it to a ServiceAccount. Credentials set on the nodes in k3s
  `registries.yaml` aren't visible to the app.

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

## Volume backups

The **Backups** page copies volumes (PersistentVolumeClaims) on a schedule. Start from **Back up a
volume…** there, or **Back up on a schedule…** in a claim's row menu on the Storage page. Only the
volumes you pick are backed up.

You choose:

- **Take the app offline** (on by default): the Deployments and StatefulSets using the volume are
  scaled to 0 before the copy and back to their size right after. Nothing is writing while the volume
  is read, so databases and other files that change are copied whole. The app is down for the copy.
  Turn it off to copy while the app runs; then a file being written at the time can be saved
  half-written. DaemonSets, CronJobs and bare pods can't be stopped this way, so the dialog says if
  any of those use the volume.
- **How often:** every few hours, daily, weekly or monthly at a set time, or any cron expression,
  in a time zone of your choice (your PC's by default)
- **How many to keep:** older copies are deleted after each run
- **When to warn:** if there's been no successful backup for this many hours, it shows on Problems
  and sends an alert. The default is one interval plus some slack (36 h for a daily backup).

**How it works.** Each backup is a CronJob plus its own backup volume, both named
`portside-backup-<name>`, in the volume's namespace. The job mounts the volume read-only and writes
a dated `.tar.gz` to the backup volume, keeping it only if the archive checks out. When it takes the
app offline it uses its own ServiceAccount, whose Role can only read and scale the chosen workloads
and list pods. The app is started again right after the copy, and also if the run fails or is
stopped. If a run is killed hard (say the node goes down) and the app is left at 0, Problems shows
**Left stopped by a backup**, and the scale dialog offers Restore. Because the cluster runs the
jobs, backups happen whether or not Portside is open. They're tested with k3s `local-path`; storage
that attaches to one node at a time may need the job on the app's node.

**Row actions:** Run now, Change, Browse backups (opens the backup volume in the file browser so you
can download archives), Logs of the last run, Pause / Resume, and Remove. Remove asks separately
whether to delete the stored archives too.

**Backups to this PC.** Under Settings → Backups to this PC, two jobs run on this PC for every saved
connection, each on its own schedule (every few hours, daily, weekly or monthly at a time, in this
PC's time):

- **Copy volume backups to this PC** (on by default): copies the archives each volume backup keeps
  on its cluster into the backup folder, only the ones not already there, and keeps the newest N
  per volume. Until they're copied, backups sit on the same disk as the data.
- **Back up manifests and cluster settings** (on by default): saves every namespace's workloads,
  Services, ConfigMaps, claims, Ingresses, autoscalers and access rules, plus cluster-wide settings
  (namespaces, storage classes, cluster roles and bindings, ingress and priority classes, CRDs, and
  k3s's HelmChart/HelmChartConfig objects), as YAML you can bring back with **Import**. Objects a
  controller recreates and ones the cluster makes for itself are left out. **Secret values are
  never saved**; `secrets-needed.md` lists each Secret's name, type and key names so you know what
  to recreate.

On disk: `<backup folder>/<connection>/volumes/<namespace>/<volume>/` and
`<backup folder>/<connection>/manifests/<date_time>/` (with a README). A run missed while the PC
was off happens soon after Portside starts again; a failed one is retried an hour later. The
Backups page shows each connection's last copy and manifest backup (and any error), what's running,
and how many copies of each volume are on this PC.

A failed run shows as **Volume backup failed** on Problems, and one that's overdue as **Volume
backup is overdue**. Both offer **Run backup now**.

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

- The objects it can save: config, Secrets, the ServiceAccount and its RoleBindings and Roles,
  Services, Ingresses, autoscalers and PVC claims. The
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
