//! Browsing a PersistentVolumeClaim's files through a short-lived helper pod
//! that mounts the claim at [`MOUNT`]. Pure parts only: path rules, the
//! helper pod spec, the shell scripts run in it, output parsing, and who is
//! using a claim (which decides whether writes are allowed).
//!
//! Writes are allowed only while nothing else can touch the volume: no pod
//! (other than our helpers) mounts it, and no workload that mounts it is
//! scaled above zero, so an app never sees files change under it.

use k8s_openapi::api::apps::v1::StatefulSet;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, PodSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};

use crate::summarize::ClusterObjects;

/// Where the claim is mounted inside the helper pod.
pub const MOUNT: &str = "/data";
/// Label on every helper pod (value "true").
pub const HELPER_LABEL: &str = "portside-lite/files";
/// Annotation on a helper pod naming the claim it mounts.
pub const CLAIM_ANNOTATION: &str = "portside-lite/claim";
/// Annotation naming the app install that started a helper, so a later run
/// can clear up what an earlier one (crashed or killed) left behind.
pub const OWNER_ANNOTATION: &str = "portside-lite/owner";
/// A helper exits on its own after this long, so a crash can't leave the
/// volume attached forever.
pub const HELPER_LIFETIME_SECS: u64 = 2 * 60 * 60;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    /// file | dir | link | other
    pub kind: String,
    pub size: u64,
    pub modified_ms: Option<i64>,
    /// Symbolic link that points at a directory (can be opened).
    pub link_to_dir: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListing {
    /// Relative to the volume root; "" is the root.
    pub path: String,
    pub entries: Vec<FileEntry>,
    pub total_bytes: Option<u64>,
    pub free_bytes: Option<u64>,
}

/// Who is using a claim right now.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClaimUsers {
    /// Non-finished pods mounting it (helpers excluded), with their node.
    pub pods: Vec<(String, Option<String>)>,
    /// Workloads whose pod template mounts it, e.g. "Deployment/web".
    pub workloads: Vec<String>,
    /// Why writing now would be unsafe; empty = writes allowed.
    pub blockers: Vec<String>,
}

// --- paths ---------------------------------------------------------------------

/// Clean a path relative to the volume root: no `..`/`.` parts, no NUL,
/// surrounding and doubled slashes dropped. "" is the root. Only `/`
/// separates: a backslash is an ordinary character in a Linux file name.
pub fn clean_rel(path: &str) -> Result<String, String> {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" => {}
            "." | ".." => return Err(format!("\"{path}\" isn't allowed: paths can't contain . or ..")),
            p if p.contains('\0') => return Err("Paths can't contain NUL characters.".into()),
            p => parts.push(p),
        }
    }
    Ok(parts.join("/"))
}

/// Absolute path inside the helper for a (cleaned) relative path.
pub fn abs(rel: &str) -> String {
    if rel.is_empty() { MOUNT.to_string() } else { format!("{MOUNT}/{rel}") }
}

/// Join a relative directory and a single name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// A single file or folder name (upload, new folder, rename).
pub fn valid_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name == "." || name == ".." {
        return Err("Enter a name.".into());
    }
    if name.contains(['/', '\\', '\0']) {
        return Err(format!("\"{name}\" can't contain / or \\."));
    }
    if name.len() > 255 {
        return Err("Names can be at most 255 bytes.".into());
    }
    Ok(())
}

// --- scripts -----------------------------------------------------------------------
// Each runs as `sh -c <script> sh <args…>`: arguments arrive as positional
// parameters, so nothing user-supplied is ever spliced into shell text.
// Written for busybox (the default helper image).

/// `$1` = directory. One stat call for every entry, then which symlinks point
/// at directories, then the filesystem's size.
const LIST: &str = r#"cd -- "$1" || exit 1
shift
for f in .* *; do
  case "$f" in .|..) continue;; esac
  if [ -e "$f" ] || [ -L "$f" ]; then set -- "$@" "$f"; fi
