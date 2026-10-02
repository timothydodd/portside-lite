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

**Credentials.** SSH passwords, key passphrases and sudo passwords are stored **unencrypted** in that
database. Prefer key authentication. Keep in mind that anyone who can read your user profile can
read the file.

**Archives** are written to the archive folder (Settings → Archives; default `archives` in the app
data folder). They contain the workload's YAML and, if you choose, its stored logs. **Secrets saved
in an archive are in plain YAML.**

**Volume file downloads** go wherever you save them.

## On your cluster

The app changes your cluster only when you take an action. The changes are what you'd expect from
the action, plus the following:

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
