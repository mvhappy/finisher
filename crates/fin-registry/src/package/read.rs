//! The strict reader (spec §6.4). Two passes:
//! 1. the central directory alone: entry count, names ([`super::names`]), types (regular
//!    files and directories only), compression (stored or deflate, not encrypted), declared
//!    sizes against [`Limits`] and the per-entry ratio, the root manifest;
//! 2. extraction into a new `dest`: the manifest first (parsed for its executables), then
//!    every file streamed to a temp file in a private directory inside `dest`, refused unless
//!    it yields exactly its declared size, given mode 0644 (0755 when the manifest names it
//!    as an executable) and renamed into place. Directories are created one component at a
//!    time, each checked to be a real directory, so nothing is written through a path that
//!    leaves `dest`. Any failure removes `dest`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use fin_plugin_api::{MANIFEST_FILE, MAX_MANIFEST_BYTES, Manifest};
use sha2::{Digest, Sha256};
use zip::{CompressionMethod, ZipArchive};

use super::names::Names;
use super::tree::{class, hash_records};
use super::{FileMeta, Limits, PackageError, Unpacked, hex};

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

struct Entry {
    index: usize,
    path: String,
    dir: bool,
    size: u64,
}

fn corrupt(entry: &str, e: impl std::fmt::Display) -> PackageError {
    PackageError::unsafe_entry(entry, "corrupt", e)
}

/// Pass 1: the central directory.
fn scan<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    limits: &Limits,
) -> Result<(Vec<Entry>, Names), PackageError> {
    // The archive keeps one entry per name: a second one (how some tools store hard links)
    // would be invisible here and visible to other readers.
    if zip.has_duplicate_names() {
        return Err(PackageError::unsafe_entry(
            "",
            "duplicate",
            "two central-directory entries share a name",
        ));
    }
    if zip.len() > limits.entries as usize {
        return Err(PackageError::too_large(
            "entries",
            limits.entries.into(),
            zip.len() as u64,
        ));
    }
    let mut names = Names::new(*limits);
    let mut entries = Vec::with_capacity(zip.len());
    let mut total = 0u64;
    for index in 0..zip.len() {
        let f = zip
            .by_index_raw(index)
            .map_err(|e| corrupt(&format!("#{index}"), e))?;
        let raw = f.name_raw().to_vec();
        let shown = String::from_utf8_lossy(&raw).into_owned();
        let Ok(name) = String::from_utf8(raw) else {
            return Err(PackageError::unsafe_entry(
                &shown,
                "notUtf8",
                "a name that is not UTF-8",
            ));
        };
        // Without the UTF-8 flag (bit 11) a non-ASCII name is CP437 to other readers.
        const UTF8_FLAG: u16 = 0x0800;
        if !name.is_ascii() && f.flags().as_u16() & UTF8_FLAG == 0 {
            return Err(PackageError::unsafe_entry(
                &name,
                "notUtf8",
                "a non-ASCII name without the UTF-8 flag",
            ));
        }
        if f.encrypted() {
            return Err(PackageError::unsafe_entry(
                &name,
                "encrypted",
                "an encrypted entry",
            ));
        }
        let method = f.compression();
        if method != CompressionMethod::Stored && method != CompressionMethod::Deflated {
            return Err(PackageError::unsafe_entry(
                &name,
                "compression",
                format!("compression {method:?}"),
            ));
        }
        let dir = name.ends_with('/');
        let path = name.strip_suffix('/').unwrap_or(&name).to_string();
        let kind = f.unix_mode().map_or(0, |m| m & S_IFMT);
        let type_ok = match kind {
            0 => true,
            S_IFREG => !dir,
            S_IFDIR => dir,
            _ => false,
        };
        if f.is_symlink() || kind == S_IFLNK {
            return Err(PackageError::unsafe_entry(
                &name,
                "symlink",
                "a symbolic link",
            ));
        }
        if !type_ok {
            return Err(PackageError::unsafe_entry(
                &name,
                "notRegular",
                "not a regular file or directory",
            ));
        }
        names.add(&name, &path, dir)?;
        let size = f.size();
        if dir {
            if size != 0 {
                return Err(PackageError::unsafe_entry(
                    &name,
                    "dirWithData",
                    "a directory with data",
                ));
            }
        } else {
            total = total.saturating_add(size);
            if total > limits.unpacked {
                return Err(PackageError::too_large(
                    "unpackedBytes",
                    limits.unpacked,
                    total,
                ));
            }
            let compressed = f.compressed_size();
            if method == CompressionMethod::Stored && compressed != size {
                return Err(PackageError::unsafe_entry(
                    &name,
                    "sizeMismatch",
                    "a stored entry whose sizes differ",
                ));
            }
            if size > u64::from(limits.ratio).saturating_mul(compressed.max(1)) {
                return Err(PackageError::unsafe_entry(
                    &name,
                    "ratio",
                    format!("{size} bytes from {compressed}: over {}:1", limits.ratio),
                ));
            }
        }
        entries.push(Entry {
            index,
            path,
            dir,
            size,
        });
    }
    Ok((entries, names))
}

