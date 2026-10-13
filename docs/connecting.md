# Connecting to a cluster

Add connections under **Settings → Cluster connections**. You can save as many as you like, mixing
local and SSH ones, and switch between them from the dropdown at the top of the sidebar.

Only the active cluster is fully monitored; the others get a light health check in the background.
Each connection keeps its own stored logs, metrics and problem history. Two connections that point at the same cluster the same way (the same kubeconfig file and context, or the same SSH host and API address) share them.

## Local kubeconfig

Uses `$KUBECONFIG` or `~/.kube/config` by default, or a file you pick, with any context in it. Use
this when you already have `kubectl` access from your machine.

## SSH to a k3s server

Use this when the API server isn't reachable from your machine but SSH is.

**How it works:**

1. Portside connects over SSH, with a key or a password.
2. It runs `sudo -n cat /etc/rancher/k3s/k3s.yaml` to read the cluster credentials. You can change
   the command per connection.
3. It forwards the API server port through the SSH session.

Nothing is installed on the node, unless you ask for it (below).

**No k3s yet?** **Test connection** first checks the host over SSH. If k3s isn't installed it says
so and offers to install it: optionally update the OS packages first (apt, dnf, yum, zypper or
apk), then run the official installer (`curl -sfL https://get.k3s.io | sh -s - server …`) with
the options you tick, each explained in the form: kubeconfig readable without sudo, Traefik,
ServiceLB, local-path storage, the metrics server, Secrets encryption, embedded etcd, a pinned
version and an extra TLS name. The installer's output streams into the form; when the node is
Ready the connection test runs again. The login needs working sudo (passwordless, or the sudo
password in the form). To remove it later, run `/usr/local/bin/k3s-uninstall.sh` on the server.

**Host key.** The server's host key is pinned on first connect. If it changes later, the connection
is blocked until you clear the pinned key in Settings.

### Reading k3s.yaml

k3s writes `k3s.yaml` readable by root only. You have two options:

- **Enter a sudo password** on the connection. Portside passes it to `sudo` when it reads the file.
- **Allow passwordless sudo for that one command** (recommended). On the k3s server:

  ```bash
  echo "$USER ALL=(root) NOPASSWD: /usr/bin/cat /etc/rancher/k3s/k3s.yaml" \
    | sudo tee /etc/sudoers.d/portside-lite
  sudo chmod 440 /etc/sudoers.d/portside-lite && sudo visudo -c
  ```

Starting k3s with `--write-kubeconfig-mode 644` also works, but it lets every user on the node read
the cluster's admin credentials.

## What access is needed

- **Monitoring** needs permission to read cluster objects, events and pod logs. Reading the
  metrics API is optional; without it, the CPU and memory charts are empty.
- **Actions** (restart, scale, edit, import, archive, port-forward, volume files) need permission to
  change the objects involved. Volume files also need permission to create pods and use `pods/exec`
  in the volume's namespace.

The k3s admin kubeconfig has all of this. With a restricted kubeconfig, whatever it can't read
simply doesn't appear in the app.
