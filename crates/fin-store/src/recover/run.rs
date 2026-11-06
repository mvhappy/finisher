//! Running recover groups (plan 4B Decision 27): one walk over the groups that either only
//! plans them, to list what each would give and why it cannot be created (`problems`), or
//! writes them inside one [`Store::write`](crate::Store::write).
//!
//! A group is re-run from its fork state record by record: a record with ops through
//! [`Remap::prepare`], then `apply` in `Internal` mode (full policy) with the store's id
//! counters, appended as `cherryPick { origin, from, originHash }`; a `checkpoint` again as a
//! fresh `checkpoint` record; a `branch` or `branchSet` record not at all (a `branch`
//! record's branch is a child group). The groups forking at its rows follow, from the state
//! and the remap at that row, on the recovered branch at its copy. A group without records
//! with ops creates no branch: its children fork where it does (its records change no
//! state).

use std::collections::{HashMap, HashSet};

use fin_core::rebase::reasons;
use fin_core::{
    ApplyMode, CommittedTx, Conflict, CoreError, Link, Project, Record, RecordKind, apply_tx_mode,
};
use rusqlite::Connection;

use super::groups::{Forest, Fork, Group, resolve_fork};
use super::remap::{Remap, entity_ids};
use crate::StoreError;
use crate::branch::create::{At, BranchCreate, metadata_draft, plan_create};
use crate::branch::lineage::{self, LineageSeg};
use crate::db::{Counters, parse_record, to_u64};
use crate::write::{Draft, WriteCtx};

/// Problem reasons besides the rebase ones.
pub(crate) const NO_COMMON_HISTORY: &str = "NO_COMMON_HISTORY";
pub(crate) const NOTHING_TO_RECOVER: &str = "NOTHING_TO_RECOVER";
pub(crate) const INVALID_FORMAT: &str = "INVALID_FORMAT";

/// Where and from what a group starts.
#[derive(Clone)]
struct Start {
    /// The branch and seq to fork from (written runs only; `None` while planning).
    at: Option<(String, u64)>,
    state: Project,
    remap: Remap,
    /// The record the chain leaves the log at (see [`Fork::At`]).
    root: (String, u64),
}

/// Whether a record off the lineage up to a root binds an id (memoised).
struct Bound<'a> {
    conn: &'a Connection,
    lineages: HashMap<String, Vec<LineageSeg>>,
    memo: HashMap<(String, u64, String), bool>,
}

impl Bound<'_> {
    /// A `txs` record that binds `id` and is not on `root`'s lineage at or below its seq: not
    /// counting a `merge` or `cherryPick` copy whose origin binds the same id (copies keep
    /// their source's ids, Decision 18).
    fn elsewhere(&mut self, root: &(String, u64), id: &str) -> Result<bool, StoreError> {
        let key = (root.0.clone(), root.1, id.to_string());
        if let Some(b) = self.memo.get(&key) {
            return Ok(*b);
        }
        if !self.lineages.contains_key(&root.0) {
            let segs = lineage::lineage(self.conn, &root.0)?;
            self.lineages.insert(root.0.clone(), segs);
        }
        let segs = &self.lineages[&root.0];
        let on_lineage = |branch: &str, seq: u64| {
            seq <= root.1
                && segs.iter().any(|s| {
                    s.branch == branch && seq > s.after && s.through.is_none_or(|t| seq <= t)
                })
        };
        let pattern = format!("\"{id}\"").into_bytes();
        let mut stmt = self.conn.prepare_cached(
            "SELECT branch, seq, record_bytes FROM txs WHERE instr(record_bytes, ?1) > 0",
        )?;
        let rows: Vec<(String, i64, Vec<u8>)> = stmt
            .query_map([pattern], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        let mut found = false;
        for (branch, seq, bytes) in rows {
            let seq = to_u64(seq)?;
            if on_lineage(&branch, seq) {
                continue;
            }
            let rec = parse_record(Some(seq), &bytes)?;
            if !rec.bindings.values().any(|v| v == id) {
                continue;
            }
            if matches!(rec.kind, RecordKind::Merge | RecordKind::CherryPick)
                && let Some(origin) = &rec.origin
                && origin_binds(self.conn, origin, id)?
            {
                continue;
            }
            found = true;
            break;
        }
        self.memo.insert(key, found);
        Ok(found)
    }
}

