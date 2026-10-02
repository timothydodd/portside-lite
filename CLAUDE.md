# CLAUDE.md

Guidance for working on **Portside Lite**, a desktop monitor/triage tool for k3s clusters. Tauri 2 + Rust backend, React + TypeScript frontend. The UI and conventions follow `../curlew`; the domain follows `../core/portside`.

## Build & verify

> **Run dev/prod builds from Windows, not WSL.** `npm run tauri dev` / `tauri build` need the MSVC toolchain + Windows Node. WSL is for editing and checks only.

**In WSL (checks only):**
```bash
export CARGO_TARGET_DIR=$HOME/.cargo-target/portside-lite
cargo test -p portside-core -p portside-store -p portside-kube -p portside-monitor
scripts/check-tauri-wsl.sh        # type-checks src-tauri using stub pkg-config files
npx tsc --noEmit
npx vite build
```

- `src-tauri` can't *build* in WSL (no GTK/webkit libs), but `scripts/check-tauri-wsl.sh` gives it a real `cargo check`.
- Running `npm install` from WSL swaps in Linux native binaries (rollup, tailwind oxide). Run `npm install` again on Windows before building.
- Vite's file watcher doesn't see edits on `/mnt/f` from WSL. Restart the dev server to pick them up.
- Once `node_modules` has been installed from Windows, don't `npm install` in WSL. For a browser check from WSL, copy `src/`, `public/`, `index.html`, `package*.json`, `vite.config.ts` and `tsconfig*.json` to a temp dir, `npm ci` there, and run Vite on a port other than 1420 (`tauri dev` uses 1420). `npx tsc --noEmit` still works in place.
- TLS/crypto is **ring only** (kube `ring` feature, russh `ring` feature). Don't let `aws-lc-rs` or OpenSSL into the tree: they need cmake/nasm/perl on Windows. Check with `cargo tree -i aws-lc-rs --workspace`.

## Architecture

```
crates/portside-core     settings, UI DTOs (models.rs), quantity parsing, log-line
                         parsing/level detection, summarize.rs (k8s objects → DTOs),
                         issues.rs (problem rules). Pure; unit-tested.
crates/portside-kube     connection (local kubeconfig | SSH tunnel), collect.rs (one
                         poll + metrics.k8s.io), logs.rs, actions.rs (delete pod,
                         rollout restart, scale, cordon, YAML, events).
crates/portside-store    SQLite: settings, node/pod samples, logs + FTS5, log cursors,
                         issue history, retention.
crates/portside-monitor  background engine: poll loop + log loop, reconnects, TOFU host
                         key pinning; pushes via the EventSink trait.
src-tauri                thin: commands.rs wraps the crates; lib.rs forwards EventSink
                         to webview events.
src                      React app: pages/, components/, stores/ (Zustand), lib/.
```

**Layering rule:** only `src-tauri` imports Tauri. Put testable logic in the crates.

## Key conventions

