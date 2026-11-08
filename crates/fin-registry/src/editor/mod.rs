//! The repository editor (spec §5.6, §6.1): [`RepoBuilder`] builds and signs local-folder
//! registries for teams and tests, and is what `fin-plugin registry …` runs. It is not test
//! code: the client and the editor share the layout and custom metadata in
//! [`crate::tuf::custom`].
//!
//! Layout of `dir` (the folder users point a registry at, or the clone they `git pull`):
//! `metadata/<n>.root.json`, `timestamp.json`, `<n>.snapshot.json`, `<n>.targets.json`,
//! `<n>.<publisher>.json`; `targets/<publisher>/<name>/<sha256>.<version>.finpkg`,
//! `targets/<publisher>/<sha256>.advisories.json`, `targets/<sha256>.advisories.json`.
//! Private keys live in a separate [`KeyRing`] directory, never inside `dir`.
//!
//! Every change re-signs what it touches, then the snapshot, then writes `timestamp.json`
//! last, each file temp then rename, so a client reading the folder mid-change sees the
//! previous timestamp and the previous, complete set.

mod keys;
mod paths;
mod publish;
mod rotate;
mod state;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Map, Value, json};
use tough::schema::{Signature, Signed};

pub use keys::{KeyRing, PublicKey, SigningKey};
pub use paths::keys_inside;
pub use rotate::Role;
pub use state::GITATTRIBUTES;

use crate::atomic;
use crate::tuf::{Clock, SystemClock};
use state::State;

/// An editor failure `{code, message, details}`: `KEY_INVALID`, `KEY_MISSING {role}`,
/// `REGISTRY_EXISTS`, `REGISTRY_INVALID {reason}`, `PUBLISHER_NOT_DELEGATED {publisher}`,
/// `PUBLISHER_EXISTS {publisher}`, `PLUGIN_VERSION_EXISTS {id, version}`,
/// `PLUGIN_NOT_FOUND {id, version}`, `KEY_UNPROTECTED {path, problems}`, `IO_ERROR {op, path}`,
/// and the package reader's and manifest validation's codes for a package that cannot be
/// published.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorError {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
    /// The command that fixes it, when it depends on the error (`KEY_UNPROTECTED`).
    pub fix: Option<String>,
}

impl EditorError {
    pub(crate) fn new(code: &str, message: impl Into<String>, details: Value) -> EditorError {
        let Value::Object(details) = details else {
            unreachable!("details are an object")
        };
        EditorError {
            code: code.into(),
            message: message.into(),
            details,
            fix: None,
        }
    }

    pub(crate) fn key(message: impl Into<String>) -> EditorError {
        EditorError::new("KEY_INVALID", message, json!({}))
    }

    pub(crate) fn invalid(reason: &str, message: impl Into<String>) -> EditorError {
        EditorError::new("REGISTRY_INVALID", message, json!({ "reason": reason }))
    }

    pub(crate) fn io(op: &str, path: &Path, e: impl fmt::Display) -> EditorError {
        EditorError::new(
            "IO_ERROR",
            format!("{op} {}: {e}", path.display()),
            json!({ "op": op, "path": path.display().to_string() }),
        )
    }
}

impl fmt::Display for EditorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for EditorError {}

impl From<crate::package::PackageError> for EditorError {
    fn from(e: crate::package::PackageError) -> EditorError {
        EditorError {
            code: e.code.into(),
            message: e.message,
            details: e.details,
            fix: None,
        }
    }
}

/// Days until each role expires when it is signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expiry {
    pub root: u32,
    pub targets: u32,
    pub snapshot: u32,
    pub timestamp: u32,
}

impl Default for Expiry {
    fn default() -> Expiry {
        Expiry {
            root: 365,
            targets: 365,
            snapshot: 365,
            timestamp: 30,
        }
    }
}

/// Builds and signs a registry in `dir` with the keys in a [`KeyRing`].
pub struct RepoBuilder {
    dir: PathBuf,
    keys: KeyRing,
    clock: Arc<dyn Clock>,
    pub expiry: Expiry,
    st: State,
    /// Versions to use for the next signature of a role, instead of the next one (tests of
    /// rollback and fast-forward recovery build repositories that go back).
    next: BTreeMap<String, u64>,
}

impl fmt::Debug for RepoBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RepoBuilder")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

/// A role signed and serialised as it is written.
pub(crate) struct SignedFile<T> {
    pub signed: Signed<T>,
    pub bytes: Vec<u8>,
}