fn origin_binds(conn: &Connection, origin: &str, id: &str) -> Result<bool, StoreError> {
    Ok(crate::db::find_tx(conn, origin)?.is_some_and(|c| c.bindings.values().any(|v| v == id)))
}

/// The group walk (module docs). `ctx`: write the groups in `create`; `None`: plan only.
pub(crate) struct Runner<'a, 'c, 'w> {
    conn: &'a Connection,
    forest: &'a Forest,
    groups: &'a [Group],
    author: &'a str,
    ctx: Option<&'c mut WriteCtx<'w>>,
    /// The groups to write (with `ctx`), and their names.
    create: HashMap<usize, String>,
    /// The request's txId: the records are `<txId>-<n>`.
    key: &'a str,
    n: u64,
    ids: Counters,
    bound: Bound<'a>,
    pub problems: Vec<Vec<Conflict>>,
    /// The branch each written group made, in the order they were made.
    pub created: Vec<(usize, String)>,
    pub committed: Vec<CommittedTx>,
}

impl<'a, 'c, 'w> Runner<'a, 'c, 'w> {
    pub(crate) fn new(
        conn: &'a Connection,
        forest: &'a Forest,
        groups: &'a [Group],
        author: &'a str,
        key: &'a str,
    ) -> Result<Self, StoreError> {
        Ok(Runner {
            conn,
            forest,
            groups,
            author,
            ctx: None,
            create: HashMap::new(),
            key,
            n: 0,
            ids: Counters::load(conn)?,
            bound: Bound {
                conn,
                lineages: HashMap::new(),
                memo: HashMap::new(),
            },
            problems: vec![vec![]; groups.len()],
            created: vec![],
            committed: vec![],
        })
    }

    /// Writes the groups of `create` (with their names) through `ctx`.
    pub(crate) fn writing(
        mut self,
        ctx: &'c mut WriteCtx<'w>,
        create: HashMap<usize, String>,
    ) -> Self {
        self.ctx = Some(ctx);
        self.create = create;
        self
    }

    /// Runs every root group in `order` (and, from each, the groups forking at its rows).
    pub(crate) fn run(&mut self, order: &[usize]) -> Result<(), StoreError> {
        for &g in order {
            if self.groups[g].parent.is_some() {
                continue;
            }
            if self.ctx.is_some() && !self.create.contains_key(&g) {
                continue;
            }
            let (groups, forest) = (self.groups, self.forest);
            let first = &forest.rows[groups[g].rows[0]];
            let Some(rec) = &first.record else {
                self.fail(
                    g,
                    Conflict::new(first.hash.clone(), 0, INVALID_FORMAT, None),
                );
                continue;
            };
            match resolve_fork(self.conn, rec.parent_hash.as_deref())? {
                Fork::At { branch, .. }
                    if crate::branch::index::get(self.conn, &branch)?
                        .is_some_and(|i| i.archived) =>
                {
                    // A branch is never made from an archived one (`BRANCH_ARCHIVED`).
                    let mut c = Conflict::new(rec.tx_id.clone(), 0, "BRANCH_ARCHIVED", None);
                    c.fix = format!("fin branch archive {branch} --restore");
                    self.fail(g, c);
                }
                Fork::At {
                    branch,
                    seq,
                    root,
                    learn,
                } => {
                    let mut remap = Remap::default();
                    for (rec, bindings) in learn.iter().rev() {
                        remap.learn(rec, bindings);
                    }
                    let state = lineage::state_at(self.conn, &branch, seq)?;
                    self.group(
                        g,
                        Start {
                            at: Some((branch, seq)),
                            state,
                            remap,
                            root,
                        },
                    )?;
                }
                Fork::NoCommonHistory => {
                    let c = Conflict::new(rec.tx_id.clone(), 0, NO_COMMON_HISTORY, None);
                    self.fail(g, c);
                }
            }
        }
        if let Some(ctx) = self.ctx.as_deref() {
            // The ids the copies took are never handed out again.
            self.ids.save(ctx.txn)?;
        }
        Ok(())
    }

