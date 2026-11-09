//! Server data only on a known local filesystem (review #4, architecture spec §6.7): a
//! project's database runs SQLite in WAL mode and holds OS locks (`db.lock`, `lease.lock`),
//! which are only safe on this machine's own disk. A network share, a FUSE or sync folder, or
//! a container's overlayfs (which hides the disk under it) is refused with
//! `SERVER_DATA_NOT_LOCAL` unless the person sets [`ALLOW_NONLOCAL_ENV`]. Symlinks and
//! ancestors are resolved first: the check looks at the filesystem the path really lands on,
//! from its nearest existing ancestor when it does not exist yet.
//!
//! `finisherd` checks the server-data root at startup (fin-node `checkServerDataLocal`);
//! [`Store::create`](crate::Store::create) and [`Store::open`](crate::Store::open) check the
//! project's data folder (`<server-data>/projects/<id>/`) before they write anything there.
//! Project folders are not checked: they may live anywhere (§6.7, write-once files only).

use std::path::{Path, PathBuf};

use fin_core::CoreError;
use serde_json::json;

use crate::StoreError;

/// The override: `1` (or `true`) accepts server data on any filesystem.
pub const ALLOW_NONLOCAL_ENV: &str = "FIN_ALLOW_NONLOCAL_SERVER_DATA";

/// Whether [`ALLOW_NONLOCAL_ENV`] is set to `1` or `true`.
pub fn allowed_by_env() -> bool {
    std::env::var(ALLOW_NONLOCAL_ENV).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// `dir`, or its nearest existing ancestor, absolute with every symlink resolved. `None`
/// when no ancestor can be resolved.
pub fn resolve_existing(dir: &Path) -> Option<PathBuf> {
    let abs = std::path::absolute(dir).ok()?;
    let mut cur: &Path = &abs;
    loop {
        if let Ok(c) = std::fs::canonicalize(cur) {
            return Some(c);
        }
        cur = cur.parent()?;
    }
}

/// Names for the Linux filesystem types a refusal most often meets.
#[cfg(target_os = "linux")]
fn linux_name(t: u64) -> String {
    match t {
        super::storage_kind::local_disk::LINUX_OVERLAY => "overlayfs".into(),
        0x6969 => "nfs".into(),
        0xFF53_4D42 => "cifs".into(),
        0xFE53_4D42 => "smb2".into(),
        0x6573_5546 => "fuse".into(),
        0x0102_1997 => "9p".into(),
        0x9FA0 => "proc".into(),
        0x6265_6572 => "sysfs".into(),
        other => format!("statfs type {other:#x}"),
    }
}

/// `None` when `resolved` is on a known local filesystem, else what it is on.
pub fn not_local(resolved: &Path) -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        use super::storage_kind::local_disk::{LINUX_OVERLAY, is_linux_local, linux_fs_type};
        let Some(t) = linux_fs_type(resolved) else {
            return Some("unknown (the filesystem could not be probed)".into());
        };
        if t == LINUX_OVERLAY || !is_linux_local(t) {
            return Some(linux_name(t));
        }
    }
    #[cfg(not(target_os = "linux"))]
    if !super::storage_kind::local_disk::is_local_disk(resolved) {
        return Some("not a local disk".into());
    }
    // A local disk can still hold a sync client's folder (Dropbox, iCloud, OneDrive).
    let kind = super::storage_kind::detect(resolved, None);
    (kind != super::storage_kind::StorageKind::Local).then(|| format!("{kind} storage"))
}

/// Refuses `dir` (`what`: "server data" or "project data") unless it is on a known local
/// filesystem or `allow` is set.
pub fn check(dir: &Path, what: &str, allow: bool) -> Result<(), StoreError> {
    if allow {
        return Ok(());
    }
    let resolved = resolve_existing(dir);
    let found = match &resolved {
        Some(r) => not_local(r),
        None => Some("unknown (no folder of the path exists)".into()),
    };
    let Some(filesystem) = found else {
        return Ok(());
    };
    let shown = resolved.as_deref().unwrap_or(dir);
    Err(CoreError::new(
        "SERVER_DATA_NOT_LOCAL",
        format!("the {what} folder {} is on {filesystem}, not on a known local disk", dir.display()),
    )
    .with_details(json!({
        "path": dir.to_string_lossy(),
        "resolved": shown.to_string_lossy(),
        "filesystem": filesystem,
        "override": ALLOW_NONLOCAL_ENV,
    }))
    .with_fix(format!(
        "keep the server data on this computer's own disk (the default location is), or set {ALLOW_NONLOCAL_ENV}=1 to use this folder anyway: the project databases and their locks are only safe on a local filesystem"
    ))
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_nearest_existing_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        assert_eq!(resolve_existing(&tmp.path().join("a/b/c")).unwrap(), real);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_and_overlay_are_not_local() {
        assert_eq!(not_local(Path::new("/proc/self")).as_deref(), Some("proc"));
        assert_eq!(
            linux_name(super::super::storage_kind::local_disk::LINUX_OVERLAY),
            "overlayfs"
        );
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            not_local(tmp.path()),
            None,
            "the test's temp dir is on a local disk"
        );
    }
}
