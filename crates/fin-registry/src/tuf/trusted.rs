//! [`TrustedRepo`]: the TUF 1.0 client workflow (spec §6.2) over `tough`, per root.
//!
//! A refresh gives `tough` a scratch datastore seeded with copies of the trusted timestamp
//! and snapshot (never `latest_known_time.json`: the clock is ours), the newest trusted root
//! as its anchor, and the [`Adapter`] as its transport; `tough` always loads with expiry
//! enforcement off. Only after `load()` succeeds and our own checks pass is the fetched set
//! promoted, as one generation of the [`TrustStore`]. A failure leaves the trusted set as it
//! was (roots verified before the failure stay saved, as TUF 1.0 §5.3.8 requires).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tough::schema::{Root, Signed, Snapshot, Targets, Timestamp};
use tough::{ExpirationEnforcement, Limits, RepositoryLoader};
use url::Url;

use super::adapter::{self, Adapter, State};
use super::checks;
use super::clock::{self, unix_secs};
use super::custom::{self, PackageCustom};
use super::error::{from_tough, verify_error};
use super::freshness;
use super::{Clock, Policy, RefreshReport, TrustState, TrustStore, TufError, Warning};
use crate::atomic;
use crate::package::hex;
use crate::transport::{Transport, TransportError};

/// A target of the trusted metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetInfo {
    /// The target name, e.g. `acme/pack/1.0.0.finpkg`.
    pub name: String,
    /// The role that signs it: a publisher, or `targets` for registry-wide files.
    pub role: String,
    pub length: u64,
    /// Lowercase hex SHA-256.
    pub sha256: String,
    pub custom: Map<String, Value>,
}

impl TargetInfo {
    /// The package custom metadata, when this target is a package carrying valid metadata.
    pub fn package(&self) -> Option<PackageCustom> {
        if !self.name.ends_with(".finpkg") {
            return None;
        }
        PackageCustom::from_map(&self.custom)
    }

    /// The path of the target's file under `targets/`.
    pub fn file_path(&self) -> String {
        format!(
            "targets/{}",
            custom::consistent_target_path(&self.name, &self.sha256)
        )
    }
}

/// The trusted metadata of a root, parsed.
#[derive(Debug, Clone)]
pub(crate) struct Trusted {
    pub root: Signed<Root>,
    pub timestamp: Option<Signed<Timestamp>>,
    pub snapshot: Option<Signed<Snapshot>>,
    pub targets: Option<Signed<Targets>>,
    pub roles: BTreeMap<String, Signed<Targets>>,
}

fn parse<T: serde::de::DeserializeOwned>(name: &str, b: &[u8]) -> Result<T, TufError> {
    serde_json::from_slice(b).map_err(|e| {
        TufError::new(
            "REGISTRY_INVALID",
            format!("{name}: {e}"),
            json!({ "reason": "parse", "role": name.trim_end_matches(".json") }),
        )
    })
}

impl Trusted {
    fn load(store: &TrustStore) -> Result<Trusted, TufError> {
        let (_, root) = store.latest_root()?.ok_or_else(|| {
            TufError::invalid("trustStore", "the trust store holds no root".to_string())
        })?;
        Trusted::from_files(parse("root.json", &root)?, &store.current()?)
    }
}

/// A TUF repository trusted through one root (spec §6.2).
pub struct TrustedRepo {
    store: TrustStore,
    trusted: Trusted,
    rt: tokio::runtime::Runtime,
}

impl std::fmt::Debug for TrustedRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrustedRepo")
            .field("root", &self.store.root_sha256())
            .finish_non_exhaustive()
    }
}

fn runtime() -> Result<tokio::runtime::Runtime, TufError> {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .map_err(|e| TufError::invalid("runtime", e.to_string()))
}

/// Lowercase hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

