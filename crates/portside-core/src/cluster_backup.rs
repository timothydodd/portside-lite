//! Cluster backups made on this PC: what goes in one beyond the plain
//! manifests (sealed Secret values, Helm releases, custom resources, the k3s
//! node's own files, images that exist only on the node, a datastore
//! snapshot), and how a rebuild puts them back. Pure; the cluster and SSH
//! calls are in `portside-kube` (`manifests::manifest_backup`, `node`), the
//! files and the rebuild in `portside-monitor`.
//!
//! One backup folder (`<backup root>/<cluster>/manifests/<stamp>/`):
//!
//! - `backup.json`: [`BackupIndex`], what's in it.
//! - `cluster/`, `namespaces/<ns>/`: one multi-document YAML file per kind.
//! - `helm-releases.md`, `images.md`, `secrets-needed.md`, `README.md`.
//! - `node/<path>`: the k3s server's config and custom manifests (SSH only).
//! - [`SEALED_FILE`]: Secret values, Helm release records and values, and
//!   node files holding credentials, sealed with the backup passphrase.
//! - [`DATASTORE_FILE`]: optional snapshot of the k3s datastore, sealed.
//!
//! Images only the node has are exported once per image to
//! `<backup root>/<cluster>/images/` (see [`image_file_name`]).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Read;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backup::{BackupSpec, BACKUP_LABEL, SPEC_ANNOTATION};

pub const INDEX_FILE: &str = "backup.json";
/// Everything sealed with the passphrase except the datastore: a `.tar.gz` inside age.
pub const SEALED_FILE: &str = "secrets.tar.gz.age";
pub const DATASTORE_FILE: &str = "datastore.tar.gz.age";
/// Node files live under `node/` in the backup, at their path on the node.
pub const NODE_DIR: &str = "node";
/// Exported images, next to the dated backups (they rarely change and can be large).
pub const IMAGES_DIR: &str = "images";
/// Inside the sealed file: one multi-document file of every Secret saved.
pub const SEALED_SECRETS: &str = "secrets/Secret.yaml";
/// Inside the sealed file: the user-supplied values of each Helm release.
pub const SEALED_HELM_VALUES: &str = "helm-values";

/// What a backup holds, written next to it as [`INDEX_FILE`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct BackupIndex {
    pub version: u32,
    /// The connection's name when it was taken.
    pub cluster: String,
    pub cluster_id: String,
    pub taken_ms: i64,
    pub server_version: Option<String>,
    /// Objects in the YAML files.
    pub objects: usize,
    /// Secrets whose values are in [`SEALED_FILE`]; 0 when nothing was sealed.
    pub sealed_secrets: usize,
    /// [`SEALED_FILE`] exists.
    pub sealed: bool,
    pub helm_releases: usize,
    pub custom_resources: usize,
    pub node: Option<NodeSummary>,
    /// `etcd` or `sqlite` when [`DATASTORE_FILE`] exists.
    pub datastore: Option<String>,
    /// Images only the node had: their files under `../../images/`.
    pub images: Vec<ImageFile>,
    pub volume_backups: Vec<VolumeBackupRef>,
    /// Kinds left out (not served, not allowed).
    pub skipped: Vec<String>,
    /// Parts that couldn't be saved this time.
    pub warnings: Vec<String>,
}

pub const INDEX_VERSION: u32 = 1;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct NodeSummary {
    pub hostname: String,
    pub k3s_version: Option<String>,
    /// `etcd` | `sqlite` | none found.
    pub datastore: Option<String>,
    pub arch: String,
    /// Saved under `node/` in plain text.
    pub files: Vec<String>,
    /// Saved in [`SEALED_FILE`] (they hold credentials).
    pub sealed_files: Vec<String>,
    /// Holding credentials, but left out: no passphrase is set.
    pub left_out: Vec<String>,
    /// The k3s service's `ExecStart` (install flags), for reference.
    pub exec_start: Option<String>,
    /// Files in the node's `agent/images` folder (k3s imports them at start).
    pub agent_images: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", default)]
pub struct ImageFile {
    pub image: String,
    pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase", default)]
pub struct VolumeBackupRef {
    pub namespace: String,
    /// The backup's name (its folder under `volumes/<namespace>/`).
    pub name: String,
    pub claim: String,
}

fn s<'a>(v: &'a Value, path: &str) -> Option<&'a str> {
    v.pointer(path).and_then(Value::as_str)
}

fn ns_name(v: &Value) -> (String, String) {
    (s(v, "/metadata/namespace").unwrap_or_default().to_string(), s(v, "/metadata/name").unwrap_or_default().to_string())
}

fn controlled(v: &Value) -> bool {
    v.pointer("/metadata/ownerReferences").and_then(Value::as_array).is_some_and(|o| o.iter().any(|r| r["controller"] == Value::Bool(true)))
}

// --- extra kinds ----------------------------------------------------------------------

/// A static PersistentVolume (made by hand, or by something other than a
/// provisioner) is worth saving; a provisioned one comes back by itself when
/// its claim does.
pub fn keep_persistent_volume(pv: &Value) -> bool {
    let ann = pv.pointer("/metadata/annotations");
    ann.and_then(|a| a.get("pv.kubernetes.io/provisioned-by")).is_none()
}

/// Services with no selector: their Endpoints are made by hand.
pub fn selectorless_services(services: &[Value]) -> HashSet<(String, String)> {
    services
        .iter()
        .filter(|svc| s(svc, "/spec/type") != Some("ExternalName"))
        .filter(|svc| svc.pointer("/spec/selector").and_then(Value::as_object).is_none_or(|m| m.is_empty()))
        .map(ns_name)
        .collect()
}