/// Reads entry `e` fully, refusing anything but exactly its declared size.
fn copy_entry<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    e: &Entry,
    out: &mut dyn Write,
) -> Result<String, PackageError> {
    let mut f = zip.by_index(e.index).map_err(|err| corrupt(&e.path, err))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut n = 0u64;
    let mut limited = (&mut f).take(e.size.saturating_add(1));
    loop {
        let k = limited
            .read(&mut buf)
            .map_err(|err| corrupt(&e.path, err))?;
        if k == 0 {
            break;
        }
        n += k as u64;
        if n > e.size {
            return Err(PackageError::unsafe_entry(
                &e.path,
                "sizeMismatch",
                "more bytes than declared",
            ));
        }
        h.update(&buf[..k]);
        out.write_all(&buf[..k])
            .map_err(|err| PackageError::unsafe_entry(&e.path, "write", err))?;
    }
    if n != e.size {
        return Err(PackageError::unsafe_entry(
            &e.path,
            "sizeMismatch",
            format!("{n} bytes; {} declared", e.size),
        ));
    }
    Ok(hex(&h.finalize()))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result<(), PackageError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| PackageError::io("chmod", path, e))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result<(), PackageError> {
    Ok(())
}

/// `root/<parts…>`, creating each missing directory and checking each is a real directory.
fn ensure_dir(
    root: &Path,
    parts: &[&str],
    made: &mut BTreeSet<String>,
) -> Result<PathBuf, PackageError> {
    let mut at = root.to_path_buf();
    for (i, part) in parts.iter().enumerate() {
        at.push(part);
        let key = parts[..=i].join("/");
        if made.contains(&key) {
            continue;
        }
        match fs::create_dir(&at) {
            Ok(()) => set_mode(&at, 0o755)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(PackageError::io("mkdir", &at, e)),
        }
        let meta = fs::symlink_metadata(&at).map_err(|e| PackageError::io("stat", &at, e))?;
        if !meta.file_type().is_dir() {
            return Err(PackageError::unsafe_entry(
                &key,
                "notDirectory",
                "a directory path that is not a directory",
            ));
        }
        made.insert(key);
    }
    Ok(at)
}

/// Unpacks `archive` into `dest`, which must not exist (its parent must).
pub fn unpack(
    archive: &Path,
    dest: &Path,
    executables: impl Fn(&Manifest) -> Vec<String>,
    limits: &Limits,
) -> Result<Unpacked, PackageError> {
    let file = fs::File::open(archive).map_err(|e| PackageError::io("open", archive, e))?;
    unpack_from(BufReader::new(file), dest, executables, limits)
}

/// [`unpack`] of a package already in memory, for callers that must use the very bytes they
/// checked (the editor hashes and publishes the bytes it unpacked, read once).
pub fn unpack_bytes(
    archive: &[u8],
    dest: &Path,
    executables: impl Fn(&Manifest) -> Vec<String>,
    limits: &Limits,
) -> Result<Unpacked, PackageError> {
    unpack_from(std::io::Cursor::new(archive), dest, executables, limits)
}

