//! Trusted metadata per root (spec §5.1, §6.3; 8A Decision 14):
//!
//! ```text
//! <server-data>/plugins/trust/<rootSha256>/
//!   roots/<n>.root.json     every trusted root, the bytes as fetched (never re-serialised)
//!   current                 the trusted generation's number (replaced atomically)
//!   gen-<k>/                timestamp.json, snapshot.json, targets.json, roles/<role>.json
//!   state.json              {lastRefreshAt} of the last successful refresh
//!   tmp/                    scratch datastores and staging; emptied on every refresh
//! ```
//!
//! A refresh writes a whole new generation and then replaces `current`, so the trusted set
//! changes all at once or not at all. `<rootSha256>` is the hash of the root the registry was
//! added with; the directory outlives the registry's config (removal keeps trust).

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::TufError;
use crate::atomic;

const ROOTS: &str = "roots";
const CURRENT: &str = "current";
const STATE: &str = "state.json";
const TMP: &str = "tmp";

/// The trusted metadata of one root.
#[derive(Debug, Clone)]
pub struct TrustStore {
    dir: PathBuf,
    root_sha256: String,
}

/// What a successful refresh records (8A Decision 13).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustState {
    /// Unix seconds of the injected clock at the last successful refresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_refresh_at: Option<i64>,
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn read_opt(path: &Path) -> Result<Option<Vec<u8>>, TufError> {
    match fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(TufError::io("read", path, e)),
    }
}

