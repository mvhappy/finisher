//! The editor's view of a registry (the newest signed roles, read back from `metadata/`),
//! `init`, and the commit pipeline every change goes through.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroU64;
use std::path::Path;
use std::sync::Arc;

use serde_json::json;
use sha2::{Digest, Sha256};
use tough::schema::{
    Delegations, Hashes, Metafile, RoleKeys, RoleType, Root, Signed, Snapshot, Targets, Timestamp,
};

use super::{EditorError, KeyRing, PublicKey, RepoBuilder, SigningKey, sign};
use crate::tuf::custom::root_custom;
use crate::tuf::{Clock, SystemClock};

pub(crate) const PENDING: &str = "delegations.json";

pub(crate) struct State {
    pub root: Signed<Root>,
    pub targets: Signed<Targets>,
    pub snapshot: Signed<Snapshot>,
    pub timestamp: Signed<Timestamp>,
    /// Publisher roles by name.
    pub roles: BTreeMap<String, Signed<Targets>>,
    /// Publishers delegated by `delegate` whose first package is not yet published: their
    /// delegation is written with their first role, because `tough` loads no delegated role
    /// at all when one of them has no file (8A Task 4a).
    pub pending: BTreeMap<String, PublicKey>,
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, EditorError> {
    let b = std::fs::read(path).map_err(|e| EditorError::io("read", path, e))?;
    serde_json::from_slice(&b)
        .map_err(|e| EditorError::invalid("parse", format!("{}: {e}", path.display())))
}

fn nz(v: u64) -> NonZeroU64 {
    NonZeroU64::new(v.max(1)).expect("at least 1")
}

pub(crate) fn metafile(bytes: &[u8], version: NonZeroU64) -> Metafile {
    Metafile {
        length: Some(bytes.len() as u64),
        hashes: Some(Hashes {
            sha256: Sha256::digest(bytes).to_vec().into(),
            _extra: HashMap::new(),
        }),
        version,
        _extra: HashMap::new(),
    }
}

pub(crate) fn role_keys(ids: Vec<String>, threshold: u64) -> RoleKeys {
    RoleKeys {
        keyids: ids
            .into_iter()
            .map(|id| hex::decode(id).expect("a key id is hex").into())
            .collect(),
        threshold: nz(threshold),
        _extra: HashMap::new(),
    }
}

/// The key ring must never be inside the folder that is published (or pushed).
pub(crate) fn refuse_keys_inside(dir: &Path, keys_dir: &Path) -> Result<(), EditorError> {
    if super::paths::keys_inside(dir, keys_dir) {
        return Err(EditorError::invalid(
            "keysInside",
            "the key ring must not be inside the registry folder: it would be published",
        ));
    }
    Ok(())
}

fn unsigned<T>(signed: T) -> Signed<T> {
    Signed {
        signed,
        signatures: Vec::new(),
    }
}

pub(crate) fn ids(keys: &RoleKeys) -> Vec<String> {
    keys.keyids
        .iter()
        .map(|k| hex::encode(k.as_ref()))
        .collect()
}

impl State {
    pub fn load(dir: &Path, keys: &KeyRing) -> Result<State, EditorError> {
        let meta = dir.join("metadata");
        let entries = std::fs::read_dir(&meta).map_err(|_| {
            EditorError::invalid(
                "notARegistry",
                format!("{} holds no registry; run `registry init`", dir.display()),
            )
        })?;
        let mut newest = 0;
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Some(v) = n.strip_suffix(".root.json").and_then(|v| v.parse().ok()) {
                newest = newest.max(v);
            }
        }
        let root: Signed<Root> = read(&meta.join(format!("{newest}.root.json")))?;
        let roles_ok = [
            RoleType::Root,
            RoleType::Targets,
            RoleType::Snapshot,
            RoleType::Timestamp,
        ]
        .iter()
        .all(|r| root.signed.roles.contains_key(r));
        if !roles_ok {
            return Err(EditorError::invalid(
                "layout",
                "the root does not name all four roles",
            ));
        }
        let missing = |what: &str| EditorError::invalid("layout", format!("no {what} is listed"));
        let timestamp: Signed<Timestamp> = read(&meta.join("timestamp.json"))?;
        let sv = timestamp
            .signed
            .meta
            .get("snapshot.json")
            .ok_or_else(|| missing("snapshot"))?
            .version;
        let snapshot: Signed<Snapshot> = read(&meta.join(format!("{sv}.snapshot.json")))?;
        let tv = snapshot
            .signed
            .meta
            .get("targets.json")
            .ok_or_else(|| missing("targets"))?
            .version;
        let targets: Signed<Targets> = read(&meta.join(format!("{tv}.targets.json")))?;
        let mut roles = BTreeMap::new();
        for (file, m) in &snapshot.signed.meta {
            if let Some(name) = file.strip_suffix(".json")
                && name != "targets"
            {
                let v = m.version;
                roles.insert(
                    name.to_string(),
                    read(&meta.join(format!("{v}.{name}.json")))?,
                );
            }
        }
        let mut pending = BTreeMap::new();
        let p = keys.dir().join(PENDING);
        if p.exists() {
            let raw: BTreeMap<String, String> = read(&p)?;
            for (publisher, pem) in raw {
                pending.insert(publisher, PublicKey::from_pem(&pem)?);
            }
        }
        Ok(State {
            root,
            targets,
            snapshot,
            timestamp,
            roles,
            pending,
        })
    }
}

