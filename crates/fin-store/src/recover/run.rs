//! Running recover groups (plan 4B Decision 27): one walk over the groups that either only
//! plans them, to list what each would give and why it cannot be created (`problems`) or
//! what it leaves in quarantine (`notes`), or writes them inside one
//! [`Store::write`](crate::Store::write).
//!
//! A group is re-run from its fork state record by record ([`super::rows`]): a record with
//! ops as a `cherryPick` copy, a `checkpoint` as a fresh `checkpoint`, a `branch` record whose
//! branch has no records (no group of its own) as that branch made again, a `branchSet` of a
//! live branch as a fresh `branchSet`; each names the row it stands for (`recovers`). The
//! groups forking at its rows follow, from the state and the remap at that row, on the
//! recovered branch at its copy. A group with neither records with ops nor checkpoints makes
//! no branch: its children fork where it does (its records change no state).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fin_core::layout::TextMeasurer;
use fin_core::rebase::reasons;
use fin_core::{CommittedTx, Conflict, CoreError, Project, RecordKind};
use rusqlite::Connection;

use super::groups::{Forest, Fork, Group, resolve_fork};
use super::ids::RootIds;
use super::remap::{Bound, Remap, entity_ids};
use crate::StoreError;
use crate::branch::create::{At, BranchCreate, plan_create_with};
use crate::branch::lineage;
use crate::db::Counters;
use crate::write::WriteCtx;

/// Problem reasons besides the rebase ones.
pub(crate) const NO_COMMON_HISTORY: &str = "NO_COMMON_HISTORY";
pub(crate) const NOTHING_TO_RECOVER: &str = "NOTHING_TO_RECOVER";
pub(crate) const INVALID_FORMAT: &str = "INVALID_FORMAT";

/// The fork a group starts at, as listed: an existing branch and seq (`hash` its record), or
/// a seq on the branch group `group` makes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Shown {
    pub branch: String,
    pub seq: u64,
    pub hash: Option<String>,
    pub group: Option<usize>,
}

/// Where and from what a group starts.
#[derive(Clone)]
pub(super) struct Start {
    /// The branch and seq to fork from (written runs only; `None` while planning).
    pub at: Option<(String, u64)>,
    pub shown: Shown,
    pub state: Project,
    pub remap: Remap,
    /// The record the chain leaves the log at (see [`Fork::At`]).
    pub root: (String, u64),
}