/// Endpoints are saved only for Services without a selector (the rest are
/// the endpoints controller's).
pub fn keep_endpoints(ep: &Value, selectorless: &HashSet<(String, String)>) -> bool {
    selectorless.contains(&ns_name(ep))
}

/// EndpointSlices nothing in the cluster manages.
pub fn keep_endpoint_slice(slice: &Value) -> bool {
    let managed = s(slice, "/metadata/labels/endpointslice.kubernetes.io~1managed-by");
    !controlled(slice) && !matches!(managed, Some("endpointslice-controller.k8s.io" | "endpointslicemirroring-controller.k8s.io"))
}

/// API groups whose custom resources k3s makes and manages itself, or that a
/// backup already covers by kind.
const SKIP_CR_GROUPS: &[&str] = &["helm.cattle.io", "k3s.cattle.io"];

/// Where a CRD's objects are served: enough to list them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrdResource {
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
}

impl CrdResource {
    /// File name for its objects: `<Kind>.<group>.yaml` (kinds repeat across groups).
    pub fn file_name(&self) -> String {
        crate::archive::safe_segment(&format!("{}.{}.yaml", self.kind, self.group))
    }
}

/// The version to read a CRD's objects at (its storage version, if served),
/// or `None` for groups k3s manages.
pub fn crd_resource(crd: &Value) -> Option<CrdResource> {
    let group = s(crd, "/spec/group")?;
    if SKIP_CR_GROUPS.contains(&group) {
        return None;
    }
    let versions = crd.pointer("/spec/versions")?.as_array()?;
    let served = |v: &&Value| v["served"] == Value::Bool(true);
    let version = versions
        .iter()
        .filter(served)
        .find(|v| v["storage"] == Value::Bool(true))
        .or_else(|| versions.iter().find(served))?;
    Some(CrdResource {
        group: group.to_string(),
        version: version["name"].as_str()?.to_string(),
        kind: s(crd, "/spec/names/kind")?.to_string(),
        plural: s(crd, "/spec/names/plural")?.to_string(),
        namespaced: s(crd, "/spec/scope") != Some("Cluster"),
    })
}

/// A custom resource worth saving: not made by a controller.
pub fn keep_custom_resource(obj: &Value) -> bool {
    !controlled(obj)
}

// --- Secrets and Helm releases ----------------------------------------------------------

/// Namespaces whose Secrets are the cluster's own (k3s serving certs, node
/// passwords): putting them on a new cluster would break it.
const SYSTEM_SECRET_NAMESPACES: &[&str] = &["kube-system", "kube-public", "kube-node-lease"];

pub const HELM_RELEASE_TYPE: &str = "helm.sh/release.v1";

/// The newest record of each Helm release, as (namespace, Secret name).
/// Older revisions only matter for `helm rollback` and aren't saved.
pub fn latest_helm_secrets(secrets: &[Value]) -> HashSet<(String, String)> {
    let mut newest: HashMap<(String, String), (u64, String)> = HashMap::new();
    for sec in secrets.iter().filter(|v| s(v, "/type") == Some(HELM_RELEASE_TYPE)) {
        let (ns, name) = ns_name(sec);
        let release = s(sec, "/metadata/labels/name").unwrap_or(&name).to_string();
        let rev = s(sec, "/metadata/labels/version").and_then(|v| v.parse().ok()).unwrap_or(0);
        let e = newest.entry((ns, release)).or_insert((0, String::new()));
        if rev >= e.0 {
            *e = (rev, name);
        }
    }
    newest.into_iter().map(|((ns, _), (_, name))| (ns, name)).collect()
}

/// Whether a Secret's values go in the sealed file: not a token the cluster
/// makes, not the cluster's own, not made by a controller, and for Helm
/// release records only the newest revision.
pub fn seal_secret(secret: &Value, latest_helm: &HashSet<(String, String)>) -> bool {
    let (ns, _) = ns_name(secret);
    if SYSTEM_SECRET_NAMESPACES.contains(&ns.as_str()) || controlled(secret) {
        return false;
    }
    match s(secret, "/type").unwrap_or("Opaque") {
        "kubernetes.io/service-account-token" | "bootstrap.kubernetes.io/token" => false,
        HELM_RELEASE_TYPE => latest_helm.contains(&ns_name(secret)),
        _ => true,
    }
}

/// One Helm release, read from its newest release record.
#[derive(Debug, Clone, PartialEq)]
pub struct HelmRelease {
    pub namespace: String,
    pub name: String,
    pub revision: u64,
    pub status: String,
    pub chart: String,
    pub chart_version: String,
    pub app_version: Option<String>,
    /// Where the chart says it comes from (`home`, `sources`): Helm doesn't record the repo.
    pub links: Vec<String>,
    /// The values given at install/upgrade (`helm get values`), as YAML. Can hold secrets.
    pub values: Option<String>,
}