impl TrustedRepo {
    /// Starts trusting `root` (the bytes of a root metadata file) if its SHA-256 is
    /// `expect_sha256` (`REGISTRY_UNTRUSTED` otherwise). A store that already trusts this
    /// root is opened as it is, so re-adding a registry resumes from its trusted versions.
    pub fn init(store: TrustStore, root: &[u8], expect_sha256: &str) -> Result<Self, TufError> {
        let got = sha256_hex(root);
        let expect = expect_sha256.to_ascii_lowercase();
        if got != expect || store.root_sha256() != expect {
            return Err(TufError::new(
                "REGISTRY_UNTRUSTED",
                format!("the root's SHA-256 is {got}, not the pinned {expect}"),
                json!({ "expected": expect, "got": got }),
            ));
        }
        if store.exists() {
            return TrustedRepo::open(store);
        }
        let parsed: Signed<Root> = parse("root.json", root)?;
        parsed
            .signed
            .verify_role(&parsed)
            .map_err(|e| verify_error("root", &e))?;
        checks::root(&parsed)?;
        store.save_root(parsed.signed.version.get(), root)?;
        TrustedRepo::open(store)
    }

    /// Opens a store that [`TrustedRepo::init`] created.
    pub fn open(store: TrustStore) -> Result<Self, TufError> {
        store.clear_scratch();
        let trusted = Trusted::load(&store)?;
        Ok(TrustedRepo {
            store,
            trusted,
            rt: runtime()?,
        })
    }

    pub fn store(&self) -> &TrustStore {
        &self.store
    }

    pub fn root_version(&self) -> u64 {
        self.trusted.root.signed.version.get()
    }

    /// `custom.finisher.maxStaleDays` of the trusted root (default 7, at most 90).
    pub fn max_stale_days(&self) -> u16 {
        custom::max_stale_days(&self.trusted.root.signed)
    }

    /// Whole days (rounded up) the trusted metadata is past its earliest expiry at `clock`'s
    /// now, or `None` while it is fresh.
    pub fn stale_days(&self, clock: &dyn Clock) -> Option<u16> {
        freshness::stale_days(&self.trusted, clock.now())
    }

    /// The freshness of the trusted metadata under `policy` at `clock`'s now:
    /// `REGISTRY_STALE` as a warning, `TUF_EXPIRED` as an error.
    pub fn freshness(
        &self,
        policy: &Policy,
        clock: &dyn Clock,
    ) -> Result<Option<Warning>, TufError> {
        freshness::check(&self.trusted, policy, clock.now())
    }

    /// Unix seconds of the injected clock at the last successful refresh.
    pub fn last_refresh_at(&self) -> Result<Option<i64>, TufError> {
        Ok(self.store.state()?.last_refresh_at)
    }

    /// Trusted versions: `(root, timestamp, snapshot, targets)`.
    pub fn versions(&self) -> (u64, Option<u64>, Option<u64>, Option<u64>) {
        let t = &self.trusted;
        (
            self.root_version(),
            t.timestamp.as_ref().map(|s| s.signed.version.get()),
            t.snapshot.as_ref().map(|s| s.signed.version.get()),
            t.targets.as_ref().map(|s| s.signed.version.get()),
        )
    }

    /// Runs the client workflow against `t` and promotes the result.
    pub fn refresh(
        &mut self,
        t: Arc<dyn Transport>,
        policy: &Policy,
        clock: &dyn Clock,
    ) -> Result<RefreshReport, TufError> {
        let r = self.refresh_inner(t, policy, clock);
        self.store.clear_scratch();
        r
    }

