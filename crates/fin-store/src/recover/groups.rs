//! Recover groups (plan 4B Decision 27) and the recovered marks (Decision 5), both derived from
//! the current quarantine rows and the log alone, so a follower and a rebuild derive the same.
//!
//! - **Marks.** A row is recovered while a record in `txs` stands for it: a record that names
//!   its `txHash` as `recovers` (`record_links.recovers`; only a recovery writes it, never an
//!   ordinary cherry-pick, whose `originHash` names its root). A row with ops is stood for by
//!   its `cherryPick` copy; a `checkpoint` row by a fresh `checkpoint`, a `branch` row by the
//!   `branch` record of its recovered branch (its child group's, or the empty branch made
//!   again), a `branchSet` row of a live branch by a fresh `branchSet`, each carrying
//!   `recovers`. A `branchSet` row of a branch that is set aside with it (a `branch` row here
//!   makes it) is recovered with that `branch` row. Rows dismissed on this server (`meta`
//!   `recoverDismissed`, [`super::dismiss`]) are neither listed nor counted.
//! - **Groups** are made of the rows not recovered. Each is a run of `parentHash` links on one
//!   branch. A row with two children on its branch ends its group, and so does a row whose
//!   child is on another branch (a child branch forking there): each child starts a child
//!   group, forked at that row. A root group forks where its first record's parent is: a
//!   record in `txs`, the start (no parent), or, through rows already recovered, the record
//!   that stands for the nearest one with ops. A group whose first parent is none of these has
//!   no common history. A group whose rows are on the branch a `branch` row made, forking
//!   where that record says, is that row's (`made_by`): its branch stands for it; a root one
//!   is listed as the row's group's child too.

use std::collections::{BTreeMap, HashMap, HashSet};

use fin_core::{Record, RecordKind};
use rusqlite::{Connection, OptionalExtension, params};

use crate::StoreError;
use crate::db::{parse_record, to_u64};

/// One current quarantine row. `record`: `None` when the bytes are no readable record.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    pub hash: String,
    pub branch: String,
    pub origin: String,
    pub reason: String,
    pub detail: String,
    pub record: Option<Record>,
}

impl Row {
    fn parent(&self) -> Option<&str> {
        self.record.as_ref().and_then(|r| r.parent_hash.as_deref())
    }
}

/// One group: its rows in chain order (indices into [`Forest::rows`]); `parent`: the group and
/// row position it forks at, when that row is in quarantine too; `children`: the groups
/// forking at its rows, and the groups listed under it for its `branch` rows (`under`).
#[derive(Debug, Clone, Default)]
pub(crate) struct Group {
    pub rows: Vec<usize>,
    pub parent: Option<(usize, usize)>,
    pub children: Vec<usize>,
    pub under: Option<usize>,
    /// The `branch` row whose branch this group's records are on.
    pub made_by: Option<usize>,
}

/// The current quarantine rows not dismissed, linked by `parentHash`.
pub(crate) struct Forest {
    pub rows: Vec<Row>,
    by_hash: HashMap<String, usize>,
    by_tx: HashMap<String, usize>,
    kids: HashMap<String, Vec<usize>>,
    /// `txHash`es a record in `txs` stands for (`recovers`).
    stood: HashSet<String>,
}