fn unpack_from<R: Read + std::io::Seek>(
    archive: R,
    dest: &Path,
    executables: impl Fn(&Manifest) -> Vec<String>,
    limits: &Limits,
) -> Result<Unpacked, PackageError> {
    let mut zip = ZipArchive::new(archive).map_err(|e| corrupt("", e))?;
    let (entries, names) = scan(&mut zip, limits)?;
    let Some(manifest_entry) = entries.iter().find(|e| !e.dir && e.path == MANIFEST_FILE) else {
        return Err(PackageError::manifest(
            "missing",
            format!("no {MANIFEST_FILE} at the package root"),
        ));
    };
    if manifest_entry.size > MAX_MANIFEST_BYTES as u64 {
        return Err(PackageError::manifest_too_large(manifest_entry.size));
    }
    fs::create_dir(dest).map_err(|e| PackageError::io("mkdir", dest, e))?;
    let r = extract(
        &mut zip,
        &entries,
        &names,
        manifest_entry,
        dest,
        executables,
    );
    if r.is_err() {
        let _ = fs::remove_dir_all(dest);
    }
    r
}

fn extract<R: Read + std::io::Seek>(
    zip: &mut ZipArchive<R>,
    entries: &[Entry],
    names: &Names,
    manifest_entry: &Entry,
    dest: &Path,
    executables: impl Fn(&Manifest) -> Vec<String>,
) -> Result<Unpacked, PackageError> {
    set_mode(dest, 0o755)?;
    let root = fs::canonicalize(dest).map_err(|e| PackageError::io("canonicalize", dest, e))?;
    let mut manifest = Vec::new();
    copy_entry(zip, manifest_entry, &mut manifest)?;
    let parsed: Manifest = serde_json::from_slice(&manifest)
        .map_err(|e| PackageError::manifest("schema", format!("{MANIFEST_FILE}: {e}")))?;
    let exec: BTreeSet<String> = executables(&parsed).into_iter().collect();

    // A private directory for temp files, named like no top-level entry.
    let tmp_name = (0..)
        .map(|i| format!(".fin-unpack-{i}"))
        .find(|n| !names.top_level_taken(n))
        .unwrap_or_default();
    let tmp_dir = root.join(&tmp_name);
    fs::create_dir(&tmp_dir).map_err(|e| PackageError::io("mkdir", &tmp_dir, e))?;
    let mut made = BTreeSet::new();
    let mut files = BTreeMap::new();
    for e in entries {
        let parts: Vec<&str> = e.path.split('/').collect();
        if e.dir {
            ensure_dir(&root, &parts, &mut made)?;
            continue;
        }
        let parent = ensure_dir(&root, &parts[..parts.len() - 1], &mut made)?;
        let tmp = tmp_dir.join(format!("{}", e.index));
        let mut out =
            fs::File::create_new(&tmp).map_err(|err| PackageError::io("create", &tmp, err))?;
        let sha256 = copy_entry(zip, e, &mut out)?;
        out.sync_all()
            .map_err(|err| PackageError::io("fsync", &tmp, err))?;
        drop(out);
        let executable = exec.contains(&e.path);
        set_mode(&tmp, if executable { 0o755 } else { 0o644 })?;
        let target = parent.join(parts[parts.len() - 1]);
        if fs::symlink_metadata(&target).is_ok() {
            return Err(PackageError::unsafe_entry(
                &e.path,
                "duplicate",
                "the path exists",
            ));
        }
        fs::rename(&tmp, &target).map_err(|err| PackageError::io("rename", &target, err))?;
        files.insert(
            e.path.clone(),
            FileMeta {
                size: e.size,
                sha256,
                executable,
            },
        );
    }
    fs::remove_dir(&tmp_dir).map_err(|e| PackageError::io("rmdir", &tmp_dir, e))?;
    let records = files
        .iter()
        .map(|(p, m)| (p.clone(), (class(m.executable), m.sha256.clone())))
        .collect();
    Ok(Unpacked {
        root: dest.to_path_buf(),
        tree_hash: hash_records(&records),
        manifest,
        files,
    })
}