/// What a commit re-signs.
#[derive(Default)]
pub(crate) struct Change {
    /// A new root (its version already set); signed by the old and the new root keys.
    pub root: Option<Root>,
    pub targets: bool,
    /// Publisher roles to re-sign, with their signers.
    pub roles: BTreeMap<String, Vec<SigningKey>>,
    pub snapshot: bool,
    /// Re-sign the timestamp alone, expiring in this many days.
    pub timestamp_days: Option<u32>,
}

impl RepoBuilder {
    /// Creates a registry in `dir` with `root_keys` (threshold `threshold`) and new targets,
    /// snapshot and timestamp keys, all kept in the key ring at `keys_dir`.
    pub fn init(
        dir: &Path,
        keys_dir: &Path,
        root_keys: &[SigningKey],
        threshold: u32,
        max_stale_days: Option<u16>,
    ) -> Result<RepoBuilder, EditorError> {
        RepoBuilder::init_with(
            dir,
            keys_dir,
            root_keys,
            threshold,
            max_stale_days,
            Arc::new(SystemClock),
        )
    }

    /// [`RepoBuilder::init`] signing with times from `clock`.
    pub fn init_with(
        dir: &Path,
        keys_dir: &Path,
        root_keys: &[SigningKey],
        threshold: u32,
        max_stale_days: Option<u16>,
        clock: Arc<dyn Clock>,
    ) -> Result<RepoBuilder, EditorError> {
        refuse_keys_inside(dir, keys_dir)?;
        let meta = dir.join("metadata");
        if std::fs::read_dir(&meta).is_ok_and(|mut d| d.next().is_some()) {
            return Err(EditorError::new(
                "REGISTRY_EXISTS",
                format!("{} already holds a registry", dir.display()),
                json!({}),
            ));
        }
        let mut root_ids: Vec<String> = root_keys.iter().map(|k| k.public().key_id()).collect();
        root_ids.sort();
        root_ids.dedup();
        if threshold == 0 || threshold as usize > root_ids.len() {
            return Err(EditorError::key(format!(
                "a root threshold of {threshold} needs at least that many distinct root keys"
            )));
        }
        let ring = KeyRing::new(keys_dir);
        let online: Vec<SigningKey> = (0..3).map(|_| SigningKey::generate()).collect();
        for k in root_keys.iter().chain(&online) {
            ring.add(k)?;
        }
        let mut keys = HashMap::new();
        for k in root_keys.iter().chain(&online) {
            let p = k.public();
            keys.insert(hex::decode(p.key_id()).expect("hex").into(), p.tuf_key());
        }
        let mut roles = HashMap::new();
        roles.insert(RoleType::Root, role_keys(root_ids, u64::from(threshold)));
        for (r, k) in [RoleType::Targets, RoleType::Snapshot, RoleType::Timestamp]
            .into_iter()
            .zip(&online)
        {
            roles.insert(r, role_keys(vec![k.public().key_id()], 1));
        }
        let mut extra = HashMap::new();
        if let Some(d) = max_stale_days {
            extra.insert("custom".to_string(), root_custom(d));
        }
        let placeholder = jiff::Timestamp::UNIX_EPOCH;
        let root = Root {
            spec_version: "1.0.0".into(),
            consistent_snapshot: true,
            version: nz(1),
            expires: placeholder,
            keys,
            roles,
            _extra: extra,
        };
        let mut targets = Targets::new("1.0.0".into(), nz(1), placeholder);
        targets.delegations = Some(Delegations::new());
        let mut b = RepoBuilder {
            dir: dir.to_path_buf(),
            keys: ring,
            clock,
            expiry: super::Expiry::default(),
            st: State {
                root: unsigned(root.clone()),
                targets: unsigned(targets),
                snapshot: unsigned(Snapshot::new("1.0.0".into(), nz(1), placeholder)),
                timestamp: unsigned(Timestamp::new("1.0.0".into(), nz(1), placeholder)),
                roles: BTreeMap::new(),
                pending: BTreeMap::new(),
            },
            next: BTreeMap::new(),
        };
        for r in ["targets", "snapshot", "timestamp"] {
            b.next.insert(r.into(), 1);
        }
        let mut root = root;
        root.expires = b.expires(b.expiry.root);
        let signed = sign(root, root_keys);
        b.write_metadata("1.root.json", &signed.bytes)?;
        b.st.root = signed.signed;
        b.commit(Change {
            targets: true,
            ..Change::default()
        })?;
        Ok(b)
    }