/// Decode a `helm.sh/release.v1` Secret (base64 → gzip → JSON, inside the
/// Secret's own base64).
pub fn helm_release(secret: &Value) -> Option<HelmRelease> {
    if s(secret, "/type") != Some(HELM_RELEASE_TYPE) {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD;
    let outer = b64.decode(s(secret, "/data/release")?).ok()?;
    let inner = b64.decode(outer.trim_ascii()).ok()?;
    let json = if inner.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(inner.as_slice()).read_to_end(&mut out).ok()?;
        out
    } else {
        inner
    };
    let r: Value = serde_json::from_slice(&json).ok()?;
    let meta = &r["chart"]["metadata"];
    let mut links: Vec<String> = meta["home"].as_str().into_iter().map(str::to_string).collect();
    links.extend(meta["sources"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string));
    links.dedup();
    let values = r.get("config").filter(|c| c.as_object().is_some_and(|m| !m.is_empty())).and_then(|c| serde_yaml::to_string(c).ok());
    Some(HelmRelease {
        namespace: r["namespace"].as_str().map(str::to_string).unwrap_or_else(|| ns_name(secret).0),
        name: r["name"].as_str()?.to_string(),
        revision: r["version"].as_u64().unwrap_or(0),
        status: r["info"]["status"].as_str().unwrap_or("unknown").to_string(),
        chart: meta["name"].as_str().unwrap_or("?").to_string(),
        chart_version: meta["version"].as_str().unwrap_or("?").to_string(),
        app_version: meta["appVersion"].as_str().map(str::to_string),
        links,
        values,
    })
}

/// `helm-releases.md`: what was installed with Helm, without any values.
pub fn helm_releases_md(releases: &[HelmRelease], values_sealed: bool) -> String {
    let mut out = String::from("# Helm releases\n\n");
    if releases.is_empty() {
        out.push_str("None found (releases installed with the `helm` command keep a record Secret per revision; there were none).\n");
        return out;
    }
    out.push_str(if values_sealed {
        "Their objects are in this backup, and so is each release's newest record (sealed), so `helm upgrade` keeps working after a rebuild. \
         The values each was installed with are in the sealed file under `helm-values/`.\n\n"
    } else {
        "Their objects are in this backup, but the release records and values aren't (they're Secrets; set a backup passphrase to include them). \
         To manage these with Helm again, reinstall them with the same chart and version.\n\n"
    });
    out.push_str("| Namespace | Release | Chart | Version | App version | Revision | Status | From |\n|---|---|---|---|---|---|---|---|\n");
    for r in releases {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.namespace,
            r.name,
            r.chart,
            r.chart_version,
            r.app_version.as_deref().unwrap_or(""),
            r.revision,
            r.status,
            r.links.join(" ")
        ));
    }
    out
}

// --- images -----------------------------------------------------------------------------

/// An image no registry is known to have: pulled never, or imported onto the node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeImage {
    /// Full reference, e.g. `docker.io/library/app:dev`.
    pub image: String,
    /// The runtime's image id (`sha256:…`).
    pub image_id: String,
    pub used_by: Vec<String>,
    pub reason: &'static str,
}

/// `app` → `docker.io/library/app:latest`, the name containerd keeps it under.
pub fn full_image_ref(image: &str) -> String {
    let (host, path) = crate::registry::split_image(image);
    let path = if host == "docker.io" && !path.contains('/') { format!("library/{path}") } else { path };
    let last = path.rsplit('/').next().unwrap_or_default();
    let tagged = path.contains('@') || last.contains(':');
    format!("{host}/{path}{}", if tagged { "" } else { ":latest" })
}

/// Only what an image reference can contain, so it can sit on a command line.
pub fn plain_image_ref(image: &str) -> bool {
    !image.is_empty() && !image.starts_with('-') && image.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '/' | ':' | '@' | '_' | '-'))
}

/// Images the pods run that a new cluster couldn't pull: `imagePullPolicy:
/// Never`, or no registry digest (imported onto the node with `ctr import`
/// or from k3s's `agent/images`). kube-system is left out: k3s brings its own.
pub fn node_only_images(pods: &[Value]) -> Vec<NodeImage> {
    let mut found: BTreeMap<String, NodeImage> = BTreeMap::new();
    for pod in pods {
        let (ns, name) = ns_name(pod);
        if SYSTEM_SECRET_NAMESPACES.contains(&ns.as_str()) {
            continue;
        }
        let owner = pod
            .pointer("/metadata/ownerReferences/0/name")
            .and_then(Value::as_str)
            .map(|o| o.to_string())
            .unwrap_or(name);
        let by = format!("{ns}/{owner}");
        let statuses: Vec<&Value> = ["/status/containerStatuses", "/status/initContainerStatuses"]
            .iter()
            .filter_map(|p| pod.pointer(p).and_then(Value::as_array))
            .flatten()
            .collect();
        for c in ["/spec/containers", "/spec/initContainers"].iter().filter_map(|p| pod.pointer(p).and_then(Value::as_array)).flatten() {
            let Some(image) = c["image"].as_str() else { continue };
            let status = statuses.iter().find(|st| st["name"] == c["name"]);
            let image_id = status.and_then(|st| st["imageID"].as_str()).unwrap_or_default();
            let reason = if c["imagePullPolicy"].as_str() == Some("Never") {
                "imagePullPolicy: Never"
            } else if !image_id.is_empty() && !image_id.contains('@') {
                "imported onto the node (no registry digest)"
            } else {
                continue;
            };
            let full = full_image_ref(image);
            let e = found.entry(full.clone()).or_insert_with(|| NodeImage { image: full, image_id: image_id.to_string(), used_by: Vec::new(), reason });
            if e.image_id.is_empty() {
                e.image_id = image_id.to_string();
            }
            if !e.used_by.contains(&by) {
                e.used_by.push(by.clone());
            }
        }
    }
    found.into_values().collect()
}

/// File for an exported image: its name plus a short id, so a rebuilt image
/// with the same tag is exported again.
pub fn image_file_name(image: &str, image_id: &str) -> String {
    let id: String = image_id.trim_start_matches("sha256:").chars().take(12).collect();
    let base = crate::archive::safe_segment(&image.replace(['/', ':', '@'], "_"));
    let base: String = base.chars().take(150).collect();
    if id.is_empty() { format!("{base}.tar") } else { format!("{base}-{id}.tar") }
}