/// The group walk (module docs). `ctx`: write the groups in `create`; `None`: plan only.
pub(crate) struct Runner<'a, 'c, 'w> {
    pub(super) conn: &'a Connection,
    pub(super) forest: &'a Forest,
    pub(super) groups: &'a [Group],
    pub(super) author: &'a str,
    pub(super) ctx: Option<&'c mut WriteCtx<'w>>,
    /// The groups to write (with `ctx`), and their names.
    create: HashMap<usize, String>,
    /// The request's txId: the records are `<txId>-<n>`.
    pub(super) key: &'a str,
    /// The request's identity ([`super::request_hash`]), the `argsHash` of every record
    /// written: a retry is answered only from records that carry it.
    pub(super) args_hash: Option<String>,
    n: u64,
    pub(super) ids: Counters,
    pub(super) bound: Bound<'a>,
    pub(super) root_ids: RootIds,
    /// Where each row of this run stands now: the branch and seq (while planning, `#<group>`
    /// for a branch to be made).
    pub(super) pos_of: HashMap<String, (String, u64)>,
    pub problems: Vec<Vec<Conflict>>,
    /// Rows a create leaves in quarantine, and copies that lost their source's link.
    pub notes: Vec<Vec<Conflict>>,
    pub shown: Vec<Shown>,
    /// The branch each written group made, in the order they were made.
    pub created: Vec<(usize, String)>,
    pub committed: Vec<CommittedTx>,
    /// What the rows' re-runs check caption overflow with (plan 4C Task 20).
    pub(super) measurer: Option<Arc<dyn TextMeasurer>>,
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
            args_hash: None,
            n: 0,
            ids: Counters::load(conn)?,
            bound: Bound::new(conn),
            root_ids: RootIds::default(),
            pos_of: HashMap::new(),
            problems: vec![vec![]; groups.len()],
            notes: vec![vec![]; groups.len()],
            shown: vec![Shown::default(); groups.len()],
            created: vec![],
            committed: vec![],
            measurer: None,
        })
    }

    /// The run re-running its rows with `m` (the store's measurer).
    pub(crate) fn measuring(mut self, m: Option<Arc<dyn TextMeasurer>>) -> Self {
        self.measurer = m;
        self
    }

    /// Writes the groups of `create` (with their names) through `ctx`, each record with
    /// `args_hash` (the request's identity).
    pub(crate) fn writing(
        mut self,
        ctx: &'c mut WriteCtx<'w>,
        create: HashMap<usize, String>,
        args_hash: String,
    ) -> Self {
        self.ctx = Some(ctx);
        self.create = create;
        self.args_hash = Some(args_hash);
        self
    }

    pub(super) fn writes(&self) -> bool {
        self.ctx.is_some()
    }

    /// Runs every root group in `order` (and, from each, the groups forking at its rows).
    pub(crate) fn run(&mut self, order: &[usize]) -> Result<(), StoreError> {
        for &g in order {
            if self.groups[g].parent.is_some() {
                continue;
            }
            if self.writes() && !self.create.contains_key(&g) {
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
                    let shown = Shown {
                        hash: lineage::hash_at(self.conn, &branch, seq)?,
                        branch: branch.clone(),
                        seq,
                        group: None,
                    };
                    self.group(
                        g,
                        Start {
                            at: Some((branch, seq)),
                            shown,
                            state,
                            remap,
                            root,
                        },
                    )?;
                }
                Fork::NoCommonHistory => {
                    let mut c = Conflict::new(rec.tx_id.clone(), 0, NO_COMMON_HISTORY, None);
                    c.fix = "the record it continues is not here: import the folder that holds it (fin project recover --import <dir>), or dismiss the group".into();
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
    pub(super) fn fail(&mut self, g: usize, c: Conflict) {
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

    pub(super) fn next_id(&mut self) -> Result<String, StoreError> {
        let id = format!("{}-{}", self.key, self.n + 1);
        super::taken(self.conn, &id, self.key)?;
        self.n += 1;
        Ok(id)
    }

    fn group(&mut self, g: usize, start: Start) -> Result<(), StoreError> {
        let (groups, forest) = (self.groups, self.forest);
        let group = &groups[g];
        let rows = &forest.rows;
        self.shown[g] = start.shown.clone();
        if group.rows.iter().any(|&i| rows[i].record.is_none()) {
            let c = Conflict::new(rows[group.rows[0]].hash.clone(), 0, INVALID_FORMAT, None);
            self.fail(g, c);
            return Ok(());
        }
        let kids = self.tree_children(g);
        let needs_branch = group.rows.iter().any(|&i| {
            rows[i]
                .record
                .as_ref()
                .is_some_and(|r| r.kind.has_ops() || r.kind == RecordKind::Checkpoint)
        });
        let fork_ids = entity_ids(&start.state);
        let mut state = start.state.clone();
        let mut remap = start.remap.clone();
        // The branch the rows go on: made now, or (planning, or nothing to make) none.
        let mut branch = None;
        let mut seq = start.shown.seq;
        if needs_branch
            && let Some(name) = self.create.get(&g).cloned()
            && self.ctx.is_some()
        {
            let (from, at) = start.at.clone().ok_or_else(|| {
                CoreError::new("DB_ERROR", "a recovered group has no fork to write at")
            })?;
            let tx_id = self.next_id()?;
            let made_by = group.made_by.map(|j| rows[j].hash.clone());
            // `plan_create_with` allocates the branch id from the saved counters.
            let ctx = self.ctx.as_deref_mut().expect("writing");
            self.ids.save(ctx.txn)?;
            let made = plan_create_with(
                ctx,
                &BranchCreate {
                    tx_id,
                    author: self.author.to_string(),
                    name: Some(name),
                    from,
                    at: Some(At::Seq(at)),
                    dry_run: ctx.dry_run,
                    args_hash: self.args_hash.clone(),
                },
                made_by,
            )?;
            self.ids = Counters::load(ctx.txn)?;
            state = ctx.head(&made.branch.id)?.project.clone();
            self.committed.push(made.record);
            self.created.push((g, made.branch.id.clone()));
            branch = Some(made.branch.id);
        }
        // Where this group's rows stand while planning: its branch to be, or (making none)
        // its fork.
        let here = |branch: &Option<String>| match branch {
            Some(b) => b.clone(),
            None if needs_branch => format!("#{g}"),
            None => start.at.clone().map_or_else(|| format!("#{g}"), |a| a.0),
        };
        let mut forks: Vec<(usize, Start)> = vec![];
        let mut failed = None;
        let mut wrote = false;
        for (pos, &i) in group.rows.iter().enumerate() {
            let row = &rows[i];
            let rec = row.record.as_ref().expect("checked above");
            if failed.is_none() {
                let at = (here(&branch), seq);
                let r = self.row(
                    g,
                    i,
                    rec,
                    &fork_ids,
                    &mut state,
                    &mut remap,
                    branch.as_deref(),
                    at,
                    &start.root,
                )?;
                match r {
                    Ok((on_branch, any)) => {
                        wrote |= any;
                        seq += on_branch;
                    }
                    Err(c) => failed = Some(c),
                }
            }
            let at_now = match (&branch, self.ctx.as_deref_mut()) {
                (Some(b), Some(ctx)) => (b.clone(), ctx.head(b)?.rev),
                _ => (here(&branch), seq),
            };
            self.pos_of.insert(row.hash.clone(), at_now.clone());
            for &k in &kids {
                if self.groups[k].parent != Some((g, pos)) {
                    continue;
                }
                let (at, shown) = if needs_branch {
                    let shown = Shown {
                        branch: String::new(),
                        seq,
                        hash: None,
                        group: Some(g),
                    };
                    (branch.as_ref().map(|_| at_now.clone()), shown)
                } else {
                    (start.at.clone(), start.shown.clone())
                };
                forks.push((
                    k,
                    Start {
                        at,
                        shown,
                        state: state.clone(),
                        remap: remap.clone(),
                        root: start.root.clone(),
                    },
                ));
            }
        }
        if let Some(c) = failed {
            if self.writes() && branch.is_some() {
                // Planning found none: the state the write sees is not the one planned.
                return Err(CoreError::from(fin_core::Conflicts::one(c)).into());
            }
            self.fail(g, c);
            return Ok(());
        }
        if !needs_branch && !wrote && group.children.is_empty() && !self.writes() {
            // Nothing stands for any of it: what it leaves is why it cannot be created.
            let mut why = std::mem::take(&mut self.notes[g]);
            if why.is_empty() {
                let first = rows[group.rows[0]].record.as_ref().map(|r| r.tx_id.clone());
                let mut c = Conflict::new(first.unwrap_or_default(), 0, NOTHING_TO_RECOVER, None);
                c.fix = "nothing in it can be brought back on its own (it changes a branch set aside with another group, which brings it back); if that group cannot be recovered either, dismiss both: fin project recover --dismiss <group>".into();
                why.push(c);
            }
            self.problems[g].extend(why);
        }
        for (k, s) in forks {
            if !self.writes() || self.create.contains_key(&k) {
                self.group(k, s)?;
            }
        }
        Ok(())
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
