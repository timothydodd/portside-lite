# Data and privacy

Portside Lite has no server and sends no telemetry. It talks only to the clusters you configure. It
also contacts the hosts you set up for SSH connections and, if you start one, the targets of a
port-forward.

## On your computer

Everything is stored in `portside-lite.db` in the app data folder (`%APPDATA%\com.portside.lite`):

- settings and saved connections
- stored container logs, metrics samples and problem history (kept for the retention period you set)
- which pods belong to which workload, so logs stay searchable by workload

Under **Settings → Local data** you can see the database size and prune or clear it.

**Credentials.** SSH passwords, key passphrases and sudo passwords are encrypted in that database
with Windows DPAPI, so only your Windows account on this computer can read them. A copied
database is no use elsewhere; after restoring one on another computer or account, type the
passwords again. Passwords saved by older versions are encrypted the first time this version
starts. The settings screen never receives a saved password: it shows "Saved" and only sends a new
one when you type it. Programs running as you can still ask Windows to decrypt them, so key
authentication remains the better choice.

**Archives** are written to the archive folder (Settings → Archives; default `archives` in the app
data folder). They contain the workload's YAML and, if you choose, its stored logs. **Secrets saved
in an archive are in plain YAML.**

**Volume file downloads** go wherever you save them.

**Backups to this PC** go to the backup folder (Settings → Backups to this PC; default `backups` in
the app data folder), one folder per connection. Copied volume backups are the volume's files as
they were, in plain `.tar.gz` archives, so they hold whatever the app stored there (database files
included). Manifest backups are plain YAML **without Secret values**: `secrets-needed.md` lists
Secret names, types and key names only.

## On your cluster

The app changes your cluster only when you take an action, with one standing exception you can
turn off: copying volume backups to this PC (below). The changes are what you'd expect from the
action, plus the following:

- **Scale to 0** adds the annotation `portside-lite/disabled-replicas` to remember the replica count.
- **Copy and import** use server-side apply with field manager `portside-lite`.
- **Rollout restart** sets the standard `kubectl.kubernetes.io/restartedAt` annotation on the pod
  template.
- **Volume files.** Browsing a volume creates a pod named `portside-files-…` in the volume's namespace:
  - It has the label `portside-lite/files=true` and is annotated with the claim it mounts.
  - It runs the image set under **Settings → Volume files** (default `busybox:1.37`, pulled by your
    cluster), as root so that uploaded files can keep the right owner.
  - It mounts the volume read-only unless nothing else is using it.
  - It's deleted shortly after you close the browser, when you switch clusters, and when you quit.
    If the app crashes, the pod stops on its own after two hours.
  - **Pod Security policies.** A namespace whose policy forbids root pods will refuse the helper pod.
- **Volume backups** you set up create, in the volume's namespace, a CronJob and a backup volume named
  `portside-backup-<name>` (plus a ServiceAccount, Role and RoleBinding of the same name when the
  app is taken offline). The Role can only read and scale the workloads you picked, and list pods.
  The job runs `curlimages/curl` as root to read every file, mounts your volume read-only, and while
  it copies, scales those workloads to 0 with the `portside-lite/disabled-replicas` and
  `portside-lite/backup-offline` annotations. Removing the backup deletes all of it; the backup
  volume only if you say so.
- **Copying volume backups to this PC** (on by default, Settings → Backups to this PC) starts a
  read-only `portside-files-…` helper pod on each backup volume, on its schedule, and deletes it when
  the copy is done. It reads; it never changes anything.
- **Manifest backups** only read.