impl TrustStore {
    /// The store of the root whose SHA-256 is `root_sha256`, under `trust_base`
    /// (`<server-data>/plugins/trust`). Creates nothing.
    pub fn new(trust_base: &Path, root_sha256: &str) -> Result<TrustStore, TufError> {
        let sha = root_sha256.to_ascii_lowercase();
        if !is_sha256_hex(&sha) {
            return Err(TufError::new(
                "REGISTRY_INVALID",
                format!("{root_sha256:?} is not a SHA-256 in hex"),
                serde_json::json!({ "reason": "rootSha256" }),
            ));
        }
        Ok(TrustStore {
            dir: trust_base.join(&sha),
            root_sha256: sha,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn root_sha256(&self) -> &str {
        &self.root_sha256
    }

    /// Whether this root has trusted metadata (an initialised store).
    pub fn exists(&self) -> bool {
        self.latest_root().ok().flatten().is_some()
    }

    /// Deletes every trusted file of this root (`registry forget`).
    pub fn forget(&self) -> Result<(), TufError> {
        match fs::remove_dir_all(&self.dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(TufError::io("remove", &self.dir, e)),
        }
    }

    fn mkdir(&self, p: &Path) -> Result<(), TufError> {
        fs::create_dir_all(p).map_err(|e| TufError::io("create", p, e))
    }

    /// The newest trusted root: its version and fetched bytes.
    pub(crate) fn latest_root(&self) -> Result<Option<(u64, Vec<u8>)>, TufError> {
        let dir = self.dir.join(ROOTS);
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(TufError::io("list", &dir, e)),
        };
        let mut best = None;
        for e in entries {
            let e = e.map_err(|e| TufError::io("list", &dir, e))?;
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(n) = name
                .strip_suffix(".root.json")
                .and_then(|v| v.parse::<u64>().ok())
            {
                best = best.max(Some(n));
            }
        }
        match best {
            None => Ok(None),
            Some(n) => Ok(self.root(n)?.map(|b| (n, b))),
        }
    }

    pub(crate) fn root(&self, version: u64) -> Result<Option<Vec<u8>>, TufError> {
        read_opt(&self.dir.join(ROOTS).join(format!("{version}.root.json")))
    }

    /// Saves a newly trusted root, temp then rename, before the next one is fetched.
    pub(crate) fn save_root(&self, version: u64, bytes: &[u8]) -> Result<(), TufError> {
        let dir = self.dir.join(ROOTS);
        self.mkdir(&dir)?;
        let p = dir.join(format!("{version}.root.json"));
        atomic::write(&p, bytes).map_err(|e| TufError::io("write", &p, e))
    }

    fn current_gen(&self) -> Result<Option<u64>, TufError> {
        let p = self.dir.join(CURRENT);
        match read_opt(&p)? {
            None => Ok(None),
            Some(b) => String::from_utf8_lossy(&b)
                .trim()
                .parse::<u64>()
                .map(Some)
                .map_err(|_| {
                    TufError::invalid("trustStore", format!("{} is corrupt", p.display()))
                }),
        }
    }

    fn gen_dir(&self, k: u64) -> PathBuf {
        self.dir.join(format!("gen-{k}"))
    }

    /// The trusted generation: file name (`timestamp.json`, `roles/<role>.json`, …) → bytes.
    pub(crate) fn current(&self) -> Result<BTreeMap<String, Vec<u8>>, TufError> {
        let mut out = BTreeMap::new();
        let Some(k) = self.current_gen()? else {
            return Ok(out);
        };
        let dir = self.gen_dir(k);
        for name in ["timestamp.json", "snapshot.json", "targets.json"] {
            if let Some(b) = read_opt(&dir.join(name))? {
                out.insert(name.to_string(), b);
            }
        }
        let roles = dir.join("roles");
        if let Ok(entries) = fs::read_dir(&roles) {
            for e in entries {
                let e = e.map_err(|e| TufError::io("list", &roles, e))?;
                let name = e.file_name().to_string_lossy().into_owned();
                let b = fs::read(e.path()).map_err(|e| TufError::io("read", &roles, e))?;
                out.insert(format!("roles/{name}"), b);
            }
        }
        Ok(out)
    }

    /// Makes `files` the trusted generation: written in full under a new directory, then
    /// `current` is replaced, then older generations are deleted.
    pub(crate) fn commit(&self, files: &BTreeMap<String, Vec<u8>>) -> Result<(), TufError> {
        let next = self.current_gen()?.map_or(1, |k| k + 1);
        let tmp = self.scratch("gen")?;
        self.mkdir(&tmp.join("roles"))?;
        for (name, bytes) in files {
            let p = tmp.join(name);
            atomic::write(&p, bytes).map_err(|e| TufError::io("write", &p, e))?;
        }
        let dest = self.gen_dir(next);
        fs::rename(&tmp, &dest).map_err(|e| TufError::io("rename", &dest, e))?;
        atomic::sync_dir(&self.dir).map_err(|e| TufError::io("sync", &self.dir, e))?;
        let cur = self.dir.join(CURRENT);
        atomic::write(&cur, next.to_string().as_bytes())
            .map_err(|e| TufError::io("write", &cur, e))?;
        self.drop_old_generations(next);
        Ok(())
    }

    fn drop_old_generations(&self, keep: u64) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(k) = name
                .strip_prefix("gen-")
                .and_then(|k| k.parse::<u64>().ok())
                && k != keep
            {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }

    pub(crate) fn state(&self) -> Result<TrustState, TufError> {
        let p = self.dir.join(STATE);
        match read_opt(&p)? {
            None => Ok(TrustState::default()),
            Some(b) => serde_json::from_slice(&b).map_err(|_| {
                TufError::invalid("trustStore", format!("{} is corrupt", p.display()))
            }),
        }
    }

    pub(crate) fn save_state(&self, s: &TrustState) -> Result<(), TufError> {
        self.mkdir(&self.dir)?;
        let p = self.dir.join(STATE);
        let bytes = serde_json::to_vec(s).unwrap_or_default();
        atomic::write(&p, &bytes).map_err(|e| TufError::io("write", &p, e))
    }

    /// A fresh, empty directory under `tmp/`.
    pub(crate) fn scratch(&self, what: &str) -> Result<PathBuf, TufError> {
        let base = self.dir.join(TMP);
        self.mkdir(&base)?;
        let p = atomic::temp_sibling(&base.join(what));
        self.mkdir(&p)?;
        Ok(p)
    }

    /// Removes everything under `tmp/` (left by a crash or a failed refresh).
    pub(crate) fn clear_scratch(&self) {
        let _ = fs::remove_dir_all(self.dir.join(TMP));
    }
}