/// The `txHash`es of current quarantine rows a record in `txs` names as `recovers`.
fn stood(conn: &Connection) -> Result<HashSet<String>, StoreError> {
    let mut stmt = conn.prepare_cached(
        "SELECT DISTINCT q.tx_hash FROM quarantine q
         JOIN record_links l ON l.recovers = q.tx_hash
         WHERE q.current = 1",
    )?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Forest {
    pub(crate) fn load(conn: &Connection) -> Result<Forest, StoreError> {
        let dismissed = super::dismiss::load(conn)?;
        let mut stmt = conn.prepare_cached(
            "SELECT tx_hash, branch, origin, reason, detail, record_bytes FROM quarantine
             WHERE current = 1 ORDER BY id",
        )?;
        let rows: Vec<Row> = stmt
            .query_map([], |r| {
                let bytes: Vec<u8> = r.get(5)?;
                Ok(Row {
                    hash: r.get(0)?,
                    branch: r.get(1)?,
                    origin: r.get(2)?,
                    reason: r.get(3)?,
                    detail: r.get(4)?,
                    record: parse_record(None, &bytes).ok(),
                })
            })?
            .collect::<Result<_, _>>()?;
        let rows: Vec<Row> = rows
            .into_iter()
            .filter(|r| !dismissed.contains(&r.hash))
            .collect();
        let mut by_hash = HashMap::new();
        let mut by_tx = HashMap::new();
        let mut kids: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            by_hash.insert(row.hash.clone(), i);
            if let Some(r) = &row.record {
                by_tx.entry(r.tx_id.clone()).or_insert(i);
            }
            if let Some(p) = row.parent() {
                kids.entry(p.to_string()).or_default().push(i);
            }
        }
        for v in kids.values_mut() {
            v.sort_by_key(|&i| (rows[i].record.as_ref().map(|r| r.seq), rows[i].hash.clone()));
        }
        Ok(Forest {
            rows,
            by_hash,
            by_tx,
            kids,
            stood: stood(conn)?,
        })
    }

    /// The row holding the record with `tx_id`.
    pub(crate) fn by_tx(&self, tx_id: &str) -> Option<usize> {
        self.by_tx.get(tx_id).copied()
    }

    /// The `branch` rows that make branch `id`.
    pub(crate) fn makers(&self, id: &str) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|&j| {
                self.rows[j].record.as_ref().is_some_and(|r| {
                    r.kind == RecordKind::Branch && r.branch_id.as_deref() == Some(id)
                })
            })
            .collect()
    }

    fn kids_of(&self, i: usize) -> &[usize] {
        self.kids.get(&self.rows[i].hash).map_or(&[], Vec::as_slice)
    }

    /// The rows recovered (module docs), by index.
    pub(crate) fn recovered(&self) -> HashSet<usize> {
        let mut out: HashSet<usize> = (0..self.rows.len())
            .filter(|&i| self.stood.contains(&self.rows[i].hash))
            .collect();
        for (i, row) in self.rows.iter().enumerate() {
            if let Some(r) = &row.record
                && r.kind == RecordKind::BranchSet
                && let Some(b) = &r.branch_id
                && self
                    .makers(b)
                    .iter()
                    .any(|j| self.stood.contains(&self.rows[*j].hash))
            {
                out.insert(i);
            }
        }
        out
    }

    /// The groups of the rows not recovered, and the order they are listed in (parents before
    /// their children).
    pub(crate) fn groups(&self) -> (Vec<Group>, Vec<usize>) {
        let done = self.recovered();
        let open = |i: usize| !done.contains(&i);
        let mut groups: Vec<Group> = vec![];
        let mut roots = vec![];
        for i in (0..self.rows.len()).filter(|&i| open(i)) {
            let row = &self.rows[i];
            let in_chain = row.record.is_some()
                && row
                    .parent()
                    .and_then(|p| self.by_hash.get(p))
                    .is_some_and(|&p| open(p) && self.rows[p].record.is_some());
            if !in_chain {
                roots.push(self.build(i, None, &open, &mut groups));
            }
        }
        // Groups made by a `branch` row: stood for by its branch; a root one is listed under
        // the row's group.
        let row_group: HashMap<usize, usize> = groups
            .iter()
            .enumerate()
            .flat_map(|(g, gr)| gr.rows.iter().map(move |&i| (i, g)))
            .collect();
        for g in 0..groups.len() {
            let first = &self.rows[groups[g].rows[0]];
            let Some(rec) = &first.record else { continue };
            let maker = self.makers(&first.branch).into_iter().find(|&j| {
                self.rows[j].record.as_ref().map(|r| &r.fork_hash) == Some(&rec.parent_hash)
            });
            groups[g].made_by = maker;
            if let Some(p) = maker.and_then(|j| row_group.get(&j).copied())
                && groups[g].parent.is_none()
                && p != g
            {
                groups[g].under = Some(p);
                groups[p].children.push(g);
            }
        }
        let mut order = vec![];
        let mut stack: Vec<usize> = roots
            .iter()
            .copied()
            .filter(|&g| groups[g].under.is_none())
            .rev()
            .collect();
        let mut seen = HashSet::new();
        while let Some(g) = stack.pop() {
            if !seen.insert(g) {
                continue;
            }
            order.push(g);
            stack.extend(groups[g].children.iter().rev().copied());
        }
        // A loop of `under` links (never made by real records) still lists every group.
        order.extend((0..groups.len()).filter(|g| !seen.contains(g)));
        (groups, order)
    }

    /// Builds the group starting at row `i` and, recursively, the groups forking from it.
    fn build(
        &self,
        i: usize,
        parent: Option<(usize, usize)>,
        open: &dyn Fn(usize) -> bool,
        groups: &mut Vec<Group>,
    ) -> usize {
        let gi = groups.len();
        groups.push(Group {
            parent,
            ..Group::default()
        });
        let mut rows = vec![i];
        let mut forks = vec![];
        let mut cur = i;
        while self.rows[cur].record.is_some() {
            let kids: Vec<usize> = self
                .kids_of(cur)
                .iter()
                .copied()
                .filter(|&k| open(k) && self.rows[k].record.is_some())
                .collect();
            let same: Vec<usize> = kids
                .iter()
                .copied()
                .filter(|&k| self.rows[k].branch == self.rows[cur].branch)
                .collect();
            let next = (same.len() == 1).then(|| same[0]);
            forks.extend(
                kids.into_iter()
                    .filter(|&k| Some(k) != next)
                    .map(|k| (k, rows.len() - 1)),
            );
            match next {
                Some(k) => {
                    rows.push(k);
                    cur = k;
                }
                None => break,
            }
        }
        groups[gi].rows = rows;
        for (k, pos) in forks {
            let c = self.build(k, Some((gi, pos)), open, groups);
            groups[gi].children.push(c);
        }
        gi
    }
}