/// `images.md`: every image the cluster runs, and which ones only the node had.
pub fn images_md(in_use: &BTreeMap<String, Vec<String>>, node_only: &[NodeImage], exported: &[ImageFile], export_note: Option<&str>) -> String {
    let mut out = String::from("# Images\n\n");
    if node_only.is_empty() {
        out.push_str("Every image comes from a registry. A rebuilt cluster pulls them again, as long as the registries still have these tags.\n\n");
    } else {
        out.push_str("## Only on the node\n\nNo registry is known to have these, so a new cluster can't pull them.\n\n");
        for i in node_only {
            let file = exported.iter().find(|f| f.image == i.image).map(|f| format!(" Saved as `images/{}`.", f.file)).unwrap_or_else(|| " Not saved.".into());
            out.push_str(&format!("- `{}` ({}), used by {}.{file}\n", i.image, i.reason, i.used_by.join(", ")));
        }
        if let Some(note) = export_note {
            out.push_str(&format!("\n{note}\n"));
        }
        out.push('\n');
    }
    out.push_str("## In use\n\n");
    for (image, by) in in_use {
        out.push_str(&format!("- `{image}`: {}\n", by.join(", ")));
    }
    out
}

/// Every image the pods run, with who runs it.
pub fn images_in_use(pods: &[Value]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pod in pods {
        let (ns, name) = ns_name(pod);
        let owner = pod.pointer("/metadata/ownerReferences/0/name").and_then(Value::as_str).map(str::to_string).unwrap_or(name);
        let by = format!("{ns}/{owner}");
        for c in ["/spec/containers", "/spec/initContainers"].iter().filter_map(|p| pod.pointer(p).and_then(Value::as_array)).flatten() {
            if let Some(image) = c["image"].as_str() {
                let e = out.entry(image.to_string()).or_default();
                if !e.contains(&by) {
                    e.push(by.clone());
                }
            }
        }
    }
    out
}

/// Exports one image from the node's containerd to stdout (run with sudo).
pub fn image_export_command(image: &str, arch: &str) -> Result<String, String> {
    if !plain_image_ref(image) {
        return Err(format!("{image} isn't a plain image reference"));
    }
    let platform = match arch {
        "x86_64" | "amd64" => "linux/amd64",
        "aarch64" | "arm64" => "linux/arm64",
        a if a.starts_with("armv7") || a == "arm" => "linux/arm/v7",
        _ => return Ok(format!("sudo k3s ctr -n k8s.io images export - {image}")),
    };
    Ok(format!("sudo k3s ctr -n k8s.io images export --platform {platform} - {image}"))
}

// --- the k3s node ---------------------------------------------------------------------

/// Quote for a POSIX shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Run `script` as root: `sudo sh -c '<script>'`.
pub fn sudo_script(script: &str) -> String {
    format!("sudo sh -c {}", sh_quote(script))
}