- **Connection profiles.** `Settings.connections` holds named profiles (any mix of local/SSH) plus `active_connection_id`; only the active one is polled. `Settings::normalize()` (run on load and save) migrates the old single `connection` field and repairs a dangling active id. Data is partitioned by `Connection::cluster_id()`, so each profile keeps its own logs and history. `poll_once` captures the cluster id at the start and discards the result if the user switched mid-poll. Host keys are pinned onto the profile that was connected to, not whichever is active.
- **Manifests** (`portside-core/manifest.rs` = pure shaping; `portside-kube/manifests.rs` = cluster ops). Kinds resolve through API discovery (`pinned_kind`), never guessed plurals. Export = `clean_for_export` (like kubectl-neat). The editor loads `clean_for_edit` (keeps `resourceVersion`) and saves with `replace`, so a 409 means someone else changed it; the edit may not change kind/name/namespace. Copy = server-side apply with field manager `portside-lite`, target namespace auto-created, dependencies from `references()`. Copy targets connect through `Monitor::connect_profile`, which reuses the active client or makes a one-off connection.
- **Service mode.** `on_window_event(CloseRequested)` hides to tray when `close_to_tray` is set; the tray (id `main`) has Open / Check now / Quit. `tauri-plugin-single-instance` makes a second launch focus the running one. Alerts: `portside-core/alerts.rs` decides what's new (notify once per issue key, re-arm when it clears); `Monitor::process_alerts` / `record_outage` fire `EventSink::notify` and keep the tray tooltip current. The active cluster alerts from `poll_once` (3 failed polls = unreachable); others come from `run_sweep_loop` every `background_check_minutes`, with a 60 s timeout per cluster.
- **Related objects** (`manifest::related_objects`, pure) drive export bundles and copy: pod-spec `references()`, Services whose selector matches the pod template labels, Ingresses whose backends hit those Services, HPAs whose `scaleTargetRef` is the workload. `clean_for_export` also strips per-kind cluster-assigned fields (Service clusterIP(s) and non-NodePort nodePorts, PVC volumeName and binding annotations, ServiceAccount token refs). The snapshot carries Services (endpoints computed from pod labels + Ready) and ConfigMaps/Secrets as **key names and sizes only**; values are fetched on demand by `config::get_config` and saved by a guarded `replace`.
- **Port-forwards** (`portside-kube/portforward.rs` = resolve Service → Ready pod + container port, incl. named `targetPort`; `portside-monitor/forwards.rs` = manager). One 127.0.0.1 listener per forward; each accepted connection opens its own kube `portforward` stream (kube `ws` feature, still ring-only). On failure it re-resolves the pod with a fresh client via `connect_profile`, so forwards survive restarts. Stopping sends a `watch` shutdown that closes the listener and live connections. State reaches the UI through `EventSink::forwards_changed` → `forwards:changed`.
- **Pause** (`Settings.monitoring_paused`, tray toggle + `set_monitoring_paused`) stops the poll, log and sweep loops; it persists. `update_settings` always emits `settings_changed`, so the tray label and UI stay in step. **Import** = `manifest::parse_sources` (multi-doc, `*List` expansion, per-doc errors, stable `index`) then `manifests::import_docs` (SSA in `apply_order`). Dry runs into a missing namespace report "will be created" instead of failing (`namespace_ready`); copy uses the same helper.
- **SSH mode = tunnel, not kubectl.** We read the node's kubeconfig over SSH, then forward `127.0.0.1:<random>` → `api_host:api_port` over `direct-tcpip`, so SSH and local share one `kube::Client`. The k3s serving cert covers 127.0.0.1.
- **Issues** have stable keys (`<rule>:<Kind>/<ns>/<name>[/<container>]`). `Store::sync_issues` opens, touches and resolves history rows by key, so changing a rule's key format resets its history. Add a rule in `issues.rs` with a unit test; give it `actions` the UI can run (`ActionKind`).
- **Workload drawer + archives.** `WorkloadDrawer` opens for any workload, live or not: stored logs, the archive, events, YAML (live or from `manifest.yaml`). Stored logs find a workload's pods through `pod_owners` (recorded every poll from the snapshot's owner fields), with `archive::pod_name_matches` as a fallback for lines stored before owners were recorded; `LogQuery.workload` needs `namespace`. **Archive** = `archive::plan` (related objects + `used_by` from other workloads) → `export_bundle` → parse-back check → `portside-store/archive.rs` writes `<root>/<cluster>/<ns>/<kind>-<name>/{manifest.yaml,archive.json,logs.txt}` (staged, then swapped in) → only then `archive::remove_objects` (reverse apply order; `remove` must be a subset of what was saved). Restore = `import_docs` on `manifest.yaml`; a clean run stamps `restoredMs`. Root = `Settings.archive_dir` or `<app data>/archives`; ids are validated by `valid_archive_id` so they can't escape it.
- **Volume files** (`portside-core/files.rs` = paths, busybox scripts, listing parser, helper pod spec, `claim_users`; `portside-kube/files.rs` = helper lifecycle + exec streaming; `portside-monitor/files.rs` = sessions). Browsing a PVC starts a helper pod (`generateName: portside-files-`, label `portside-lite/files=true`, claim in the `portside-lite/claim` annotation, `Settings.files_helper_image`) that exits on its own after 2 h. Scripts get paths as positional args (`sh -c <script> sh <args>`), never spliced into shell text. **Writes only when nothing else uses the claim**: no non-helper pod mounts it and every workload mounting it is scaled to 0 (DaemonSets and unsuspended CronJobs always block). Otherwise the helper mounts it read-only, pinned to the app pod's node. Every write re-checks `claim_users` live. Sessions are ref-counted per (profile, ns, claim) and deleted 30 s after the last view closes, on cluster switch, and on `RunEvent::Exit`. Uploads stream exactly N bytes into `head -c N` (works without v5 stdin-close) to a temp file, then rename, keeping the owner. Folder downloads are `.tar.gz`.
- **Log pulls are incremental per (pod UID, container)** via `log_cursors`. `sinceTime` is inclusive, so `parse_lines` drops lines at or before the cursor (ns precision). Lines and the cursor are written in one transaction. A higher restart count triggers a `previous=true` pull first.
- **DTOs are camelCase** (serde) and mirrored by hand in `src/lib/types.ts`; keep them in sync. IPC wrappers live in `src/lib/ipc.ts`.
- **Events to the UI:** `cluster:snapshot`, `cluster:status`, `settings:changed`, `logs:synced` (store in `src/stores/cluster.ts`).
- **Theming:** Portside's Dracula/neon tokens in `src/index.css` (dark default, `[data-theme="light"]` variant). Use the semantic utilities (`bg-surface`, `text-content`, `border-border`, `text-critical`, …) and component classes (`btn-*`, `field`, `card`, `table`, `navtab`, `tint-*`). Status colors (`critical/warning/info/good`) are for state only and always come with an icon or label. Log-level colors are `--lvl-*`.
- **Charts** (`components/charts.tsx`): one series and one y-axis per chart, a 2px line with a 10% wash, bars ≤24px with 2px gaps, a hover tooltip on every chart, and text in text tokens.
- **Zustand selectors must return existing references.** `useClusterStore((s) => s.snapshot?.issues.filter(...))` returns a new array every call; zustand 5 then re-renders forever ("getSnapshot should be cached" → "Maximum update depth exceeded"). Select the raw field and derive with `useMemo`. Pages and drawers sit inside `ErrorBoundary`, so a render crash shows an error panel instead of a blank window, but fix the cause anyway.
- **Licenses.** The app is MIT (`LICENSE`). `THIRD_PARTY_NOTICES.md` is generated by `scripts/third-party-notices.py` from the lockfiles; rerun it after any dependency change (CI runs `--check`). Releases ship `THIRD_PARTY_LICENSES.txt` (`--texts`: every shipped package's license files, falling back to `scripts/license-templates/` when a package has none) next to the exe, in the zip and as a release asset. User docs live in `docs/`; keep README short and keep account IDs and other identifying setup details out of public docs.
- **Dialogs:** use `confirmDestructive` (`src/lib/dialog.ts`), never `window.confirm`. Actions go through `runAction` in `src/lib/actions.ts`.
