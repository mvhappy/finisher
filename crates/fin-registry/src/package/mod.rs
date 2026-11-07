//! `.finpkg` packages (spec §6.4, 8A Decision 7): zip archives with `finisher-plugin.json` at
//! the root. Packages carry no signature of their own; their identity is the hash and length
//! in signed targets metadata, and once unpacked, their [`tree_hash`].

mod limits;
mod names;
mod pack;
mod read;
mod tree;

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

pub use limits::Limits;
pub use names::{check_component, check_path};
pub use pack::{PackInfo, pack};
pub use read::unpack;
pub use tree::tree_hash;

/// A file of an unpacked package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMeta {
    pub size: u64,
    /// Lowercase hex SHA-256 of the bytes.
    pub sha256: String,
    /// Mode 0755 (the manifest names it as an executable); otherwise 0644.
    pub executable: bool,
}

/// A package unpacked into `root` by [`unpack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unpacked {
    pub root: PathBuf,
    /// [`tree_hash`] of `root`.
    pub tree_hash: String,
    /// The bytes of `finisher-plugin.json`.
    pub manifest: Vec<u8>,
    /// Every file, by package path.
    pub files: BTreeMap<String, FileMeta>,
}

/// A package error as `{code, message, details}`:
/// - `PACKAGE_UNSAFE {entry, reason}`: an entry the reader refuses;
/// - `PACKAGE_TOO_LARGE {reason, limit, got}`: entries or unpacked bytes over [`Limits`];
/// - `PLUGIN_MANIFEST_INVALID {path, reason}`: no root manifest, or one that does not parse;
/// - `MANIFEST_TOO_LARGE {bytes, max}`;
/// - `IO_ERROR {op, path}`: the file system failed.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageError {
    pub code: &'static str,
    pub message: String,
    pub details: Map<String, Value>,
}

impl PackageError {
    fn new(code: &'static str, message: impl Into<String>, details: Value) -> PackageError {
        let Value::Object(details) = details else {
            unreachable!("details are an object")
        };
        PackageError {
            code,
            message: message.into(),
            details,
        }
    }

    pub(crate) fn unsafe_entry(entry: &str, reason: &str, why: impl fmt::Display) -> PackageError {
        PackageError::new(
            "PACKAGE_UNSAFE",
            format!("entry {entry:?}: {why}"),
            json!({ "entry": entry, "reason": reason }),
        )
    }

    pub(crate) fn too_large(reason: &str, limit: u64, got: u64) -> PackageError {
        PackageError::new(
            "PACKAGE_TOO_LARGE",
            format!("the package has {got} {reason}; at most {limit}"),
            json!({ "reason": reason, "limit": limit, "got": got }),
        )
    }

    pub(crate) fn manifest(reason: &str, message: impl Into<String>) -> PackageError {
        PackageError::new(
            "PLUGIN_MANIFEST_INVALID",
            message,
            json!({ "path": "", "reason": reason }),
        )
    }

    pub(crate) fn manifest_too_large(bytes: u64) -> PackageError {
        let max = fin_plugin_api::MAX_MANIFEST_BYTES;
        PackageError::new(
            "MANIFEST_TOO_LARGE",
            format!(
                "{} is {bytes} bytes; at most {max}",
                fin_plugin_api::MANIFEST_FILE
            ),
            json!({ "bytes": bytes, "max": max }),
        )
    }

    pub(crate) fn io(op: &str, path: &std::path::Path, e: impl fmt::Display) -> PackageError {
        PackageError::new(
            "IO_ERROR",
            format!("{op} {}: {e}", path.display()),
            json!({ "op": op, "path": path.display().to_string() }),
        )
    }

    /// `details.reason`, when the code has one.
    pub fn reason(&self) -> Option<&str> {
        self.details.get("reason").and_then(Value::as_str)
    }

    /// `details.entry`, for `PACKAGE_UNSAFE`.
    pub fn entry(&self) -> Option<&str> {
        self.details.get("entry").and_then(Value::as_str)
    }
}

impl fmt::Display for PackageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for PackageError {}

/// Lowercase hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    use fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
