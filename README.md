<p align="center">
  <img src="app-icon.png" alt="Portside Lite" width="140" />
</p>

# Portside Lite

A desktop app for watching, triaging and fixing a k3s cluster. It's the
client-side sibling of [Portside](../core/portside): no server, no login.
Tauri 2 + Rust backend, React + TypeScript frontend.

## What it does

- **Finds problems.** Every poll runs a set of rules and lists what needs
  attention, worst first, with a plain-language next step:
  - CrashLoopBackOff, ImagePullBackOff, config errors
  - OOMKills, unschedulable/stuck Pending pods, not-ready pods, stuck Terminating
  - NotReady nodes; memory/disk/PID pressure; high CPU/memory
  - degraded or down Deployments/StatefulSets/DaemonSets; failed Jobs
  - Pending/Lost PVCs; probe failures, mount failures and evictions from events
  - error-log spikes per pod
- **Tracks problem history.** When each problem started and resolved, and which
  ones keep coming back.
- **Lets you act.** Restart (delete) a pod; rollout-restart, scale or delete
  a workload; cordon/uncordon a node; view YAML and events. "Scale to 0"
  remembers the replica count (an annotation on the object) so Restore brings
  it back. Delete asks you to type the name and offers a YAML backup first. Destructive actions
  ask first.
- **Edits, exports and copies workloads.** Edit a workload's YAML in-app, with
  a dry-run check before applying. Saves work like `kubectl edit`: if the
  object changed on the cluster meanwhile, the save is refused. Export clean,
  re-appliable YAML for one workload or a whole kind/namespace, or copy a
  workload to any saved cluster or namespace, optionally bringing the
  ConfigMaps and Secrets it references. **Import** applies one or many YAML
  files, or pasted YAML (multi-document and `kind: List` included), to any
  saved cluster in dependency order, with a dry-run preview.
- **Pulls logs locally.** Container logs are pulled incrementally into SQLite
  with full-text search. When a container restarts, the crashed instance's
  tail is captured as well. You get a log explorer with a clickable volume
  histogram, plus analytics: noisiest pods, recurring error messages (with
  numbers collapsed so repeats group), restart leaders.
- **Runs in the background.** Closing the window hides it to the system tray
  and monitoring continues. You get a desktop notification when any saved
  cluster has a new critical problem or becomes unreachable, once per
  problem. The active cluster is watched at the normal poll rate; the others
  are checked every 15 minutes (configurable). Pause/resume all monitoring
  from the tray menu (or Settings); quit from the tray menu too.
- **Records metrics history.** Node and pod CPU/memory from metrics-server,
  kept for the retention window.

## Install

Download `PortsideLite-Setup-<version>.exe` from
[Releases](https://github.com/timothydodd/portside-lite/releases). It installs for your user
account (no admin prompt) and can optionally start Portside Lite in the tray when you sign in.
There's also a portable zip containing just the executable. Check downloads against
`SHA256SUMS.txt`. Release builds are code-signed.

Needs Windows 10 1809 or later. The installer adds the Microsoft Edge WebView2 runtime if it's
missing (Windows 11 already has it).

## Connecting

Settings → **Cluster connections**. Save as many as you like (any mix of
local and SSH) and switch between them from the dropdown at the top of the
sidebar. Only the active cluster is monitored; each keeps its own stored
logs, metrics and problem history.

Each connection is one of:

- **Local kubeconfig.** Uses `$KUBECONFIG` / `~/.kube/config`, or a file you
  pick, with any context.
- **SSH to a k3s server.** Portside SSHes in (key or password), runs
  `sudo -n cat /etc/rancher/k3s/k3s.yaml` to get the cluster credentials, and
  forwards the API server port through the SSH session. Nothing is installed
  on the node. The host key is pinned on first connect; if it changes, the
  connection is blocked.
  - k3s writes `k3s.yaml` root-only. Either enter a **Sudo password** on the
    connection, or (better) let your SSH user run just that one command
    without a password. On the k3s server:
    ```bash
    echo "$USER ALL=(root) NOPASSWD: /usr/bin/cat /etc/rancher/k3s/k3s.yaml" \
      | sudo tee /etc/sudoers.d/portside-lite
    sudo chmod 440 /etc/sudoers.d/portside-lite && sudo visudo -c
    ```

## Develop

> Build and run on **Windows**. WSL is for editing and quick checks only (see CLAUDE.md).

```powershell
npm install
npm run tauri dev
```

For UI work, `npm run dev` and a normal browser at http://localhost:1420 use
a mocked backend with fixture data (`src/dev/mockTauri.ts`, dev only).

## Data

Everything lives in `portside-lite.db` in the app data directory
(`%APPDATA%\com.portside.lite` on Windows). Settings → **Local data** shows
the database size and lets you prune or clear it. SSH passwords and key
passphrases are stored **unencrypted** in that database; prefer key auth.

## Releasing

Push a `vX.Y.Z` tag. The [Build workflow](.github/workflows/build.yml) builds and tests, then
**waits for approval** in the `release` environment. Nothing is signed or published until a
reviewer approves the run (Actions → the run → *Review deployments*). After approval it signs the
executable, builds and signs the Inno Setup installer, smoke-tests install/upgrade/uninstall on a
clean runner, and publishes the GitHub release with SHA256 sums.

```bash
git tag v0.1.0 && git push origin v0.1.0
```

Pushes and PRs to `main` build and test only. *Run workflow* on the Actions tab makes an unsigned
test installer (never published).

### Code signing (one-time setup)

Signing uses Azure Trusted Signing through OIDC, so no certificate or password is stored anywhere.
It switches on once these exist on the repo (until then releases are unsigned):

- **Secrets:** `AZURE_CLIENT_ID`, `AZURE_TENANT_ID`, `AZURE_SUBSCRIPTION_ID`
- **Variables:** `SIGNING_ENDPOINT`, `SIGNING_ACCOUNT`, `SIGNING_PROFILE`
- **In Azure:** the app registration needs a federated credential for
  `repo:timothydodd/portside-lite:environment:release` and the *Trusted Signing Certificate
  Profile Signer* role on the certificate profile.