/// What the node is: `key=value` lines, parsed by [`parse_node_info`]. Run as root.
pub const NODE_INFO_SCRIPT: &str = r#"d=/var/lib/rancher/k3s
echo "version=$(k3s --version 2>/dev/null | head -n 1)"
echo "hostname=$(hostname 2>/dev/null)"
if [ -d $d/server/db/etcd ]; then echo "datastore=etcd"; elif [ -f $d/server/db/state.db ]; then echo "datastore=sqlite"; else echo "datastore="; fi
echo "arch=$(uname -m)"
echo "active=$(systemctl is-active k3s 2>/dev/null || true)"
for f in $d/agent/images/*; do [ -f "$f" ] && echo "agent_image=$(basename "$f")"; done
exit 0
"#;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeInfo {
    pub k3s_version: Option<String>,
    pub hostname: String,
    pub datastore: Option<String>,
    pub arch: String,
    pub running: bool,
    pub agent_images: Vec<String>,
}

pub fn parse_node_info(out: &str) -> NodeInfo {
    let all = |key: &str| -> Vec<String> {
        out.lines()
            .filter_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .collect()
    };
    let one = |key: &str| all(key).into_iter().next();
    NodeInfo {
        k3s_version: one("version"),
        hostname: one("hostname").unwrap_or_default(),
        datastore: one("datastore"),
        arch: one("arch").unwrap_or_default(),
        running: one("active").as_deref() == Some("active"),
        agent_images: all("agent_image"),
    }
}

/// Paths on the node a backup reads (relative to `/`).
pub const NODE_PATHS: &[&str] = &[
    "etc/rancher/k3s/config.yaml",
    "etc/rancher/k3s/config.yaml.d",
    "etc/rancher/k3s/registries.yaml",
    "etc/rancher/k3s/k3s.env",
    "var/lib/rancher/k3s/server/manifests",
    "var/lib/rancher/k3s/server/token",
    "etc/systemd/system/k3s.service",
    "etc/systemd/system/k3s.service.env",
    "etc/init.d/k3s",
];

/// Prints a `.tar.gz` of whichever [`NODE_PATHS`] exist (nothing if none). Run as root.
pub fn node_files_script() -> String {
    format!(
        "cd / || exit 1\nset --\nfor p in {}; do [ -e \"$p\" ] && set -- \"$@\" \"$p\"; done\n[ $# -gt 0 ] || exit 0\nexec tar -czf - \"$@\"\n",
        NODE_PATHS.join(" ")
    )
}

/// Manifests k3s writes into `server/manifests` itself (rewritten at every start).
const K3S_DEFAULT_MANIFESTS: &[&str] = &["ccm.yaml", "coredns.yaml", "local-storage.yaml", "rolebindings.yaml", "runtimes.yaml", "traefik.yaml"];
const MANIFESTS_DIR: &str = "var/lib/rancher/k3s/server/manifests/";

/// Worth keeping from the node: everything read except k3s's own default
/// manifests. `.skip` files (a default turned off) are kept.
pub fn keep_node_file(rel: &str) -> bool {
    match rel.strip_prefix(MANIFESTS_DIR) {
        Some(m) => !(K3S_DEFAULT_MANIFESTS.contains(&m) || m.starts_with("metrics-server/")),
        None => true,
    }
}

/// Holds a credential, so it's sealed (or left out without a passphrase):
/// the join token, registry logins, and any file mentioning a token,
/// password or secret.
pub fn node_file_sensitive(rel: &str, content: &[u8]) -> bool {
    if rel.ends_with("/token") || rel.ends_with("registries.yaml") {
        return true;
    }
    let text = String::from_utf8_lossy(content).to_ascii_lowercase();
    ["token", "password", "secret", "auth:"].iter().any(|w| text.contains(w))
}

/// The k3s service's start command from its unit file, joined onto one line.
pub fn exec_start(unit: &str) -> Option<String> {
    let mut lines = unit.lines().skip_while(|l| !l.trim_start().starts_with("ExecStart="));
    let mut line = lines.next()?.trim_start().strip_prefix("ExecStart=")?.to_string();
    let mut parts = Vec::new();
    loop {
        let trimmed = line.trim();
        let (body, more) = match trimmed.strip_suffix('\\') {
            Some(b) => (b.trim(), true),
            None => (trimmed, false),
        };
        parts.extend(body.split_whitespace().map(|w| w.trim_matches('\'').to_string()).filter(|w| !w.is_empty()));
        match (more, lines.next()) {
            (true, Some(next)) => line = next.to_string(),
            _ => break,
        }
    }
    Some(parts.join(" "))
}

/// Prints a `.tar.gz` of a consistent copy of the datastore plus the server
/// token (needed to restore it). Etcd: `k3s etcd-snapshot save`. SQLite:
/// `sqlite3 .backup` when sqlite3 is installed, else the database files as
/// they are (k3s keeps writing; SQLite usually recovers such a copy). Run as root.
pub const DATASTORE_SCRIPT: &str = r#"d=/var/lib/rancher/k3s/server
t=$(mktemp -d) || exit 1
trap 'rm -rf "$t"' EXIT
if [ -d $d/db/etcd ]; then
  k3s etcd-snapshot save --name portside --dir "$t" >&2 || exit 1
elif [ -f $d/db/state.db ]; then
  if command -v sqlite3 >/dev/null 2>&1; then
    sqlite3 $d/db/state.db ".backup '$t/state.db'" >&2 || exit 1
  else
    for f in state.db state.db-wal state.db-shm; do if [ -f $d/db/$f ]; then cp $d/db/$f "$t/" || exit 1; fi; done
  fi
else
  echo "No k3s datastore under $d/db (a custom --data-dir or an external database?)" >&2
  exit 1
fi
cp $d/token "$t/token" 2>/dev/null
tar -czf - -C "$t" .
"#;

/// Where a node file from a backup goes back on a node, for the files a
/// rebuild restores: the k3s config, registries and custom manifests. Never
/// the token or the service unit (the installer writes those).
pub fn node_restore_dest(rel: &str) -> Option<String> {
    let path = rel.strip_prefix(NODE_DIR)?.strip_prefix('/')?;
    let allowed = path == "etc/rancher/k3s/config.yaml"
        || path == "etc/rancher/k3s/registries.yaml"
        || path.starts_with("etc/rancher/k3s/config.yaml.d/")
        || path.starts_with(MANIFESTS_DIR);
    let safe = path
        .split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != ".." && seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')));
    (allowed && safe && keep_node_file(path)).then(|| format!("/{path}"))
}

// --- volume backups and rebuilds ---------------------------------------------------------

/// The volume backups among the objects (CronJobs Portside set up), from their saved spec.
pub fn volume_backup_refs(objects: &[Value]) -> Vec<VolumeBackupRef> {
    let mut out: Vec<VolumeBackupRef> = objects
        .iter()
        .filter(|o| s(o, "/kind") == Some("CronJob"))
        .filter(|o| o.pointer("/metadata/labels").and_then(|l| l.get(BACKUP_LABEL)).is_some())
        .filter_map(|o| {
            let raw = o.pointer("/metadata/annotations").and_then(|a| a.get(SPEC_ANNOTATION)).and_then(Value::as_str)?;
            let spec: BackupSpec = serde_json::from_str(raw).ok()?;
            Some(VolumeBackupRef { namespace: spec.namespace, name: spec.name, claim: spec.claim })
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The three passes of a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RestorePhase {
    /// CRDs, which must be established before their objects can be sent.
    Definitions,
    /// Namespaces, accounts, config, Secrets, claims, Services, custom resources…
    Objects,
    /// What runs pods. Sent after the volume data is back, so nothing has the claims mounted while they're written.
    Workloads,
}

pub fn restore_phase(kind: &str) -> RestorePhase {
    match kind {
        "CustomResourceDefinition" => RestorePhase::Definitions,
        "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" | "CronJob" | "Pod" => RestorePhase::Workloads,
        _ => RestorePhase::Objects,
    }
}

/// HelmCharts (k3s's Helm controller) among the objects, by name: the
/// controller installs each as a Helm release of that name.
pub fn helm_chart_names(objects: &[Value]) -> HashSet<String> {
    objects.iter().filter(|o| s(o, "/kind") == Some("HelmChart")).filter_map(|o| s(o, "/metadata/name")).map(str::to_string).collect()
}

/// Why a rebuild leaves an object to k3s rather than sending it: k3s makes
/// it from its own manifests (CoreDNS, local-path, …: sending the old one
/// could downgrade it), or the k3s Helm controller installs it from a
/// HelmChart in the backup (Traefik and its settings come back through the
/// HelmChart and HelmChartConfig).
pub fn rebuild_skip(obj: &Value, helm_charts: &HashSet<String>) -> Option<String> {
    let labels = obj.pointer("/metadata/labels");
    if labels.and_then(|l| l.get("objectset.rio.cattle.io/hash")).is_some() {
        return Some("k3s makes it from its own manifests".into());
    }
    let release = obj
        .pointer("/metadata/annotations")
        .and_then(|a| a.get("meta.helm.sh/release-name"))
        .and_then(Value::as_str)
        .or_else(|| (s(obj, "/type") == Some(HELM_RELEASE_TYPE)).then(|| labels.and_then(|l| l.get("name")).and_then(Value::as_str)).flatten());
    match release {
        Some(r) if helm_charts.contains(r) => Some(format!("the k3s Helm controller installs it from HelmChart {r}")),
        _ => None,
    }
}

/// A hostPath that holds an app's data (as opposed to the node's own files a
/// log shipper or monitoring agent reads). Volume backups can't copy these.
pub fn data_host_path(path: &str) -> bool {
    const SYSTEM: &[&str] = &[
        "/proc", "/sys", "/dev", "/run", "/var/run", "/var/log", "/etc", "/lib", "/usr", "/boot", "/bin", "/sbin", "/tmp",
        "/var/lib/kubelet", "/var/lib/rancher/k3s/agent", "/var/lib/rancher/k3s/data", "/var/lib/containerd", "/var/lib/docker",
        "/var/lib/cni", "/opt/cni", "/var/lib/calico", "/var/lib/cilium", "/var/lib/rancher/k3s/server",
    ];
    let p = path.trim_end_matches('/');
    !p.is_empty() && !SYSTEM.iter().any(|s| p == *s || p.starts_with(&format!("{s}/")))
}

/// README for a backup folder: what's there and how to put it back.
pub fn readme(index: &BackupIndex, when: &str) -> String {
    let mut r = format!("# Cluster backup of {}\n\nTaken {when} by Portside Lite. {} objects.\n\n## What's here\n\n", index.cluster, index.objects);
    r.push_str("- `cluster/`: cluster-wide settings (namespaces, storage classes, static volumes, cluster roles, CRDs and cluster-wide custom resources).\n");
    r.push_str("- `namespaces/<namespace>/<Kind>.yaml`: everything else, one file per kind (custom resources as `<Kind>.<group>.yaml`).\n");
    r.push_str("- `helm-releases.md`: what was installed with Helm. `images.md`: every image in use, and the ones only the node had.\n");
    if index.sealed {
        r.push_str(&format!(
            "- `{SEALED_FILE}`: sealed with your backup passphrase: the values of {} Secrets, Helm release records and values{}.\n",
            index.sealed_secrets,
            if index.node.as_ref().is_some_and(|n| !n.sealed_files.is_empty()) { ", and node files that hold credentials" } else { "" }
        ));
    } else {
        r.push_str("- `secrets-needed.md`: Secret values aren't saved (no backup passphrase is set); this lists the ones to recreate.\n");
    }
    if let Some(node) = &index.node {
        r.push_str(&format!(
            "- `node/`: the k3s server's own files from {} ({}): config, registries, custom manifests, the service's install flags.\n",
            if node.hostname.is_empty() { "the node" } else { &node.hostname },
            node.k3s_version.as_deref().unwrap_or("k3s")
        ));
        if !node.left_out.is_empty() {
            r.push_str(&format!("  Left out because they hold credentials and no passphrase is set: {}.\n", node.left_out.join(", ")));
        }
    }
    if let Some(ds) = &index.datastore {
        r.push_str(&format!("- `{DATASTORE_FILE}`: a snapshot of the k3s datastore ({ds}) and the server token, sealed.\n"));
    }
    if !index.images.is_empty() {
        r.push_str(&format!("- `../../{IMAGES_DIR}/`: {} images only the node had, exported (see images.md).\n", index.images.len()));
    }
    r.push_str("\nVolume data is in `../../volumes/<namespace>/<backup>/` when copying volume backups to this PC is on.\n");
    r.push_str(
        "\n## Rebuilding\n\nIn Portside: Backups → Rebuild a cluster. It puts the node files back, installs k3s if needed, \
         then sends the objects (CRDs first), copies each volume's newest archive into its claim, and starts the workloads last.\n\n\
         By hand: `kubectl apply -f cluster/` (CRDs first), then `namespaces/`. To open the sealed file: \
         `age -d secrets.tar.gz.age | tar xz` (https://age-encryption.org); it asks for the backup passphrase.\n",
    );
    if index.datastore.is_some() {
        r.push_str(
            "\n### The datastore snapshot\n\nAn exact copy of the cluster state, for when a rebuild from the manifests isn't enough. \
             It only restores onto a k3s server with the same token: `age -d datastore.tar.gz.age | tar xz` gives the snapshot and `token`.\n\
             - etcd: `k3s server --cluster-reset --cluster-reset-restore-path=<snapshot> --token=<token>`, then start k3s normally.\n\
             - SQLite: stop k3s, put `state.db` in `/var/lib/rancher/k3s/server/db/` (remove any `state.db-wal`/`-shm` there), \
             put the token in `/var/lib/rancher/k3s/server/token`, start k3s.\n",
        );
    }
    let notes: Vec<&String> = index.skipped.iter().chain(&index.warnings).collect();
    if !notes.is_empty() {
        r.push_str(&format!("\n## Left out this time\n\n{}\n", notes.iter().map(|s| format!("- {s}")).collect::<Vec<_>>().join("\n")));
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn helm_secret(ns: &str, release: &str, rev: u64, status: &str) -> Value {
        let b64 = base64::engine::general_purpose::STANDARD;
        let body = json!({
            "name": release, "namespace": ns, "version": rev, "info": { "status": status },
            "chart": { "metadata": { "name": "redis", "version": "19.0.1", "appVersion": "7.2", "home": "https://bitnami.com" } },
            "config": { "auth": { "password": "hunter2" } }
        });
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut gz, body.to_string().as_bytes()).unwrap();
        let helm_b64 = b64.encode(gz.finish().unwrap());
        json!({
            "metadata": { "namespace": ns, "name": format!("sh.helm.release.v1.{release}.v{rev}"),
                          "labels": { "owner": "helm", "name": release, "version": rev.to_string(), "status": status } },
            "type": HELM_RELEASE_TYPE,
            "data": { "release": b64.encode(helm_b64) }
        })
    }

    #[test]
    fn helm_releases_decode_and_only_the_newest_is_sealed() {
        let secrets = vec![helm_secret("cache", "redis", 1, "superseded"), helm_secret("cache", "redis", 2, "deployed"), helm_secret("web", "app", 10, "deployed")];
        let latest = latest_helm_secrets(&secrets);
        assert!(latest.contains(&("cache".into(), "sh.helm.release.v1.redis.v2".into())));
        assert!(!latest.contains(&("cache".into(), "sh.helm.release.v1.redis.v1".into())));
        assert!(seal_secret(&secrets[1], &latest) && !seal_secret(&secrets[0], &latest));
        let r = helm_release(&secrets[1]).unwrap();
        assert_eq!((r.name.as_str(), r.revision, r.chart.as_str(), r.chart_version.as_str()), ("redis", 2, "redis", "19.0.1"));
        assert_eq!(r.links, ["https://bitnami.com"]);
        assert!(r.values.unwrap().contains("hunter2"));
        let md = helm_releases_md(&[helm_release(&secrets[1]).unwrap()], true);
        assert!(md.contains("| cache | redis | redis | 19.0.1 |") && !md.contains("hunter2"), "values never in the plain file");
    }

    #[test]
    fn which_secrets_are_sealed() {
        let sec = |ns: &str, ty: &str, owned: bool| {
            let mut v = json!({ "metadata": { "namespace": ns, "name": "x" }, "type": ty });
            if owned {
                v["metadata"]["ownerReferences"] = json!([{ "controller": true, "kind": "Certificate", "name": "c" }]);
            }
            v
        };
        let none = HashSet::new();
        assert!(seal_secret(&sec("app", "Opaque", false), &none));
        assert!(seal_secret(&sec("app", "kubernetes.io/tls", false), &none));
        assert!(!seal_secret(&sec("app", "kubernetes.io/service-account-token", false), &none));
        assert!(!seal_secret(&sec("kube-system", "Opaque", false), &none), "the cluster's own");
        assert!(!seal_secret(&sec("app", "Opaque", true), &none), "its controller makes it again");
    }

    #[test]
    fn node_only_images_are_found_and_named() {
        let pod = |ns: &str, image: &str, id: &str, policy: &str| {
            json!({
                "metadata": { "namespace": ns, "name": "web-abc", "ownerReferences": [{ "name": "web-5d9" }] },
                "spec": { "containers": [{ "name": "c", "image": image, "imagePullPolicy": policy }] },
                "status": { "containerStatuses": [{ "name": "c", "imageID": id }] }
            })
        };
        let pods = vec![
            pod("app", "myapp:dev", "sha256:0123456789abcdef", "IfNotPresent"),
            pod("app", "nginx:1.27", "docker.io/library/nginx@sha256:aa", "IfNotPresent"),
            pod("app", "registry.local/tool", "registry.local/tool@sha256:bb", "Never"),
            pod("kube-system", "rancher/klipper-lb:v0.4", "sha256:cc", "IfNotPresent"),
        ];
        let found = node_only_images(&pods);
        let names: Vec<&str> = found.iter().map(|i| i.image.as_str()).collect();
        assert_eq!(names, ["docker.io/library/myapp:dev", "registry.local/tool:latest"]);
        assert_eq!(found[0].used_by, ["app/web-5d9"]);
        assert_eq!(image_file_name(&found[0].image, &found[0].image_id), "docker.io_library_myapp_dev-0123456789ab.tar");
        assert_eq!(images_in_use(&pods).len(), 4);
        assert_eq!(full_image_ref("ghcr.io/o/app@sha256:ff"), "ghcr.io/o/app@sha256:ff");
        assert_eq!(full_image_ref("localhost:5000/app"), "localhost:5000/app:latest");
        assert!(image_export_command("docker.io/library/a:b", "x86_64").unwrap().contains("--platform linux/amd64 - docker.io/library/a:b"));
        assert!(image_export_command("a; rm -rf /", "x86_64").is_err());
    }

    #[test]
    fn crds_extra_kinds_and_endpoints() {
        let crd = json!({ "spec": { "group": "traefik.io", "scope": "Namespaced", "names": { "kind": "Middleware", "plural": "middlewares" },
            "versions": [{ "name": "v1alpha1", "served": true, "storage": true }, { "name": "v1beta1", "served": false, "storage": false }] } });
        let r = crd_resource(&crd).unwrap();
        assert_eq!((r.version.as_str(), r.namespaced, r.file_name()), ("v1alpha1", true, "Middleware.traefik.io.yaml".to_string()));
        assert!(crd_resource(&json!({ "spec": { "group": "helm.cattle.io" } })).is_none(), "k3s's own");

        assert!(keep_persistent_volume(&json!({ "metadata": { "name": "nfs" } })));
        assert!(!keep_persistent_volume(&json!({ "metadata": { "annotations": { "pv.kubernetes.io/provisioned-by": "rancher.io/local-path" } } })));

        let svcs = vec![
            json!({ "metadata": { "namespace": "a", "name": "db" }, "spec": { "type": "ClusterIP" } }),
            json!({ "metadata": { "namespace": "a", "name": "web" }, "spec": { "selector": { "app": "web" } } }),
        ];
        let sl = selectorless_services(&svcs);
        assert!(keep_endpoints(&json!({ "metadata": { "namespace": "a", "name": "db" } }), &sl));
        assert!(!keep_endpoints(&json!({ "metadata": { "namespace": "a", "name": "web" } }), &sl));
        let slice = |by: &str| json!({ "metadata": { "labels": { "endpointslice.kubernetes.io/managed-by": by } } });
        assert!(!keep_endpoint_slice(&slice("endpointslice-controller.k8s.io")));
        assert!(keep_endpoint_slice(&slice("me")));
    }

    #[test]
    fn node_files() {
        assert!(!keep_node_file("var/lib/rancher/k3s/server/manifests/traefik.yaml"));
        assert!(!keep_node_file("var/lib/rancher/k3s/server/manifests/metrics-server/metrics-apiservice.yaml"));
        assert!(keep_node_file("var/lib/rancher/k3s/server/manifests/traefik.yaml.skip"));
        assert!(keep_node_file("var/lib/rancher/k3s/server/manifests/my-app.yaml"));
        assert!(node_file_sensitive("var/lib/rancher/k3s/server/token", b"K10..."));
        assert!(node_file_sensitive("etc/rancher/k3s/config.yaml", b"token: abc\n"));
        assert!(!node_file_sensitive("etc/rancher/k3s/config.yaml", b"disable:\n  - traefik\n"));
        assert!(node_files_script().contains("etc/rancher/k3s/registries.yaml"));

        assert_eq!(node_restore_dest("node/etc/rancher/k3s/config.yaml").as_deref(), Some("/etc/rancher/k3s/config.yaml"));
        assert_eq!(node_restore_dest("node/var/lib/rancher/k3s/server/manifests/app.yaml").as_deref(), Some("/var/lib/rancher/k3s/server/manifests/app.yaml"));
        assert_eq!(node_restore_dest("node/var/lib/rancher/k3s/server/token"), None, "the installer makes a new one");
        assert_eq!(node_restore_dest("node/etc/systemd/system/k3s.service"), None);
        assert_eq!(node_restore_dest("node/var/lib/rancher/k3s/server/manifests/../../token"), None);
        assert_eq!(node_restore_dest("node/var/lib/rancher/k3s/server/manifests/a b.yaml"), None);

        let unit = "[Service]\nType=notify\nExecStart=/usr/local/bin/k3s \\\n    server \\\n\t'--disable' \\\n\t'traefik' \\\n\nRestart=always\n";
        assert_eq!(exec_start(unit).as_deref(), Some("/usr/local/bin/k3s server --disable traefik"));

        let info = parse_node_info("version=k3s version v1.31.4+k3s1 (a)\nhostname=box\ndatastore=sqlite\narch=x86_64\nactive=active\nagent_image=a.tar\nagent_image=b.tar.zst\n");
        assert_eq!((info.datastore.as_deref(), info.running, info.agent_images.len()), (Some("sqlite"), true, 2));
        assert_eq!(sudo_script("echo 'hi'"), r#"sudo sh -c 'echo '\''hi'\'''"#);
    }

    #[test]
    fn rebuild_phases_and_volume_refs() {
        assert_eq!(restore_phase("CustomResourceDefinition"), RestorePhase::Definitions);
        assert_eq!(restore_phase("Secret"), RestorePhase::Objects);
        assert_eq!(restore_phase("Middleware"), RestorePhase::Objects);
        assert_eq!(restore_phase("StatefulSet"), RestorePhase::Workloads);
        let spec = crate::backup::sample_spec();
        let cron = json!({ "kind": "CronJob", "metadata": { "labels": { BACKUP_LABEL: spec.name }, "annotations": { SPEC_ANNOTATION: serde_json::to_string(&spec).unwrap() } } });
        assert_eq!(volume_backup_refs(&[cron, json!({ "kind": "CronJob", "metadata": {} })]), [VolumeBackupRef { namespace: spec.namespace, name: spec.name, claim: spec.claim }]);
        assert!(data_host_path("/srv/app-data") && data_host_path("/var/lib/rancher/k3s/storage/x"));

        let objs = vec![json!({ "kind": "HelmChart", "metadata": { "name": "traefik" } })];
        let charts = helm_chart_names(&objs);
        assert!(rebuild_skip(&json!({ "metadata": { "labels": { "objectset.rio.cattle.io/hash": "x" } } }), &charts).is_some());
        assert!(rebuild_skip(&json!({ "metadata": { "annotations": { "meta.helm.sh/release-name": "traefik" } } }), &charts).is_some());
        assert!(rebuild_skip(&json!({ "type": HELM_RELEASE_TYPE, "metadata": { "labels": { "name": "traefik" } } }), &charts).is_some());
        assert!(rebuild_skip(&json!({ "metadata": { "annotations": { "meta.helm.sh/release-name": "redis" } } }), &charts).is_none(), "a user's own release");
        assert!(!data_host_path("/var/log") && !data_host_path("/proc/1") && !data_host_path("/"));
    }
}