/// Signs `role` with every key in `keys` (each once) over its canonical JSON, as TUF 1.0
/// specifies (`olpc-cjson`, the canonical form `tough` verifies).
pub(crate) fn sign<T: Serialize>(role: T, keys: &[SigningKey]) -> SignedFile<T> {
    let mut data = Vec::new();
    let mut ser =
        serde_json::Serializer::with_formatter(&mut data, olpc_cjson::CanonicalFormatter::new());
    role.serialize(&mut ser).expect("a role serialises");
    let mut seen = BTreeSet::new();
    let signatures = keys
        .iter()
        .filter(|k| seen.insert(k.public().key_id()))
        .map(|k| Signature {
            keyid: hex::decode(k.public().key_id())
                .expect("a key id is hex")
                .into(),
            sig: k.sign(&data).into(),
        })
        .collect();
    let signed = Signed {
        signed: role,
        signatures,
    };
    let mut bytes = serde_json::to_vec_pretty(&signed).expect("a signed role serialises");
    bytes.push(b'\n');
    SignedFile { signed, bytes }
}

impl RepoBuilder {
    /// Opens the registry in `dir`, signing with the keys in `keys_dir`.
    pub fn open(dir: &Path, keys_dir: &Path) -> Result<RepoBuilder, EditorError> {
        state::refuse_keys_inside(dir, keys_dir)?;
        let keys = KeyRing::new(keys_dir);
        keys.check()?;
        let st = State::load(dir, &keys)?;
        Ok(RepoBuilder {
            dir: dir.to_path_buf(),
            keys,
            clock: Arc::new(SystemClock),
            expiry: Expiry::default(),
            st,
            next: BTreeMap::new(),
        })
    }

    /// Signs with times from `clock` (expiries are counted from its `now`).
    pub fn with_clock(mut self, clock: Arc<dyn Clock>) -> RepoBuilder {
        self.clock = clock;
        self
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn keys(&self) -> &KeyRing {
        &self.keys
    }

    /// The bytes of the newest root, which clients pin (`--root <file>`).
    pub fn root_bytes(&self) -> Result<Vec<u8>, EditorError> {
        let v = self.st.root.signed.version.get();
        let p = self.metadata().join(format!("{v}.root.json"));
        std::fs::read(&p).map_err(|e| EditorError::io("read", &p, e))
    }

    /// The bytes of `1.root.json`, the root a registry is usually added with.
    pub fn first_root_bytes(&self) -> Result<Vec<u8>, EditorError> {
        let p = self.metadata().join("1.root.json");
        std::fs::read(&p).map_err(|e| EditorError::io("read", &p, e))
    }

    /// The next signature of `role` uses `version` (it may go back; for tests).
    #[doc(hidden)]
    pub fn set_next_version(&mut self, role: &Role, version: u64) {
        self.next.insert(role.name().to_string(), version);
    }

    pub(crate) fn metadata(&self) -> PathBuf {
        self.dir.join("metadata")
    }

    pub(crate) fn targets_dir(&self) -> PathBuf {
        self.dir.join("targets")
    }

    pub(crate) fn expires(&self, days: u32) -> jiff::Timestamp {
        let now = crate::tuf::clock::unix_secs(self.clock.now());
        jiff::Timestamp::from_second(now + i64::from(days) * 86_400).unwrap_or(jiff::Timestamp::MAX)
    }

    /// The version for the next signature of `role`, whose current version is `current`.
    pub(crate) fn bump(&mut self, role: &str, current: u64) -> std::num::NonZeroU64 {
        let v = self.next.remove(role).unwrap_or(current + 1).max(1);
        std::num::NonZeroU64::new(v).expect("at least 1")
    }

    pub(crate) fn write_metadata(&self, name: &str, bytes: &[u8]) -> Result<(), EditorError> {
        let dir = self.metadata();
        std::fs::create_dir_all(&dir).map_err(|e| EditorError::io("create", &dir, e))?;
        let p = dir.join(name);
        atomic::write(&p, bytes).map_err(|e| EditorError::io("write", &p, e))
    }

    pub(crate) fn write_target(&self, rel: &str, bytes: &[u8]) -> Result<(), EditorError> {
        let p = self.targets_dir().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| EditorError::io("create", parent, e))?;
        }
        atomic::write(&p, bytes).map_err(|e| EditorError::io("write", &p, e))
    }

    /// The private keys of `ids` from the ring, at least `threshold` of them.
    pub(crate) fn ring_signers(
        &self,
        role: &str,
        ids: &[String],
        threshold: u64,
    ) -> Result<Vec<SigningKey>, EditorError> {
        let keys = self.keys.signers(ids)?;
        if (keys.len() as u64) < threshold {
            return Err(EditorError::new(
                "KEY_MISSING",
                format!(
                    "the key ring holds {} of the {threshold} {role} keys needed",
                    keys.len()
                ),
                json!({ "role": role }),
            ));
        }
        Ok(keys)
    }
}

/// Signs `role` with `keys` over its canonical JSON and returns the file bytes, for tools
/// that write metadata the editor does not (and for the attack tests, which forge it).
pub fn sign_metadata<T: Serialize>(role: T, keys: &[SigningKey]) -> Vec<u8> {
    sign(role, keys).bytes
}