/// Where a root group forks, through rows already recovered (module docs).
#[derive(Debug, Clone)]
pub(crate) enum Fork {
    At {
        /// The branch and seq the recovered branch forks from.
        branch: String,
        seq: u64,
        /// The record in `txs` the chain leaves the log at (`seq` 0 on `main` at the
        /// start): the lineage up to it is the history the group's ids are judged against.
        root: (String, u64),
        /// The recovered rows above the fork and their copies' bindings, nearest first.
        learn: Vec<(Record, BTreeMap<String, String>)>,
    },
    NoCommonHistory,
}

/// `(branch, seq)` of the `txs` row holding `hash`.
pub(super) fn txs_at(conn: &Connection, hash: &str) -> Result<Option<(String, u64)>, StoreError> {
    let row: Option<(String, i64)> = conn
        .prepare_cached("SELECT branch, seq FROM txs WHERE tx_hash = ?1 ORDER BY branch LIMIT 1")?
        .query_row(params![hash], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    row.map(|(b, s)| Ok((b, to_u64(s)?))).transpose()
}

/// A copy's branch, seq and bindings.
type Copy = (String, u64, BTreeMap<String, String>);

/// The record standing for quarantined record `hash`.
pub(super) fn copy_of(conn: &Connection, hash: &str) -> Result<Option<Copy>, StoreError> {
    let row: Option<(String, i64, Vec<u8>)> = conn
        .prepare_cached(
            "SELECT t.branch, t.seq, t.record_bytes FROM record_links l
             JOIN txs t ON t.tx_id = l.tx_id
             WHERE l.recovers = ?1 ORDER BY t.branch, t.seq LIMIT 1",
        )?
        .query_row(params![hash], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?;
    let Some((b, s, bytes)) = row else {
        return Ok(None);
    };
    let seq = to_u64(s)?;
    Ok(Some((b, seq, parse_record(Some(seq), &bytes)?.bindings)))
}

/// Any quarantine row's record by `txHash` (current or not).
pub(super) fn quarantined(conn: &Connection, hash: &str) -> Result<Option<Record>, StoreError> {
    let bytes: Option<Vec<u8>> = conn
        .prepare_cached("SELECT record_bytes FROM quarantine WHERE tx_hash = ?1")?
        .query_row(params![hash], |r| r.get(0))
        .optional()?;
    Ok(bytes.and_then(|b| parse_record(None, &b).ok()))
}

/// Where a root group whose first record's parent is `parent` forks (module docs).
pub(crate) fn resolve_fork(conn: &Connection, parent: Option<&str>) -> Result<Fork, StoreError> {
    let mut at: Option<(String, u64)> = None;
    let mut learn = vec![];
    let mut h = parent.map(str::to_string);
    loop {
        let Some(hash) = h else {
            let root = (crate::MAIN.to_string(), 0);
            let (branch, seq) = at.unwrap_or_else(|| root.clone());
            return Ok(Fork::At {
                branch,
                seq,
                root,
                learn,
            });
        };
        if let Some(root) = txs_at(conn, &hash)? {
            let (branch, seq) = at.unwrap_or_else(|| root.clone());
            return Ok(Fork::At {
                branch,
                seq,
                root,
                learn,
            });
        }
        let Some(rec) = quarantined(conn, &hash)? else {
            return Ok(Fork::NoCommonHistory);
        };
        // Copies and checkpoints stand in their row's place on the recovered branch; the
        // records standing for `branch` and `branchSet` rows live on other branches.
        let in_place = rec.kind.has_ops() || rec.kind == RecordKind::Checkpoint;
        match copy_of(conn, &hash)? {
            Some((b, s, bindings)) if in_place => {
                at.get_or_insert((b, s));
                if rec.kind.has_ops() {
                    learn.push((rec.clone(), bindings));
                }
            }
            // A record without ops changes no state: the fork moves above it.
            _ if !rec.kind.has_ops() => {}
            _ => return Ok(Fork::NoCommonHistory),
        }
        h = rec.parent_hash.clone();
    }
}
