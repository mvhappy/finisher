//! Key rotation (spec §6.5) and timestamp re-signing.
//!
//! - root: a new root version signed by the old and the new root thresholds;
//! - targets, snapshot, timestamp: a new root version (signed by the root keys) naming the
//!   new keys, then the rotated role and everything below it re-signed;
//! - a publisher: re-delegation in `targets`, and the publisher's role re-signed by the new
//!   keys at once, so the registry never lists a role its delegation does not verify.

use std::collections::{BTreeMap, HashSet};

use tough::schema::RoleType;

use super::state::{Change, ids, role_keys};
use super::{EditorError, RepoBuilder, SigningKey};

/// A role whose keys rotate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Root,
    Targets,
    Snapshot,
    Timestamp,
    Publisher(String),
}

impl Role {
    pub fn name(&self) -> &str {
        match self {
            Role::Root => "root",
            Role::Targets => "targets",
            Role::Snapshot => "snapshot",
            Role::Timestamp => "timestamp",
            Role::Publisher(p) => p,
        }
    }

    /// `root`, `targets`, `snapshot`, `timestamp`, or a publisher's name.
    pub fn parse(s: &str) -> Role {
        match s {
            "root" => Role::Root,
            "targets" => Role::Targets,
            "snapshot" => Role::Snapshot,
            "timestamp" => Role::Timestamp,
            p => Role::Publisher(p.to_string()),
        }
    }

    fn top_level(&self) -> Option<RoleType> {
        match self {
            Role::Root => Some(RoleType::Root),
            Role::Targets => Some(RoleType::Targets),
            Role::Snapshot => Some(RoleType::Snapshot),
            Role::Timestamp => Some(RoleType::Timestamp),
            Role::Publisher(_) => None,
        }
    }
}

impl RepoBuilder {
    /// Replaces `role`'s keys with `new` (threshold `threshold`) and re-signs what that
    /// needs. New top-level keys are added to the key ring; publisher keys are not kept.
    pub fn rotate(
        &mut self,
        role: Role,
        new: &[SigningKey],
        threshold: u32,
    ) -> Result<(), EditorError> {
        let mut new_ids: Vec<String> = new.iter().map(|k| k.public().key_id()).collect();
        new_ids.sort();
        new_ids.dedup();
        if threshold == 0 || threshold as usize > new_ids.len() {
            return Err(EditorError::key(format!(
                "a threshold of {threshold} needs at least that many distinct keys"
            )));
        }
        let Some(rt) = role.top_level() else {
            return self.rotate_publisher(role.name(), new, &new_ids, threshold);
        };
        for k in new {
            self.keys.add(k)?;
        }
        let mut root = self.st.root.signed.clone();
        root.version = self.bump("root", root.version.get());
        root.roles
            .insert(rt, role_keys(new_ids, u64::from(threshold)));
        for k in new {
            let p = k.public();
            root.keys
                .insert(hex::decode(p.key_id()).expect("hex").into(), p.tuf_key());
        }
        let used: HashSet<String> = root.roles.values().flat_map(ids).collect();
        root.keys
            .retain(|id, _| used.contains(&hex::encode(id.as_ref())));
        self.commit(Change {
            root: Some(root),
            targets: rt == RoleType::Targets,
            snapshot: rt == RoleType::Snapshot,
            ..Change::default()
        })
    }

    fn rotate_publisher(
        &mut self,
        publisher: &str,
        new: &[SigningKey],
        new_ids: &[String],
        threshold: u32,
    ) -> Result<(), EditorError> {
        if let Some(pending) = self.st.pending.get_mut(publisher) {
            if new.len() != 1 || threshold != 1 {
                return Err(EditorError::key(
                    "a publisher not yet published rotates to exactly one key",
                ));
            }
            *pending = new[0].public();
            return self.save_pending();
        }
        let d = self
            .st
            .targets
            .signed
            .delegations
            .as_mut()
            .and_then(|d| d.roles.iter().any(|r| r.name == publisher).then_some(d))
            .ok_or_else(|| {
                EditorError::new(
                    "PUBLISHER_NOT_DELEGATED",
                    format!("{publisher} is not delegated"),
                    serde_json::json!({ "publisher": publisher }),
                )
            })?;
        let rk = role_keys(new_ids.to_vec(), u64::from(threshold));
        for r in d.roles.iter_mut().filter(|r| r.name == publisher) {
            r.keyids = rk.keyids.clone();
            r.threshold = rk.threshold;
        }
        for k in new {
            let p = k.public();
            d.keys
                .insert(hex::decode(p.key_id()).expect("hex").into(), p.tuf_key());
        }
        let used: HashSet<String> = d
            .roles
            .iter()
            .flat_map(|r| r.keyids.iter().map(|k| hex::encode(k.as_ref())))
            .collect();
        d.keys
            .retain(|id, _| used.contains(&hex::encode(id.as_ref())));
        self.commit(Change {
            targets: true,
            roles: BTreeMap::from([(publisher.to_string(), new.to_vec())]),
            ..Change::default()
        })
    }

    /// Re-signs the timestamp alone, expiring in `expires_in_days` days (a registry whose
    /// content did not change stays fresh).
    pub fn resign_timestamp(&mut self, expires_in_days: u32) -> Result<(), EditorError> {
        self.commit(Change {
            timestamp_days: Some(expires_in_days),
            ..Change::default()
        })
    }
}
