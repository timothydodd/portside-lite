//! Workload archives on disk. Each archive is a folder under the archive
//! root, `<cluster>/<namespace>/<kind>-<name>/`, holding:
//!
//! - `manifest.yaml`: the workload and its related objects, cleaned for
//!   re-applying, in apply order (what Restore applies);
//! - `archive.json`: [`ArchiveMeta`];
//! - `logs.txt`: the workload's stored log lines, if any were kept.
//!
//! The folder is plain files on purpose: it can be copied, backed up or
//! applied with `kubectl apply -f manifest.yaml` without this app.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use portside_core::archive::{valid_archive_id, ArchiveMeta};

use crate::{Result, StoreError};

pub const MANIFEST_FILE: &str = "manifest.yaml";
pub const META_FILE: &str = "archive.json";
pub const LOGS_FILE: &str = "logs.txt";

/// The folder for `id`, refusing ids that could escape the root.
pub fn dir(root: &Path, id: &str) -> Result<PathBuf> {
    if !valid_archive_id(id) {
        return Err(StoreError::Invalid(format!("not an archive id: {id}")));
    }
    Ok(id.split('/').fold(root.to_path_buf(), |p, seg| p.join(seg)))
}

/// Write file contents so a crash never leaves a half-written file behind.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Create (or replace) the archive `meta.id`. Files are written into a
/// staging folder first and swapped in at the end, so a failure leaves any
/// previous archive of the same workload untouched. `write_logs` fills
/// `logs.txt` and returns the line count; a file with no lines is dropped.
pub fn create(
    root: &Path,
    mut meta: ArchiveMeta,
    manifest: &str,
    write_logs: Option<&dyn Fn(&mut dyn Write) -> Result<usize>>,
) -> Result<ArchiveMeta> {
    let final_dir = dir(root, &meta.id)?;
    let parent = final_dir.parent().expect("archive dirs have a parent");
    fs::create_dir_all(parent)?;
    let leaf = final_dir.file_name().expect("archive dirs have a name").to_string_lossy().into_owned();
    let staging = parent.join(format!(".{leaf}.partial"));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir(&staging)?;

    let staged = (|| -> Result<ArchiveMeta> {
        fs::write(staging.join(MANIFEST_FILE), manifest)?;
        meta.log_lines = 0;
        if let Some(write_logs) = write_logs {
            let path = staging.join(LOGS_FILE);
            let mut file = std::io::BufWriter::new(fs::File::create(&path)?);
            meta.log_lines = write_logs(&mut file)?;
            file.flush()?;
            drop(file);
            if meta.log_lines == 0 {
                fs::remove_file(&path)?;
            }
        }
        fs::write(staging.join(META_FILE), serde_json::to_vec_pretty(&meta)?)?;
        Ok(meta)
    })();
    let meta = match staged {
        Ok(m) => m,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // Swap in: move any previous archive aside, then rename the new one.
    let old = parent.join(format!(".{leaf}.old"));
    if old.exists() {
        fs::remove_dir_all(&old)?;
    }
    if final_dir.exists() {
        fs::rename(&final_dir, &old)?;
    }
    if let Err(e) = fs::rename(&staging, &final_dir) {
        if old.exists() {
            let _ = fs::rename(&old, &final_dir);
        }
        return Err(e.into());
    }
    if old.exists() {
        let _ = fs::remove_dir_all(&old);
    }
    Ok(meta)
}

pub fn read_meta(root: &Path, id: &str) -> Result<ArchiveMeta> {
    let path = dir(root, id)?.join(META_FILE);
    let mut meta: ArchiveMeta = serde_json::from_slice(&fs::read(&path)?)?;
    meta.id = id.to_string(); // the folder is the truth if it was moved
    Ok(meta)
}

pub fn save_meta(root: &Path, meta: &ArchiveMeta) -> Result<()> {
    write_atomic(&dir(root, &meta.id)?.join(META_FILE), &serde_json::to_vec_pretty(meta)?)
}

pub fn read_manifest(root: &Path, id: &str) -> Result<String> {
    Ok(fs::read_to_string(dir(root, id)?.join(MANIFEST_FILE))?)
}

/// Every archive under `root`, newest first. Folders without a readable
/// `archive.json` (half-copied, hand-made) are skipped.
pub fn list(root: &Path) -> Result<Vec<ArchiveMeta>> {
    fn subdirs(p: &Path) -> Vec<(String, PathBuf)> {
        let Ok(rd) = fs::read_dir(p) else { return Vec::new() };
        rd.filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.')).then(|| (name, e.path()))
            })
            .collect()
    }
    let mut out = Vec::new();
    for (cluster, cp) in subdirs(root) {
        for (ns, np) in subdirs(&cp) {
            for (leaf, _) in subdirs(&np) {
                let id = format!("{cluster}/{ns}/{leaf}");
                if let Ok(meta) = read_meta(root, &id) {
                    out.push(meta);
                }
            }
        }
    }
    out.sort_by(|a, b| b.archived_ms.cmp(&a.archived_ms).then_with(|| a.id.cmp(&b.id)));
    Ok(out)
}

