//! [`Ctx`]: the state every branch of one import pass shares.

use std::collections::{HashMap, HashSet};

use rusqlite::Connection;

use crate::StoreError;

/// Pass-wide state shared by the branches.
pub(super) struct Ctx<'a> {
    pub(super) txn: &'a Connection,
    pub(super) server_id: &'a str,
    pub(super) now: i64,
    /// Every DB record, `txId → txHash`, loaded on first use and kept current.
    all: Option<HashMap<String, String>>,
    hashes: HashSet<String>,
    /// `txs` rows were added or removed.
    pub(super) changed: bool,
    /// Folder snapshots a rebuild may seed from.
    pub(super) seeds: &'a [super::Seed],
    /// The seq a branch was seeded at, this pass.
    pub(super) seeded: Option<u64>,
    /// Branches an applied `branch` record made, not yet resolved this pass (the worklist).
    pub(super) born: Vec<String>,
    /// Every folder segment by branch directory, as this pass listed them (a cascade reads
    /// the segments of the branches it takes out, for the origin rule).
    pub(super) dirs: HashMap<String, Vec<crate::chain::SegRef>>,
    /// Branches an imported `branch` record took out (it won their id, [`super::branches`]),
    /// not yet reported by the worklist.
    pub(super) rewound: Vec<crate::Rewind>,
    /// A `branch` or `branchSet` record was folded this pass (its inert mark is derived
    /// again before the pass ends, `crate::derived::remark`).
    pub(super) folded: bool,
}

impl<'a> Ctx<'a> {
    pub(super) fn new(
        txn: &'a Connection,
        server_id: &'a str,
        now: i64,
        seeds: &'a [super::Seed],
    ) -> Self {
        Ctx {
            txn,
            server_id,
            now,
            all: None,
            hashes: HashSet::new(),
            changed: false,
            seeds,
            seeded: None,
            born: vec![],
            dirs: HashMap::new(),
            rewound: vec![],
            folded: false,
        }
    }

    fn load(&mut self) -> Result<&HashMap<String, String>, StoreError> {
        if self.all.is_none() {
            let mut stmt = self.txn.prepare("SELECT tx_id, tx_hash FROM txs")?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            let all: HashMap<String, String> = rows.collect::<Result<_, _>>()?;
            self.hashes = all.values().cloned().collect();
            self.all = Some(all);
        }
        Ok(self.all.as_ref().expect("loaded above"))
    }

    pub(super) fn all_tx(&mut self) -> Result<HashMap<String, String>, StoreError> {
        self.load().cloned()
    }

    /// Whether the DB holds a record with this `txHash`.
    pub(super) fn has_hash(&mut self, hash: &str) -> Result<bool, StoreError> {
        self.load()?;
        Ok(self.hashes.contains(hash))
    }

    pub(super) fn server_id(&self) -> &str {
        self.server_id
    }

    pub(super) fn added(&mut self, tx_id: &str, hash: &str) {
        if let Some(all) = &mut self.all {
            all.insert(tx_id.to_string(), hash.to_string());
            self.hashes.insert(hash.to_string());
        }
        self.changed = true;
    }

    pub(super) fn removed(&mut self, tx_id: &str, hash: &str) {
        if let Some(all) = &mut self.all {
            all.remove(tx_id);
            self.hashes.remove(hash);
        }
        self.changed = true;
    }
}
