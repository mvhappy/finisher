//! Where registry bytes come from (spec §6.6): a [`Transport`] reads a path relative to the
//! repository root (`metadata/<file>` or `targets/<path>`) and refuses more than `max` bytes.
//! [`fs::FsTransport`] reads a local folder (the only v1 kind; a git registry is a folder the
//! user pulls). An HTTP transport exists only in test builds (feature `test-http`).

pub mod fs;
#[cfg(feature = "test-http")]
pub mod http;

use std::fmt;

pub use fs::FsTransport;
#[cfg(feature = "test-http")]
pub use http::HttpTransport;

/// Why a fetch failed. Only [`TransportError::NotFound`] means "no such file": the TUF client
/// ends the root walk on it and on nothing else (8A Task 1, gap 3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The file does not exist.
    NotFound { path: String },
    /// The file holds more than `max` bytes.
    TooLarge { path: String, max: u64 },
    /// The path is not a plain relative path inside the repository.
    BadPath { path: String },
    /// Anything else: I/O, a non-regular file, a refused connection.
    Unreachable { path: String, reason: String },
}

impl TransportError {
    pub fn path(&self) -> &str {
        match self {
            TransportError::NotFound { path }
            | TransportError::TooLarge { path, .. }
            | TransportError::BadPath { path }
            | TransportError::Unreachable { path, .. } => path,
        }
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TransportError::NotFound { path } => write!(f, "{path}: not found"),
            TransportError::TooLarge { path, max } => write!(f, "{path}: over {max} bytes"),
            TransportError::BadPath { path } => write!(f, "{path:?}: not a repository path"),
            TransportError::Unreachable { path, reason } => write!(f, "{path}: {reason}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// A source of repository files. `path` uses `/` separators and never starts with `/`.
pub trait Transport: Send + Sync {
    fn fetch(&self, path: &str, max: u64) -> Result<Vec<u8>, TransportError>;
}

/// Splits a repository path into its components, refusing anything but plain relative names:
/// no empty, `.` or `..` component, and no `\` (a separator on Windows), `:` (drive letters,
/// NTFS streams), NUL or other control character in any component.
pub(crate) fn components(path: &str) -> Result<Vec<&str>, TransportError> {
    let bad = || TransportError::BadPath { path: path.into() };
    if path.starts_with('/') {
        return Err(bad());
    }
    let parts: Vec<&str> = path.split('/').collect();
    let plain = |c: &&str| {
        !c.is_empty()
            && *c != "."
            && *c != ".."
            && !c
                .chars()
                .any(|ch| ch == '\\' || ch == ':' || ch.is_control())
    };
    if !parts.iter().all(plain) {
        return Err(bad());
    }
    Ok(parts)
}

#[cfg(test)]
mod tests {
    use super::components;

    #[test]
    fn only_plain_relative_paths() {
        assert_eq!(
            components("targets/acme/a.finpkg").unwrap(),
            ["targets", "acme", "a.finpkg"]
        );
        for p in [
            "",
            "/etc/passwd",
            "targets//a",
            "targets/./a",
            "targets/../a",
            "targets/a/",
            "targets\\..\\a",
            "targets/..\\a",
            "targets/a\\b",
            "C:/a",
            "targets/a:stream",
            "targets/a\0b",
            "targets/a\nb",
        ] {
            assert!(components(p).is_err(), "{p:?}");
        }
    }
}