/// Remove an archive folder, then any parent folders it leaves empty.
pub fn delete(root: &Path, id: &str) -> Result<()> {
    let d = dir(root, id)?;
    fs::remove_dir_all(&d)?;
    let mut p = d.parent();
    while let Some(parent) = p.filter(|p| *p != root) {
        if fs::remove_dir(parent).is_err() {
            break; // not empty
        }
        p = parent.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use portside_core::archive::{archive_id, ArchivedObject, ARCHIVE_FORMAT};

    fn meta(name: &str, at: i64) -> ArchiveMeta {
        ArchiveMeta {
            id: archive_id("local:default", "apps", "Deployment", name),
            format: ARCHIVE_FORMAT,
            kind: "Deployment".into(),
            namespace: "apps".into(),
            name: name.into(),
            cluster_id: "local:default".into(),
            profile_id: "p1".into(),
            connection_name: "Laptop".into(),
            archived_ms: at,
            replicas: Some(2),
            images: vec!["web:1".into()],
            objects: vec![ArchivedObject { kind: "Deployment".into(), name: name.into(), removed: false, error: None }],
            log_lines: 0,
            restored_ms: None,
            restored_to: None,
        }
    }

    #[test]
    fn create_list_replace_and_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let logs = |w: &mut dyn Write| -> Result<usize> {
            writeln!(w, "line one")?;
            writeln!(w, "line two")?;
            Ok(2)
        };
        let a = create(root, meta("web", 100), "kind: Deployment\n", Some(&logs)).unwrap();
        assert_eq!(a.log_lines, 2);
        let d = dir(root, &a.id).unwrap();
        assert_eq!(fs::read_to_string(d.join(LOGS_FILE)).unwrap(), "line one\nline two\n");
        create(root, meta("api", 200), "kind: Deployment\n", Some(&|_: &mut dyn Write| Ok(0))).unwrap();
        assert!(!dir(root, &meta("api", 0).id).unwrap().join(LOGS_FILE).exists(), "empty log file dropped");

        let names: Vec<String> = list(root).unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec!["api", "web"], "newest first");

        // Re-archiving replaces the old folder in place.
        create(root, meta("web", 300), "kind: Deployment # v2\n", None).unwrap();
        assert_eq!(read_manifest(root, &a.id).unwrap(), "kind: Deployment # v2\n");
        assert!(!d.join(LOGS_FILE).exists());
        assert_eq!(list(root).unwrap().len(), 2);

        let mut m = read_meta(root, &a.id).unwrap();
        m.restored_ms = Some(400);
        save_meta(root, &m).unwrap();
        assert_eq!(read_meta(root, &a.id).unwrap().restored_ms, Some(400));

        delete(root, &a.id).unwrap();
        delete(root, &meta("api", 0).id).unwrap();
        assert!(list(root).unwrap().is_empty());
        assert_eq!(fs::read_dir(root).unwrap().count(), 0, "empty parents cleaned up");
    }

    #[test]
    fn rejects_ids_outside_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(dir(tmp.path(), "../../etc").is_err());
        assert!(delete(tmp.path(), "a/../../b").is_err());
    }
}