    fn refresh_inner(
        &mut self,
        t: Arc<dyn Transport>,
        policy: &Policy,
        clock: &dyn Clock,
    ) -> Result<RefreshReport, TufError> {
        let now = clock.now();
        let last = self.store.state()?.last_refresh_at;
        clock::check(now, last)?;
        let before = self.versions();
        let (anchor_version, anchor) = self
            .store
            .latest_root()?
            .ok_or_else(|| TufError::invalid("trustStore", "no trusted root".to_string()))?;
        let ds = self.store.scratch("datastore")?;
        for name in ["timestamp.json", "snapshot.json"] {
            if let Some(b) = self.store.current()?.get(name) {
                let p = ds.join(name);
                std::fs::write(&p, b).map_err(|e| TufError::io("write", &p, e))?;
            }
        }
        let state = Arc::new(Mutex::new(State {
            saved_root: anchor_version,
            ..State::default()
        }));
        let adapter = Adapter {
            transport: t,
            store: self.store.clone(),
            state: state.clone(),
        };
        let base = Url::parse(adapter::BASE).expect("a valid base URL");
        let loader = RepositoryLoader::new(
            &anchor,
            base.join("metadata/").expect("join"),
            base.join("targets/").expect("join"),
        )
        .transport(adapter)
        .datastore(&ds)
        .expiration_enforcement(ExpirationEnforcement::Unsafe)
        .limits(Limits {
            max_root_size: adapter::MAX_ROOT,
            max_targets_size: adapter::MAX_TARGETS,
            max_timestamp_size: adapter::MAX_TIMESTAMP,
            max_snapshot_size: adapter::MAX_SNAPSHOT,
            max_root_updates: adapter::MAX_ROOT_UPDATES,
        });
        let loaded = self.rt.block_on(loader.load());
        let mut st = std::mem::take(&mut *state.lock().unwrap_or_else(|e| e.into_inner()));
        if let Some(e) = st.fatal.take() {
            return Err(e);
        }
        let repo = match loaded {
            Ok(r) => r,
            Err(e) => {
                return Err(match st.last_error.take() {
                    Some(precise) => precise,
                    None => name_role(from_tough(&e), st.last_role.as_deref()),
                });
            }
        };
        checks::root(repo.root())?;
        checks::delegations(repo.targets(), repo.snapshot())?;
        let files = promoted_files(&repo, &st.served)?;
        let fresh = Trusted::from_files(repo.root().clone(), &files)?;
        let mut report = RefreshReport {
            roots_added: st.roots_added,
            stale_days: freshness::stale_days(&fresh, now),
            ..RefreshReport::default()
        };
        report
            .warnings
            .extend(freshness::check(&fresh, policy, now)?);
        let changed = self.store.current()? != files;
        if changed {
            self.store.commit(&files)?;
        }
        self.trusted = fresh;
        self.store.save_state(&TrustState {
            last_refresh_at: Some(last.unwrap_or(i64::MIN).max(unix_secs(now))),
        })?;
        let after = self.versions();
        report.root_version = after.0;
        report.timestamp_version = after.1.unwrap_or(0);
        report.snapshot_version = after.2.unwrap_or(0);
        report.targets_version = after.3.unwrap_or(0);
        report.changed = changed || after != before;
        Ok(report)
    }