    pub(crate) fn save_pending(&self) -> Result<(), EditorError> {
        let raw: BTreeMap<&String, String> = self
            .st
            .pending
            .iter()
            .map(|(p, k)| (p, k.to_pem()))
            .collect();
        self.keys.create()?;
        let p = self.keys.dir().join(PENDING);
        let bytes = serde_json::to_vec_pretty(&raw).expect("a map serialises");
        crate::atomic::write(&p, &bytes).map_err(|e| EditorError::io("write", &p, e))
    }

    fn role_signers(&self, r: RoleType) -> Result<Vec<SigningKey>, EditorError> {
        let k = &self.st.root.signed.roles[&r];
        self.ring_signers(&r.to_string(), &ids(k), k.threshold.get())
    }

    fn current_meta(&self, name: &str, version: NonZeroU64) -> Result<Metafile, EditorError> {
        let p = self.metadata().join(format!("{version}.{name}.json"));
        let b = std::fs::read(&p).map_err(|e| EditorError::io("read", &p, e))?;
        Ok(metafile(&b, version))
    }

    /// Re-signs what `c` names, then the snapshot (when anything under it changed), then the
    /// timestamp, writing `timestamp.json` last.
    pub(crate) fn commit(&mut self, c: Change) -> Result<(), EditorError> {
        if let Some(mut root) = c.root {
            root.expires = self.expires(self.expiry.root);
            let old = &self.st.root.signed.roles[&RoleType::Root];
            let new = &root.roles[&RoleType::Root];
            let mut signers = self.ring_signers("root", &ids(old), old.threshold.get())?;
            signers.extend(self.ring_signers("root", &ids(new), new.threshold.get())?);
            let v = root.version;
            let s = sign(root, &signers);
            self.write_metadata(&format!("{v}.root.json"), &s.bytes)?;
            self.st.root = s.signed;
        }
        let under_snapshot = c.targets || !c.roles.is_empty() || c.snapshot;
        for (name, signers) in &c.roles {
            let cur = self.st.roles[name].signed.version.get();
            let v = self.bump(name, cur);
            let expires = self.expires(self.expiry.targets);
            let role = self
                .st
                .roles
                .get_mut(name)
                .expect("a role being re-signed exists");
            role.signed.version = v;
            role.signed.expires = expires;
            let s = sign(role.signed.clone(), signers);
            self.write_metadata(&format!("{v}.{name}.json"), &s.bytes)?;
            self.st.roles.insert(name.clone(), s.signed);
        }
        if c.targets {
            let cur = self.st.targets.signed.version.get();
            let v = self.bump("targets", cur);
            let mut t = self.st.targets.signed.clone();
            t.version = v;
            t.expires = self.expires(self.expiry.targets);
            let s = sign(t, &self.role_signers(RoleType::Targets)?);
            self.write_metadata(&format!("{v}.targets.json"), &s.bytes)?;
            self.st.targets = s.signed;
        }
        if under_snapshot {
            let mut meta = HashMap::new();
            let tv = self.st.targets.signed.version;
            meta.insert(
                "targets.json".to_string(),
                self.current_meta("targets", tv)?,
            );
            for (name, r) in &self.st.roles {
                meta.insert(
                    format!("{name}.json"),
                    self.current_meta(name, r.signed.version)?,
                );
            }
            let cur = self.st.snapshot.signed.version.get();
            let v = self.bump("snapshot", cur);
            let mut s = self.st.snapshot.signed.clone();
            s.version = v;
            s.expires = self.expires(self.expiry.snapshot);
            s.meta = meta;
            let s = sign(s, &self.role_signers(RoleType::Snapshot)?);
            self.write_metadata(&format!("{v}.snapshot.json"), &s.bytes)?;
            self.st.snapshot = s.signed;
        }
        let sv = self.st.snapshot.signed.version;
        let cur = self.st.timestamp.signed.version.get();
        let v = self.bump("timestamp", cur);
        let mut t = self.st.timestamp.signed.clone();
        t.version = v;
        t.expires = self.expires(c.timestamp_days.unwrap_or(self.expiry.timestamp));
        t.meta = HashMap::from([(
            "snapshot.json".to_string(),
            self.current_meta("snapshot", sv)?,
        )]);
        let s = sign(t, &self.role_signers(RoleType::Timestamp)?);
        self.write_metadata("timestamp.json", &s.bytes)?;
        self.st.timestamp = s.signed;
        Ok(())
    }
}
