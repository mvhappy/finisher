//! Tree hashes: BLAKE3 over the sorted `(path, mode class, sha256)` of every file in a tree
//! (8A Plan: staging re-hash at approval, `BUNDLED_PLUGINS` at build time). Directories add
//! nothing, so an empty directory does not change the hash.
//!
//! The mode class is `x` for a file with the owner-execute bit and `f` otherwise. Windows has
//! no execute bit: there every file is `f`, both when [`tree_hash`] reads a tree and when
//! [`super::unpack`] records one, so a hash compares on one OS but not across OSes when a
//! package has executables.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{PackageError, hex};

const DOMAIN: &[u8] = b"finisher-tree-v1\n";

/// The mode class of a file that is (or is not) executable, on this OS.
pub(crate) fn class(executable: bool) -> char {
    if cfg!(unix) && executable { 'x' } else { 'f' }
}

/// The hash of `(path → (class, sha256))` records.
pub(crate) fn hash_records(records: &BTreeMap<String, (char, String)>) -> String {
    let mut h = blake3::Hasher::new();
    h.update(DOMAIN);
    for (path, (class, sha)) in records {
        h.update(path.as_bytes());
        h.update(&[0]);
        h.update(&[*class as u8, 0]);
        h.update(sha.as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

pub(crate) fn sha256_file(path: &Path) -> Result<(u64, String), PackageError> {
    let mut f = fs::File::open(path).map_err(|e| PackageError::io("open", path, e))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    loop {
        let k = f
            .read(&mut buf)
            .map_err(|e| PackageError::io("read", path, e))?;
        if k == 0 {
            break;
        }
        h.update(&buf[..k]);
        n += k as u64;
    }
    Ok((n, hex(&h.finalize())))
}

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o100 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    false
}

/// Every regular file under `root` by `/`-joined relative path, with its executable bit.
/// A symlink, a special file or a name that is not UTF-8 is `PACKAGE_UNSAFE`.
pub(crate) fn walk(root: &Path) -> Result<BTreeMap<String, bool>, PackageError> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, prefix)) = stack.pop() {
        let rd = fs::read_dir(&dir).map_err(|e| PackageError::io("read_dir", &dir, e))?;
        for entry in rd {
            let entry = entry.map_err(|e| PackageError::io("read_dir", &dir, e))?;
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                let shown = entry.file_name().to_string_lossy().into_owned();
                return Err(PackageError::unsafe_entry(
                    &shown,
                    "notUtf8",
                    "a name that is not UTF-8",
                ));
            };
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let meta =
                fs::symlink_metadata(&path).map_err(|e| PackageError::io("stat", &path, e))?;
            let ft = meta.file_type();
            if ft.is_symlink() {
                return Err(PackageError::unsafe_entry(
                    &rel,
                    "symlink",
                    "a symbolic link",
                ));
            } else if ft.is_dir() {
                stack.push((path, rel));
            } else if ft.is_file() {
                out.insert(rel, is_executable(&meta));
            } else {
                return Err(PackageError::unsafe_entry(
                    &rel,
                    "notRegular",
                    "not a regular file",
                ));
            }
        }
    }
    Ok(out)
}

/// The tree hash of the files under `root`.
pub fn tree_hash(root: &Path) -> Result<String, PackageError> {
    let mut records = BTreeMap::new();
    for (rel, exec) in walk(root)? {
        let (_, sha) = sha256_file(&root.join(&rel))?;
        records.insert(rel, (class(exec), sha));
    }
    Ok(hash_records(&records))
}
