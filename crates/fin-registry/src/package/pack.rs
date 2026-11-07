//! The reproducible packer (8A Decision 7): the same tree packs to the same bytes on every OS.
//! Files only (directories are implied), sorted by name, deflate at level 6, the DOS epoch
//! (1980-01-01 00:00) as every timestamp, made by "Unix" with modes 0644 or 0755 from the
//! `executables` list (never from the file system), no extra fields, no comment. A tree the
//! reader would refuse (names, symlinks, special files, sizes) is refused here first.

use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
use std::path::Path;

use fin_plugin_api::MANIFEST_FILE;
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, DateTime, System, ZipWriter};

use super::names::Names;
use super::tree::{class, hash_records, sha256_file, walk};
use super::{Limits, PackageError, hex};

/// What [`pack`] wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackInfo {
    /// The archive's size.
    pub bytes: u64,
    /// Lowercase hex SHA-256 of the archive.
    pub sha256: String,
    /// Files in the archive.
    pub entries: u32,
    /// The [`super::tree_hash`] the archive unpacks to on this OS.
    pub tree_hash: String,
}

fn options(executable: bool) -> SimpleFileOptions {
    SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(DateTime::default())
        .system(System::Unix)
        .unix_permissions(if executable { 0o755 } else { 0o644 })
        .large_file(false)
}

/// Packs the files under `src` into `out` (written to `<out>.tmp`, then renamed).
/// `executables` are package paths that get mode 0755; each must be a file of `src`.
pub fn pack(src: &Path, executables: &[String], out: &Path) -> Result<PackInfo, PackageError> {
    let limits = Limits::default();
    let files = walk(src)?;
    if files.len() > limits.entries as usize {
        return Err(PackageError::too_large(
            "entries",
            limits.entries.into(),
            files.len() as u64,
        ));
    }
    if !files.contains_key(MANIFEST_FILE) {
        return Err(PackageError::manifest(
            "missing",
            format!("no {MANIFEST_FILE} in {}", src.display()),
        ));
    }
    let exec: BTreeSet<&str> = executables.iter().map(String::as_str).collect();
    if let Some(missing) = exec.iter().find(|e| !files.contains_key(**e)) {
        return Err(PackageError::unsafe_entry(
            missing,
            "executableMissing",
            "an executable that is not in the tree",
        ));
    }
    let mut names = Names::new(limits);
    for path in files.keys() {
        names.add(path, path, false)?;
    }
    let tmp = {
        let mut s = out.as_os_str().to_owned();
        s.push(".tmp");
        std::path::PathBuf::from(s)
    };
    let r = write_archive(src, &files, &exec, &limits, &tmp);
    let records = match r {
        Ok(records) => records,
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    };
    fs::rename(&tmp, out).map_err(|e| PackageError::io("rename", out, e))?;
    let (bytes, sha256) = sha256_file(out)?;
    Ok(PackInfo {
        bytes,
        sha256,
        entries: files.len() as u32,
        tree_hash: hash_records(&records),
    })
}

type Records = std::collections::BTreeMap<String, (char, String)>;

fn write_archive(
    src: &Path,
    files: &std::collections::BTreeMap<String, bool>,
    exec: &BTreeSet<&str>,
    limits: &Limits,
    tmp: &Path,
) -> Result<Records, PackageError> {
    let file = fs::File::create(tmp).map_err(|e| PackageError::io("create", tmp, e))?;
    let mut zip = ZipWriter::new(file);
    let mut records = Records::new();
    let mut total = 0u64;
    // `files` is a BTreeMap: sorted by path bytes.
    for path in files.keys() {
        let bytes =
            fs::read(src.join(path)).map_err(|e| PackageError::io("read", &src.join(path), e))?;
        total += bytes.len() as u64;
        if total > limits.unpacked {
            return Err(PackageError::too_large(
                "unpackedBytes",
                limits.unpacked,
                total,
            ));
        }
        let executable = exec.contains(path.as_str());
        zip.start_file(path.as_str(), options(executable))
            .map_err(|e| PackageError::io("zip", tmp, e))?;
        zip.write_all(&bytes)
            .map_err(|e| PackageError::io("zip", tmp, e))?;
        records.insert(
            path.clone(),
            (class(executable), hex(&Sha256::digest(&bytes))),
        );
    }
    let file = zip.finish().map_err(|e| PackageError::io("zip", tmp, e))?;
    file.sync_all()
        .map_err(|e| PackageError::io("fsync", tmp, e))?;
    Ok(records)
}