    /// `c` for group `g`, and `DEPENDS_ON_SKIPPED` for the groups forking from it.
    fn fail(&mut self, g: usize, c: Conflict) {
        let against = c.tx_id.clone();
        self.problems[g].push(c);
        let mut stack: Vec<usize> = self.tree_children(g);
        while let Some(k) = stack.pop() {
            let first = &self.forest.rows[self.groups[k].rows[0]];
            self.problems[k].push(Conflict::new(
                first
                    .record
                    .as_ref()
                    .map_or(first.hash.clone(), |r| r.tx_id.clone()),
                0,
                reasons::DEPENDS_ON_SKIPPED,
                Some(against.clone()),
            ));
            stack.extend(self.tree_children(k));
        }
    }

    fn tree_children(&self, g: usize) -> Vec<usize> {
        self.groups[g]
            .children
            .iter()
            .copied()
            .filter(|&c| self.groups[c].parent.is_some_and(|(p, _)| p == g))
            .collect()
    }

    fn next_id(&mut self) -> String {
        self.n += 1;
        format!("{}-{}", self.key, self.n)
    }

    fn group(&mut self, g: usize, start: Start) -> Result<(), StoreError> {
        let (groups, forest) = (self.groups, self.forest);
        let group = &groups[g];
        let rows = &forest.rows;
        if group.rows.iter().any(|&i| rows[i].record.is_none()) {
            let c = Conflict::new(rows[group.rows[0]].hash.clone(), 0, INVALID_FORMAT, None);
            self.fail(g, c);
            return Ok(());
        }
        let kids = self.tree_children(g);
        if !group
            .rows
            .iter()
            .any(|&i| rows[i].record.as_ref().is_some_and(|r| r.kind.has_ops()))
        {
            // Nothing of its own: its children fork where it does.
            if group.children.is_empty() {
                let first = rows[group.rows[0]].record.as_ref().map(|r| r.tx_id.clone());
                self.problems[g].push(Conflict::new(
                    first.unwrap_or_default(),
                    0,
                    NOTHING_TO_RECOVER,
                    None,
                ));
            }
            for k in kids {
                if self.ctx.is_none() || self.create.contains_key(&k) {
                    self.group(k, start.clone())?;
                }
            }
            return Ok(());
        }
        let fork_ids = entity_ids(&start.state);
        let mut state = start.state;
        let mut remap = start.remap;
        let mut branch = None;
        if let Some(ctx) = self.ctx.as_deref_mut()
            && let Some(name) = self.create.get(&g)
        {
            let (from, seq) = start.at.clone().ok_or_else(|| {
                CoreError::new("DB_ERROR", "a recovered group has no fork to write at")
            })?;
            let tx_id = format!("{}-{}", self.key, self.n + 1);
            super::taken(ctx.txn, &tx_id, self.key)?;
            self.n += 1;
            // `plan_create` allocates the branch id from the saved counters.
            self.ids.save(ctx.txn)?;
            let made = plan_create(
                ctx,
                &BranchCreate {
                    tx_id,
                    author: self.author.to_string(),
                    name: Some(name.clone()),
                    from,
                    at: Some(At::Seq(seq)),
                    dry_run: ctx.dry_run,
                    args_hash: None,
                },
            )?;
            self.ids = Counters::load(ctx.txn)?;
            state = ctx.head(&made.branch.id)?.project.clone();
            self.committed.push(made.record);
            self.created.push((g, made.branch.id.clone()));
            branch = Some(made.branch.id);
        }
        let mut forks: Vec<(usize, Start)> = vec![];
        let mut failed = None;
        for (pos, &i) in group.rows.iter().enumerate() {
            let row = &rows[i];
            let rec = row.record.as_ref().expect("checked above");
            if failed.is_none()
                && let Err(c) = self.one(
                    rec,
                    &row.hash,
                    &fork_ids,
                    &mut state,
                    &mut remap,
                    branch.as_deref(),
                    &start.root,
                )?
            {
                failed = Some(c);
            }
            for &k in &kids {
                if self.groups[k].parent == Some((g, pos)) {
                    let at = match (&branch, self.ctx.as_deref_mut()) {
                        (Some(b), Some(ctx)) => Some((b.clone(), ctx.head(b)?.rev)),
                        _ => None,
                    };
                    forks.push((
                        k,
                        Start {
                            at,
                            state: state.clone(),
                            remap: remap.clone(),
                            root: start.root.clone(),
                        },
                    ));
                }
            }
        }
        if let Some(c) = failed {
            if self.ctx.is_some() && branch.is_some() {
                // Planning found none: the state the write sees is not the one planned.
                return Err(CoreError::from(fin_core::Conflicts::one(c)).into());
            }
            self.fail(g, c);
            return Ok(());
        }
        for (k, s) in forks {
            if self.ctx.is_none() || self.create.contains_key(&k) {
                self.group(k, s)?;
            }
        }
        Ok(())
    }

