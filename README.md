<p align="center">
  <img src="app-icon.png" alt="Portside Lite" width="140" />
</p>

# Portside Lite

A Windows desktop app for watching, triaging and fixing k3s clusters. It runs on your machine, so
there's no server to host and no login. Built with Tauri 2 (Rust) and React.

## Features

- **Problems:** crash loops, OOMKills, stuck pods, node pressure, failing workloads and more, each
  with a plain-language next step and a history of when it started and resolved.
- **Actions:** restart pods, rollout-restart, scale or delete workloads, cordon nodes.
- **YAML:** edit with a dry run first, export, copy to another cluster, import.
- **Services and config:** endpoint health, Ingress routes, ConfigMap and Secret editing, and
  port-forwards that survive pod restarts.
- **Storage:** browse files on any PersistentVolumeClaim and download them. You can upload or
  change files once nothing else is using the volume.
- **Logs:** pulled into local, full-text-searchable storage, including the output of crashed
  containers. Still there after the pods are gone.
- **Archives:** save a workload and what it needs to a folder, remove it, restore it later.
- **Background mode:** lives in the system tray and notifies you about new critical problems on
  any saved cluster.

More detail in [docs/features.md](docs/features.md).

## Install

Download `PortsideLite-Setup-<version>.exe` (or the portable zip) from
[Releases](https://github.com/timothydodd/portside-lite/releases). It installs per user with no admin
prompt. Release builds are code-signed; check downloads against `SHA256SUMS.txt`.

Needs Windows 10 1809 or later. The installer adds the Microsoft Edge WebView2 runtime if it's
missing.

## Connect

Open **Settings → Cluster connections** and add either:

- a **local kubeconfig** (any file and context), or
- **SSH to a k3s server**. Portside reads the cluster credentials over SSH and tunnels the API
  server through the connection. Nothing is installed on the node.

Save as many as you like and switch from the sidebar. Setup details, including passwordless sudo
for the SSH option, are in [docs/connecting.md](docs/connecting.md).

## Documentation

| | |
| --- | --- |
| [Features](docs/features.md) | Everything the app does, in detail |
| [Connecting](docs/connecting.md) | Local kubeconfig and SSH setup |
| [Data and privacy](docs/data-and-privacy.md) | What's stored locally, and what the app creates on your cluster |
| [Development](docs/development.md) | Building and running from source |
| [Releasing](docs/releasing.md) | Release pipeline and code signing (maintainers) |

## License

Portside Lite is released under the [MIT License](LICENSE).

It's built on open-source components that keep their own licenses: mostly MIT and Apache-2.0,
plus a few BSD, ISC, Unicode-3.0 and MPL-2.0 ones. See
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for the full list with attributions. Every release
includes the full license texts (`THIRD_PARTY_LICENSES.txt`).

Kubernetes is a registered trademark of The Linux Foundation. Portside Lite isn't affiliated with
or endorsed by the Kubernetes or k3s projects.