done
[ $# -gt 0 ] && stat -c 'F|%s|%Y|%F|%n' -- "$@"
for f; do if [ -L "$f" ] && [ -d "$f" ]; then printf 'L|%s\n' "$f"; fi; done
printf '##df\n'
df -kP . | tail -n 1
exit 0"#;

/// `$1` = file.
const CAT: &str = r#"[ -f "$1" ] || { echo "Not a file: $1" >&2; exit 1; }
exec cat -- "$1""#;

/// `$1` = parent, `$2` = folder name: gzip'd tar of the folder on stdout.
const TAR: &str = r#"cd -- "$1" || exit 1
exec tar czf - "./$2""#;

/// `$1` = destination file, `$2` = exact byte count on stdin. Writes to a
/// temp file, checks the size, keeps an existing file's owner and mode (or
/// takes the folder's owner for a new one), then renames into place.
const PUT: &str = r#"dst="$1"; size="$2"; dir="$(dirname -- "$dst")"
[ -d "$dst" ] && { echo "A folder named $(basename -- "$dst") already exists there." >&2; exit 1; }
tmp="$dir/.portside-upload.$$"
trap 'rm -f -- "$tmp"' EXIT
head -c "$size" > "$tmp" || exit 1
got="$(stat -c %s -- "$tmp")"
[ "$got" = "$size" ] || { echo "Upload was cut short ($got of $size bytes)." >&2; exit 1; }
if [ -e "$dst" ]; then
  chown "$(stat -c %u:%g -- "$dst")" "$tmp" 2>/dev/null
  chmod "$(stat -c %a -- "$dst")" "$tmp" 2>/dev/null
else
  chown "$(stat -c %u:%g -- "$dir")" "$tmp" 2>/dev/null
  chmod 644 "$tmp" 2>/dev/null
fi
mv -f -- "$tmp" "$dst" || exit 1
trap - EXIT"#;

/// `$1` = base folder, then folders relative to it (parents first). New
/// folders take the base folder's owner.
const MKDIRS: &str = r#"base="$1"; shift
[ -d "$base" ] || { echo "Not a folder: $base" >&2; exit 1; }
owner="$(stat -c %u:%g -- "$base")"
for d; do
  [ -d "$base/$d" ] && continue
  [ -e "$base/$d" ] && { echo "A file named $d is in the way." >&2; exit 1; }
  mkdir -- "$base/$d" || exit 1
  chown "$owner" "$base/$d" 2>/dev/null
done
exit 0"#;

/// `$1` = path.
const REMOVE: &str = r#"[ -e "$1" ] || [ -L "$1" ] || { echo "It's already gone." >&2; exit 1; }
rm -rf -- "$1""#;

/// `$1` = from, `$2` = to (same folder).
const RENAME: &str = r#"[ -e "$2" ] || [ -L "$2" ] && { echo "Something named $(basename -- "$2") already exists there." >&2; exit 1; }
mv -- "$1" "$2""#;

fn script(body: &str, args: &[&str]) -> Vec<String> {
    let mut v = vec!["sh".to_string(), "-c".to_string(), body.to_string(), "sh".to_string()];
    v.extend(args.iter().map(|a| a.to_string()));
    v
}

pub fn list_command(rel: &str) -> Vec<String> {
    script(LIST, &[&abs(rel)])
}

pub fn cat_command(rel: &str) -> Vec<String> {
    script(CAT, &[&abs(rel)])
}

/// Archive a folder (relative path, may be the root).
pub fn tar_command(rel: &str) -> Vec<String> {
    let full = abs(rel);
    let (parent, name) = full.rsplit_once('/').expect("absolute path");
    script(TAR, &[if parent.is_empty() { "/" } else { parent }, name])
}

pub fn put_command(rel: &str, size: u64) -> Vec<String> {
    script(PUT, &[&abs(rel), &size.to_string()])
}

pub fn mkdirs_command(base_rel: &str, dirs: &[String]) -> Vec<String> {
    let mut args = vec![abs(base_rel)];
    args.extend(dirs.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    script(MKDIRS, &refs)
}

pub fn remove_command(rel: &str) -> Vec<String> {
    script(REMOVE, &[&abs(rel)])
}

pub fn rename_command(from_rel: &str, to_rel: &str) -> Vec<String> {
    script(RENAME, &[&abs(from_rel), &abs(to_rel)])
}

// --- parsing ---------------------------------------------------------------------

fn kind_of(stat_type: &str) -> &'static str {
    match stat_type {
        "regular file" | "regular empty file" => "file",
        "directory" => "dir",
        "symbolic link" => "link",
        _ => "other",
    }
}

/// Parse the output of [`list_command`]. A name with a newline in it spills
/// onto a line of its own; that entry is dropped (half a name would point at
/// nothing, or at another file). Folders first, then by name.
pub fn parse_listing(path: &str, out: &str) -> FileListing {
    // The marker is its own line and comes last; a file name may contain the same text.
    let (body, df) = match out.rfind("\n##df\n") {
        Some(i) => (&out[..i + 1], &out[i + 6..]),
        None => out.strip_prefix("##df\n").map_or((out, ""), |df| ("", df)),
    };
    let mut entries = Vec::new();
    let mut dir_links = std::collections::HashSet::new();
    let mut after_entry = false;
    for line in body.lines() {
        if let Some(name) = line.strip_prefix("L|") {
            dir_links.insert(name.to_string());
            after_entry = false;
            continue;
        }
        let Some(rest) = line.strip_prefix("F|") else {
            if std::mem::take(&mut after_entry) {
                entries.pop();
            }
            continue;
        };
        after_entry = false;
        let mut it = rest.splitn(4, '|');
        let (Some(size), Some(mtime), Some(t), Some(name)) = (it.next(), it.next(), it.next(), it.next()) else { continue };
        let Ok(size) = size.parse::<u64>() else { continue };
        after_entry = true;
        entries.push(FileEntry {
            name: name.to_string(),
            kind: kind_of(t).to_string(),
            size,
            modified_ms: mtime.parse::<i64>().ok().map(|s| s * 1000),
            link_to_dir: false,
        });
    }
    for e in &mut entries {
        e.link_to_dir = e.kind == "link" && dir_links.contains(&e.name);
    }
    entries.sort_by(|a, b| {
        let dir = |e: &FileEntry| !(e.kind == "dir" || e.link_to_dir);
        dir(a).cmp(&dir(b)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    let (total_bytes, free_bytes) = parse_df(df);
    FileListing { path: path.to_string(), entries, total_bytes, free_bytes }
}

/// `df -kP` data line: filesystem, 1K-blocks, used, available, capacity, mount.
fn parse_df(line: &str) -> (Option<u64>, Option<u64>) {
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 4 {
        return (None, None);
    }
    let kib = |s: &str| s.parse::<u64>().ok().map(|k| k * 1024);
    (kib(cols[1]), kib(cols[3]))
}

// --- helper pod --------------------------------------------------------------------

/// The helper pod. `node` pins it next to a pod that already has the volume
/// attached (needed for ReadWriteOnce on most storage); `read_only` mounts
/// the claim read-only.
pub fn helper_pod(namespace: &str, claim: &str, image: &str, node: Option<&str>, read_only: bool, owner: &str) -> Pod {
    let mut spec = serde_json::json!({
        "restartPolicy": "Never",
        "activeDeadlineSeconds": HELPER_LIFETIME_SECS + 60,
        "terminationGracePeriodSeconds": 0,
        "automountServiceAccountToken": false,
        "enableServiceLinks": false,
        // The volume may live on a tainted node; go wherever it is.
        "tolerations": [{ "operator": "Exists" }],
        "containers": [{
            "name": "files",
            "image": image,
            "command": ["sh", "-c", format!("trap 'exit 0' TERM; sleep {HELPER_LIFETIME_SECS} & wait")],
            "volumeMounts": [{ "name": "data", "mountPath": MOUNT, "readOnly": read_only }],
            "resources": {
                "requests": { "cpu": "10m", "memory": "16Mi" },
                "limits": { "memory": "256Mi" }
            }
        }],
        "volumes": [{ "name": "data", "persistentVolumeClaim": { "claimName": claim, "readOnly": read_only } }]
    });
    if let Some(n) = node {
        spec["nodeName"] = serde_json::json!(n);
    }
    serde_json::from_value(serde_json::json!({
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {
            "generateName": "portside-files-",
            "namespace": namespace,
            "labels": { HELPER_LABEL: "true", "app.kubernetes.io/managed-by": "portside-lite" },
            "annotations": { CLAIM_ANNOTATION: claim, OWNER_ANNOTATION: owner }
        },
        "spec": spec
    }))
    .expect("helper pod spec is valid")
}

pub fn is_helper(p: &Pod) -> bool {
    p.metadata.labels.as_ref().and_then(|l| l.get(HELPER_LABEL)).map(String::as_str) == Some("true")
}

pub fn helper_owner(p: &Pod) -> Option<&str> {
    p.metadata.annotations.as_ref()?.get(OWNER_ANNOTATION).map(String::as_str)
}

pub fn helper_claim(p: &Pod) -> Option<&str> {
    p.metadata.annotations.as_ref()?.get(CLAIM_ANNOTATION).map(String::as_str)
}

// --- who uses a claim -------------------------------------------------------------

fn spec_mounts(spec: Option<&PodSpec>, claim: &str) -> bool {
    spec.and_then(|s| s.volumes.as_ref())
        .is_some_and(|vs| vs.iter().any(|v| v.persistent_volume_claim.as_ref().is_some_and(|c| c.claim_name == claim)))
}

/// StatefulSet volumeClaimTemplates create claims named `<template>-<sts>-<ordinal>`.
fn sts_owns(s: &StatefulSet, claim: &str) -> bool {
    let sts = s.metadata.name.as_deref().unwrap_or_default();
    s.spec.as_ref().and_then(|sp| sp.volume_claim_templates.as_ref()).is_some_and(|ts| {
        ts.iter().any(|t| {
            let prefix = format!("{}-{sts}-", t.metadata.name.as_deref().unwrap_or_default());
            claim
                .strip_prefix(&prefix)
                .is_some_and(|ord| !ord.is_empty() && ord.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

fn job_finished(j: &Job) -> bool {
    j.status
        .as_ref()
        .and_then(|s| s.conditions.as_ref())
        .is_some_and(|cs| cs.iter().any(|c| (c.type_ == "Complete" || c.type_ == "Failed") && c.status == "True"))
}

fn plural(n: i32, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Pods and workloads in `namespace` that use `claim`, and what would make
/// writing unsafe right now.
pub fn claim_users(namespace: &str, claim: &str, objs: &ClusterObjects) -> ClaimUsers {
    let here = |m: &ObjectMeta| m.namespace.as_deref() == Some(namespace);
    let mut u = ClaimUsers::default();
    for p in objs.pods.iter().filter(|o| here(&o.metadata)) {
        let phase = p.status.as_ref().and_then(|s| s.phase.as_deref()).unwrap_or("Pending");
        if is_helper(p) || phase == "Succeeded" || phase == "Failed" || !spec_mounts(p.spec.as_ref(), claim) {
            continue;
        }
        let name = p.metadata.name.clone().unwrap_or_default();
        u.blockers.push(format!("Pod {name} has it mounted ({phase})."));
        u.pods.push((name, p.spec.as_ref().and_then(|s| s.node_name.clone())));
    }
    let name = |m: &ObjectMeta| m.name.clone().unwrap_or_default();
    for d in objs.deployments.iter().filter(|o| here(&o.metadata)) {
        let Some(spec) = &d.spec else { continue };
        if !spec_mounts(spec.template.spec.as_ref(), claim) {
            continue;
        }
        u.workloads.push(format!("Deployment/{}", name(&d.metadata)));
        let n = spec.replicas.unwrap_or(1);
        if n > 0 {
            u.blockers.push(format!("Deployment {} is at {}. Scale it to 0.", name(&d.metadata), plural(n, "replica")));
        }
    }
    for s in objs.statefulsets.iter().filter(|o| here(&o.metadata)) {
        let Some(spec) = &s.spec else { continue };
        if !spec_mounts(spec.template.spec.as_ref(), claim) && !sts_owns(s, claim) {
            continue;
        }
        u.workloads.push(format!("StatefulSet/{}", name(&s.metadata)));
        let n = spec.replicas.unwrap_or(1);
        if n > 0 {
            u.blockers.push(format!("StatefulSet {} is at {}. Scale it to 0.", name(&s.metadata), plural(n, "replica")));
        }
    }
    for d in objs.daemonsets.iter().filter(|o| here(&o.metadata)) {
        if d.spec.as_ref().is_some_and(|s| spec_mounts(s.template.spec.as_ref(), claim)) {
            u.workloads.push(format!("DaemonSet/{}", name(&d.metadata)));
            u.blockers.push(format!("DaemonSet {} mounts it and can't be scaled down. Delete or archive it first.", name(&d.metadata)));
        }
    }
    for j in objs.jobs.iter().filter(|o| here(&o.metadata)) {
        if j.spec.as_ref().is_some_and(|s| spec_mounts(s.template.spec.as_ref(), claim)) {
            u.workloads.push(format!("Job/{}", name(&j.metadata)));
            let suspended = j.spec.as_ref().and_then(|s| s.suspend).unwrap_or(false);
            if !job_finished(j) && !suspended {
                u.blockers.push(format!("Job {} hasn't finished and may start pods that mount it.", name(&j.metadata)));
            }
        }
    }
    for c in objs.cronjobs.iter().filter(|o| here(&o.metadata)) {
        let tmpl = c.spec.job_template.spec.as_ref().and_then(|s| s.template.spec.as_ref());
        if spec_mounts(tmpl, claim) {
            u.workloads.push(format!("CronJob/{}", name(&c.metadata)));
            if !c.spec.suspend.unwrap_or(false) {
                u.blockers.push(format!("CronJob {} isn't suspended; its next run would mount it. Suspend it first.", name(&c.metadata)));
            }
        }
    }
    u
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::Deployment;
    use serde_json::json;

    fn pod(name: &str, claim: &str, phase: &str, helper: bool) -> Pod {
        let labels = if helper { json!({ HELPER_LABEL: "true" }) } else { json!({}) };
        serde_json::from_value(json!({
            "metadata": { "name": name, "namespace": "apps", "labels": labels },
            "spec": { "nodeName": "n1", "containers": [{ "name": "c" }],
                      "volumes": [{ "name": "v", "persistentVolumeClaim": { "claimName": claim } }] },
            "status": { "phase": phase }
        }))
        .unwrap()
    }

    fn deployment(name: &str, claim: &str, replicas: i32) -> Deployment {
        serde_json::from_value(json!({
            "metadata": { "name": name, "namespace": "apps" },
            "spec": { "replicas": replicas, "selector": {}, "template": { "spec": {
                "containers": [{ "name": "c" }],
                "volumes": [{ "name": "v", "persistentVolumeClaim": { "claimName": claim } }] } } }
        }))
        .unwrap()
    }

    fn sts(name: &str, template: &str, replicas: i32) -> StatefulSet {
        serde_json::from_value(json!({
            "metadata": { "name": name, "namespace": "apps" },
            "spec": { "replicas": replicas, "serviceName": name, "selector": {},
                      "template": { "spec": { "containers": [{ "name": "c" }] } },
                      "volumeClaimTemplates": [{ "metadata": { "name": template } }] }
        }))
        .unwrap()
    }

    #[test]
    fn paths_are_cleaned_and_confined() {
        assert_eq!(clean_rel("").unwrap(), "");
        assert_eq!(clean_rel("/a//b/").unwrap(), "a/b");
        assert_eq!(clean_rel("dir/a\\b").unwrap(), "dir/a\\b", "a backslash is part of the name");
        assert!(clean_rel("a/../b").is_err());
        assert!(clean_rel("./a").is_err());
        assert_eq!(abs(""), "/data");
        assert_eq!(abs("a/b"), "/data/a/b");
        assert_eq!(join("", "x"), "x");
        assert_eq!(join("a", "x"), "a/x");
        assert!(valid_name("report.csv").is_ok());
        assert!(valid_name("..").is_err());
        assert!(valid_name("a/b").is_err());
        assert!(valid_name("").is_err());
    }

    #[test]
    fn commands_pass_paths_as_arguments() {
        let c = put_command("dir/it's here.txt", 12);
        assert_eq!(&c[..2], &["sh", "-c"]);
        assert_eq!(&c[3..], &["sh", "/data/dir/it's here.txt", "12"]);
        assert_eq!(&tar_command("")[4..], &["/", "data"]);
        assert_eq!(&tar_command("logs/old")[4..], &["/data/logs", "old"]);
        assert_eq!(&mkdirs_command("up", &["a".into(), "a/b".into()])[4..], &["/data/up", "a", "a/b"]);
    }

    #[test]
    fn parses_listing_and_df() {
        let out = "F|4096|1700000000|directory|logs\n\
                   F|12|1700000100|regular file|b|pipe.txt\n\
                   F|0|1700000200|regular empty file|A.txt\n\
                   F|7|1700000300|symbolic link|current\n\
                   F|7|1700000300|symbolic link|dangling\n\
                   L|current\n\
                   garbage line\n\
                   F|3|1700000400|regular file|notes##df\n\
                   F|5|1700000500|regular file|two\n\
                   lines.txt\n\
                   ##df\n\
                   /dev/sda1 1000 400 600 40% /data\n";
        let l = parse_listing("x", out);
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(
            names,
            ["current", "logs", "A.txt", "b|pipe.txt", "dangling", "notes##df"],
            "folders (and links to them) first; the name with a newline is dropped"
        );
        assert!(l.entries[0].link_to_dir);
        assert_eq!(l.entries[2].kind, "file");
        assert_eq!(l.entries[3].size, 12);
        assert_eq!(l.entries[3].modified_ms, Some(1_700_000_100_000));
        assert!(!l.entries[4].link_to_dir);
        assert_eq!((l.total_bytes, l.free_bytes), (Some(1000 * 1024), Some(600 * 1024)));
        assert!(parse_listing("", "##df\n").entries.is_empty());
    }

    fn objs(pods: Vec<Pod>, deployments: Vec<Deployment>, statefulsets: Vec<StatefulSet>) -> ClusterObjects {
        ClusterObjects { pods, deployments, statefulsets, ..Default::default() }
    }

    #[test]
    fn writes_blocked_while_anything_uses_the_claim() {
        let pods = vec![
            pod("web-1", "data", "Running", false),
            pod("old", "data", "Succeeded", false),
            pod("helper", "data", "Running", true),
            pod("other", "else", "Running", false),
        ];
        let deps = vec![deployment("web", "data", 1), deployment("idle", "data", 0)];
        let u = claim_users("apps", "data", &objs(pods.clone(), deps.clone(), vec![]));
        assert_eq!(u.pods, vec![("web-1".to_string(), Some("n1".to_string()))], "finished pods and helpers don't count");
        assert_eq!(u.workloads, ["Deployment/web", "Deployment/idle"]);
        assert_eq!(u.blockers.len(), 2, "{:?}", u.blockers);
        assert!(u.blockers[1].contains("Scale it to 0"));

        let quiet = claim_users("apps", "data", &objs(pods[1..3].to_vec(), deps[1..].to_vec(), vec![]));
        assert!(quiet.blockers.is_empty(), "scaled to 0 and unmounted → writable");
        assert!(claim_users("other-ns", "data", &objs(pods, deps, vec![])).workloads.is_empty(), "same name, other namespace");
    }

    #[test]
    fn statefulset_templates_own_their_claims() {
        let o = objs(vec![], vec![], vec![sts("db", "data", 0)]);
        assert_eq!(claim_users("apps", "data-db-0", &o).workloads, ["StatefulSet/db"]);
        assert!(claim_users("apps", "data-db-0", &o).blockers.is_empty());
        assert!(claim_users("apps", "data-db-x", &o).workloads.is_empty());
        assert!(claim_users("apps", "data-dbx-0", &o).workloads.is_empty());
        let up = objs(vec![], vec![], vec![sts("db", "data", 2)]);
        assert_eq!(claim_users("apps", "data-db-1", &up).blockers.len(), 1);
    }

    #[test]
    fn helper_pod_spec() {
        let p = helper_pod("apps", "data", "busybox:1.37", Some("n1"), true, "me");
        assert_eq!(helper_owner(&p), Some("me"));
        assert!(is_helper(&p));
        assert_eq!(helper_claim(&p), Some("data"));
        let spec = p.spec.unwrap();
        assert_eq!(spec.node_name.as_deref(), Some("n1"));
        assert_eq!(spec.containers[0].volume_mounts.as_ref().unwrap()[0].read_only, Some(true));
        assert_eq!(spec.volumes.unwrap()[0].persistent_volume_claim.as_ref().unwrap().read_only, Some(true));
        let rw = helper_pod("apps", "data", "busybox:1.37", None, false, "me").spec.unwrap();
        assert!(rw.node_name.is_none());
        assert_eq!(rw.containers[0].volume_mounts.as_ref().unwrap()[0].read_only, Some(false));
    }
}