    /// Re-runs one record on the recovered branch (`branch`; `None` while planning).
    #[allow(clippy::too_many_arguments)]
    fn one(
        &mut self,
        rec: &Record,
        hash: &str,
        fork_ids: &std::collections::BTreeSet<String>,
        state: &mut Project,
        remap: &mut Remap,
        branch: Option<&str>,
        root: &(String, u64),
    ) -> Result<Result<(), Conflict>, StoreError> {
        if !rec.kind.has_ops() {
            if let (RecordKind::Checkpoint, Some(b), Some(name)) = (rec.kind, branch, &rec.name) {
                let tx_id = self.next_id();
                super::taken(self.conn, &tx_id, self.key)?;
                let ctx = self.ctx.as_deref_mut().expect("writing");
                let draft = metadata_draft(
                    ctx,
                    b,
                    &tx_id,
                    self.author,
                    Link::Checkpoint { name: name.clone() },
                )?;
                self.committed.push(ctx.append(draft)?);
            }
            return Ok(Ok(()));
        }
        let bound = &mut self.bound;
        let mut tx = match remap.prepare(rec, fork_ids, &mut |id| bound.elsewhere(root, id))? {
            Ok(tx) => tx,
            Err(c) => return Ok(Err(c)),
        };
        tx.author = self.author.to_string();
        tx.args_hash = None;
        if let Some(b) = branch {
            tx.tx_id = self.next_id();
            super::taken(self.conn, &tx.tx_id, self.key)?;
            tx.branch = b.to_string();
            let ctx = self.ctx.as_deref_mut().expect("writing");
            tx.base_rev = ctx.head(b)?.rev;
        }
        let applied = match apply_tx_mode(state, &tx, &mut self.ids, ApplyMode::Internal) {
            Ok(a) => a,
            Err(e) => {
                let op = e.details.get("opIndex").and_then(serde_json::Value::as_u64);
                let op = usize::try_from(op.unwrap_or(0)).unwrap_or(0);
                return Ok(Err(Conflict::from_rerun(&e, &rec.tx_id, op, None)));
            }
        };
        remap.learn(rec, &applied.bindings);
        *state = applied.project.clone();
        if let Some(b) = branch {
            let ctx = self.ctx.as_deref_mut().expect("writing");
            let c = ctx.append(Draft {
                branch: b.to_string(),
                link: Link::CherryPick {
                    origin: rec.tx_id.clone(),
                    from: rec.intent.branch.clone(),
                    origin_hash: Some(hash.to_string()),
                },
                intent: tx,
                inverse: applied.inverse,
                bindings: applied.bindings,
                diff: applied.diff,
                project: applied.project,
            })?;
            self.committed.push(c);
        }
        Ok(Ok(()))
    }
}

/// The groups a write takes for `selected`: those groups and the groups forking from them
/// (listed under them too), when they can be created.
pub(crate) fn closure(
    groups: &[Group],
    problems: &[Vec<Conflict>],
    selected: &[usize],
) -> HashSet<usize> {
    let mut out = HashSet::new();
    let mut stack = selected.to_vec();
    while let Some(g) = stack.pop() {
        if problems[g].is_empty() && out.insert(g) {
            stack.extend(groups[g].children.iter().copied());
        }
    }
    out
}
