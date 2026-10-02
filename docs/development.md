# Development

Portside Lite is a Tauri 2 app: a Rust backend split into Tauri-free crates, and a React +
TypeScript frontend. [CLAUDE.md](../CLAUDE.md) describes the architecture and conventions in detail.

```
crates/portside-core     pure logic: settings, UI types, problem rules, manifests, volume-file scripts
crates/portside-kube     cluster access: connections (local / SSH tunnel), polling, logs, actions
crates/portside-store    SQLite storage: logs (full-text), samples, problem history, archives
crates/portside-monitor  background engine: polling, log pulls, alerts, port-forwards, file sessions
src-tauri                thin Tauri layer: commands, tray, events
src                      React app
```

## Build and run

Build and run on **Windows** (it needs the MSVC toolchain and Windows Node):

```powershell
npm install
npm run tauri dev
```

**UI-only work.** `npm run dev` serves the UI at http://localhost:1420 with a mocked backend and
fixture data (`src/dev/mockTauri.ts`, dev builds only), so it opens in a normal browser.

## Checks

```bash
cargo test -p portside-core -p portside-store -p portside-kube -p portside-monitor
npx tsc --noEmit
```

**WSL.** WSL works for editing and checks, but not for building the app, because the Tauri crate
needs the Windows toolchain. `scripts/check-tauri-wsl.sh` type-checks it there with stubbed system
libraries. CLAUDE.md lists the other WSL caveats.

**Dependencies.** TLS and crypto use `ring` only. Don't add dependencies that pull in `aws-lc-rs` or
OpenSSL: they need cmake, nasm and perl to build on Windows.

## Third-party notices

`THIRD_PARTY_NOTICES.md` is generated from `Cargo.lock` and `package-lock.json`. After adding,
removing or updating dependencies, regenerate it:

```bash
python3 scripts/third-party-notices.py
```

CI fails if it's out of date. Release builds use the same script with `--texts` to produce
`THIRD_PARTY_LICENSES.txt` (the full license texts), which ships with the installer and the
portable zip.