    /// Every target of the trusted metadata: delegated roles in name order, then the
    /// top-level role's.
    pub fn targets(&self) -> impl Iterator<Item = TargetInfo> + '_ {
        let roles = self.trusted.roles.iter().map(|(n, s)| (n.as_str(), s));
        let top = self.trusted.targets.iter().map(|s| ("targets", s));
        roles.chain(top).flat_map(|(role, s)| {
            let mut v: Vec<TargetInfo> = s
                .signed
                .targets
                .iter()
                .map(|(name, t)| TargetInfo {
                    name: name.raw().to_string(),
                    role: role.to_string(),
                    length: t.length,
                    sha256: hex::encode(t.hashes.sha256.as_ref()),
                    custom: t.custom.clone().into_iter().collect(),
                })
                .collect();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v
        })
    }

    /// The trusted target named `name`.
    pub fn target(&self, name: &str) -> Option<TargetInfo> {
        self.targets().find(|t| t.name == name)
    }

    /// Fetches target `name` with its signed length as the cap and checks every byte
    /// against its signed length and SHA-256.
    pub fn fetch_target_bytes(&self, t: &dyn Transport, name: &str) -> Result<Vec<u8>, TufError> {
        let info = self.target(name).ok_or_else(|| {
            TufError::new(
                "PLUGIN_NOT_FOUND",
                format!("no trusted target {name:?}"),
                json!({ "target": name }),
            )
        })?;
        let bytes = match t.fetch(&info.file_path(), info.length) {
            Ok(b) => b,
            Err(TransportError::TooLarge { .. }) => {
                return Err(TufError::target_mismatch(name, "length"));
            }
            Err(e) => return Err(TufError::transport(&e)),
        };
        if bytes.len() as u64 != info.length {
            return Err(TufError::target_mismatch(name, "length"));
        }
        if sha256_hex(&bytes) != info.sha256 {
            return Err(TufError::target_mismatch(name, "hash"));
        }
        Ok(bytes)
    }

    /// [`TrustedRepo::fetch_target_bytes`], written to `out` (temp then rename).
    pub fn fetch_target(&self, t: &dyn Transport, name: &str, out: &Path) -> Result<(), TufError> {
        let bytes = self.fetch_target_bytes(t, name)?;
        atomic::write(out, &bytes).map_err(|e| TufError::io("write", out, e))
    }
}

/// `tough` reports a delegated role's failures as `targets`; name the role it was loading.
fn name_role(mut e: TufError, last_role: Option<&str>) -> TufError {
    if let Some(role) = last_role
        && e.role() == Some("targets")
    {
        e.details.insert("role".into(), Value::from(role));
    }
    e
}

/// The trusted generation after a successful load: the bytes as served.
fn promoted_files(
    repo: &tough::Repository,
    served: &BTreeMap<String, Vec<u8>>,
) -> Result<BTreeMap<String, Vec<u8>>, TufError> {
    let get = |path: String| {
        served
            .get(&path)
            .cloned()
            .ok_or_else(|| TufError::invalid("tuf", format!("{path} was trusted but never served")))
    };
    let mut files = BTreeMap::new();
    files.insert(
        "timestamp.json".into(),
        get("metadata/timestamp.json".into())?,
    );
    let sv = repo.snapshot().signed.version.get();
    files.insert(
        "snapshot.json".into(),
        get(format!("metadata/{sv}.snapshot.json"))?,
    );
    let tv = repo.targets().signed.version.get();
    files.insert(
        "targets.json".into(),
        get(format!("metadata/{tv}.targets.json"))?,
    );
    if let Some(d) = &repo.targets().signed.delegations {
        for role in &d.roles {
            if role.targets.is_none() {
                continue;
            }
            let meta = &repo.snapshot().signed.meta[&format!("{}.json", role.name)];
            let bytes = get(format!("metadata/{}.{}.json", meta.version, role.name))?;
            files.insert(format!("roles/{}.json", role.name), bytes);
        }
    }
    Ok(files)
}

fn get<T: serde::de::DeserializeOwned>(
    files: &BTreeMap<String, Vec<u8>>,
    name: &str,
) -> Result<Option<T>, TufError> {
    files.get(name).map(|b| parse(name, b)).transpose()
}

impl Trusted {
    fn from_files(
        root: Signed<Root>,
        files: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Trusted, TufError> {
        let mut roles = BTreeMap::new();
        for (name, bytes) in files {
            if let Some(role) = name
                .strip_prefix("roles/")
                .and_then(|r| r.strip_suffix(".json"))
            {
                roles.insert(role.to_string(), parse(name, bytes)?);
            }
        }
        Ok(Trusted {
            root,
            timestamp: get(files, "timestamp.json")?,
            snapshot: get(files, "snapshot.json")?,
            targets: get(files, "targets.json")?,
            roles,
        })
    }
}
