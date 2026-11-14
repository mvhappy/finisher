//! Which record a recovered record copies, and the ids its copy binds (plan 4B Decisions 18,
//! 27, 34; review of Task 16).
//!
//! - **Origin root.** A recovered record is a `cherryPick` whose `origin` is the record's
//!   origin root: for a `merge` or `cherryPick` record, its `origin` followed through `txs`
//!   and the quarantine rows to a record without one; for any other record, itself. `from` is
//!   that root's branch, `originHash` the root's hash (as for any pick), and `recovers` the
//!   quarantined record's hash (its mark).
//!   So a later merge judges the copy by the same root as its source (Decision 34) and never
//!   brings the source again.
//! - **Ids.** When the root is in `txs`, the copy keeps the ids the record bound (they are the
//!   source's, Decision 18). Otherwise each id it bound gets a fresh one, shared by every
//!   copy of the same root (in this write, and with copies an earlier recovery made), so a
//!   set-aside record and a set-aside merge of it stay one entity on their recovered
//!   branches.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use fin_core::{IdAlloc, IdKind, Record, RecordKind, Tx};
use rusqlite::{Connection, OptionalExtension, params};

use super::groups::{Forest, quarantined};
use crate::StoreError;
use crate::db::{Counters, parse_record, to_u64};

/// Where a record's origin root is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Root {
    /// In `txs`, on `branch`: the copy keeps the record's ids.
    Live { tx_id: String, branch: String },
    /// Set aside (the record itself, or a quarantined source), last on `branch`.
    Aside { tx_id: String, branch: String },
    /// Neither: the chain ends at a txId this database does not hold.
    Unknown { tx_id: String },
}

impl Root {
    pub(crate) fn tx_id(&self) -> &str {
        match self {
            Root::Live { tx_id, .. } | Root::Aside { tx_id, .. } | Root::Unknown { tx_id } => tx_id,
        }
    }
}

/// The origin root of quarantined record `rec` (module docs).
pub(crate) fn root(conn: &Connection, forest: &Forest, rec: &Record) -> Result<Root, StoreError> {
    let copy = |r: &Record| matches!(r.kind, RecordKind::Merge | RecordKind::CherryPick);
    if !copy(rec) {
        return Ok(Root::Aside {
            tx_id: rec.tx_id.clone(),
            branch: rec.intent.branch.clone(),
        });
    }
    let mut id = rec.origin.clone().unwrap_or_else(|| rec.tx_id.clone());
    let mut branch = rec
        .from
        .clone()
        .unwrap_or_else(|| rec.intent.branch.clone());
    let mut seen = HashSet::new();
    while seen.insert(id.clone()) {
        let live: Option<(String, i64, Vec<u8>)> = conn
            .prepare_cached("SELECT branch, seq, record_bytes FROM txs WHERE tx_id = ?1")?
            .query_row(params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .optional()?;
        let (next, at) = if let Some((b, seq, bytes)) = live {
            let r = parse_record(Some(to_u64(seq)?), &bytes)?;
            if !copy(&r) {
                return Ok(Root::Live {
                    tx_id: id,
                    branch: b,
                });
            }
            (r.origin.clone(), r.from.clone())
        } else if let Some(r) = forest
            .by_tx(&id)
            .and_then(|i| forest.rows[i].record.as_ref())
        {
            if !copy(r) {
                return Ok(Root::Aside {
                    tx_id: id,
                    branch: r.intent.branch.clone(),
                });
            }
            (r.origin.clone(), r.from.clone())
        } else {
            return Ok(Root::Unknown { tx_id: id });
        };
        match next {
            Some(o) => {
                id = o;
                branch = at.unwrap_or(branch);
            }
            None => return Ok(Root::Aside { tx_id: id, branch }),
        }
    }
    Ok(Root::Unknown { tx_id: id })
}

/// The fresh ids given per origin root, old to new.
#[derive(Debug, Default)]
pub(crate) struct RootIds {
    map: HashMap<String, BTreeMap<String, String>>,
}

impl RootIds {
    /// The ids an earlier recovery gave copies of `root`: each copy against the quarantined
    /// record it stands for.
    fn seed(&mut self, conn: &Connection, root: &str) -> Result<(), StoreError> {
        if self.map.contains_key(root) {
            return Ok(());
        }
        let mut stmt = conn.prepare_cached(
            "SELECT l.recovers, t.seq, t.record_bytes FROM record_links l
             JOIN txs t ON t.tx_id = l.tx_id
             WHERE l.kind = 'cherryPick' AND l.origin = ?1 AND l.recovers IS NOT NULL",
        )?;
        let rows: Vec<(String, i64, Vec<u8>)> = stmt
            .query_map(params![root], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let mut m = BTreeMap::new();
        for (hash, seq, bytes) in rows {
            let Some(orig) = quarantined(conn, &hash)? else {
                continue;
            };
            let copy = parse_record(Some(to_u64(seq)?), &bytes)?;
            for (p, old) in &orig.bindings {
                if let Some(new) = copy.bindings.get(p) {
                    m.insert(old.clone(), new.clone());
                }
            }
        }
        self.map.insert(root.to_string(), m);
        Ok(())
    }

    /// The ids `tx` (prepared from `rec`) must bind, in allocation order: `Some` for a kept or
    /// already given id, `None` for a fresh one.
    pub(crate) fn preset(
        &mut self,
        conn: &Connection,
        root: &Root,
        rec: &Record,
        tx: &Tx,
    ) -> Result<VecDeque<Option<String>>, StoreError> {
        if let Root::Aside { tx_id, .. } | Root::Unknown { tx_id } = root {
            self.seed(conn, tx_id)?;
        }
        let given = self.map.get(root.tx_id());
        // As `BoundIds`: the first op naming a placeholder binds it.
        let mut seen = std::collections::BTreeSet::new();
        Ok(tx
            .ops
            .iter()
            .filter_map(|op| op.allocates())
            .filter(|p| seen.insert(*p))
            .map(|p| {
                let old = rec.bindings.get(p)?;
                match root {
                    Root::Live { .. } => Some(old.clone()),
                    _ => given.and_then(|m| m.get(old)).cloned(),
                }
            })
            .collect())
    }

    /// Learns the ids a copy of `root` bound for `rec`'s.
    pub(crate) fn learn(&mut self, root: &Root, rec: &Record, new: &BTreeMap<String, String>) {
        if matches!(root, Root::Live { .. }) {
            return;
        }
        let m = self.map.entry(root.tx_id().to_string()).or_default();
        for (p, old) in &rec.bindings {
            if let Some(n) = new.get(p) {
                m.insert(old.clone(), n.clone());
            }
        }
    }
}

/// Hands out the preset ids in allocation order, and fresh ones from the counters.
pub(crate) struct Preset<'a> {
    pub queue: VecDeque<Option<String>>,
    pub ids: &'a mut Counters,
}

impl IdAlloc for Preset<'_> {
    fn next(&mut self, kind: IdKind) -> String {
        match self.queue.pop_front() {
            Some(Some(id)) => id,
            _ => self.ids.next(kind),
        }
    }
}
