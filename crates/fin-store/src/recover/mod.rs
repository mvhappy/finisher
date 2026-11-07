//! Recovered branches (spec §3.6.2 step 8, §3.7; plan 4B Decisions 5, 8, 27–29): the records
//! set aside in quarantine come back as `recovered-*` branches.
//!
//! - [`Store::recover_groups`] lists the current quarantine rows not recovered yet as groups
//!   ([`groups`]): `parentHash` chains, each forking at its last common record with the log,
//!   with child groups for the branches set aside with them. Each is planned ([`run`],
//!   [`rows`]) to tell whether it can be created (`problems`: `ID_COLLISION`,
//!   `NO_COMMON_HISTORY`, a policy failure, `NOTHING_TO_RECOVER`, `DEPENDS_ON_SKIPPED`, …)
//!   and which rows it would leave (`notes`).
//! - [`Store::recover`] with `create` makes branches in one write: per group, a `branch`
//!   record on the parent, then a record standing for each row (a `cherryPick` of the record's
//!   origin root under fresh or kept ids, [`ids`], [`remap`]; a fresh checkpoint, `branch` or
//!   `branchSet` record), then its child groups. The records are `<txId>-<n>`; a retry of the
//!   txId answers from the log. `import_dir` first reads `.fseg` files moved out of the
//!   folder into quarantine (`IMPORTED`, origin `foreign`; [`import_dir`]), and `dismiss`
//!   sets a group that cannot be recovered aside from listing and counting on this server
//!   ([`dismiss`]); both also while the project is read-only.
//!
//! A quarantine row is recovered while a record in the log stands for it (`recovers`):
//! derived on import and rebuild alike, and compared by verify through `record_links`.

mod dismiss;
mod groups;
mod ids;
mod import_dir;
mod marks;
mod naming;
mod remap;
mod rows;
mod run;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;

use fin_core::layout::TextMeasurer;
use fin_core::{CommittedTx, Conflict, CoreError, Link, RecordKind};
use rusqlite::Connection;
use serde_json::json;

use groups::{Forest, Group};
pub(crate) use marks::{check as check_marks, fill as fill_marks, uncounted};
use naming::{Writers, server8, utc_stamp};
use run::Runner;

use crate::branch::create::{check_key, key_reused, view};
use crate::branch::index;
use crate::write::WriteCtx;
use crate::{BranchView, ReadOnlyReason, RecoverGroup, RecoverReport, Store, StoreError, db};

/// Which groups `create` makes: every recoverable one, or one group (by id) with its
/// children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoverSel {
    All,
    Group(String),
}

/// On the wire: `"all"`, or a group id.
impl<'de> serde::Deserialize<'de> for RecoverSel {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(if s == "all" {
            RecoverSel::All
        } else {
            RecoverSel::Group(s)
        })
    }
}

/// [`Store::recover`]'s request: `tx_id` (the idempotency key: the records are
/// `<txId>-<n>`), `author` (the records' author), `create`, `import_dir` (a folder of `.fseg`
/// files moved out of the project folder), `dismiss` (a group id: stop listing and counting
/// a group that cannot be recovered, on this server), `dry_run` (plan against the local
/// database and keep nothing, on a follower too).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverReq {
    #[serde(default)]
    pub tx_id: String,
    #[serde(default)]
    pub author: String,
    pub create: Option<RecoverSel>,
    pub import_dir: Option<PathBuf>,
    #[serde(default)]
    pub dismiss: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// What [`listing`] found.
struct Listing {
    forest: Forest,
    groups: Vec<Group>,
    order: Vec<usize>,
    problems: Vec<Vec<Conflict>>,
    names: HashMap<usize, String>,
    out: Vec<RecoverGroup>,
}

/// Whether group `g` makes a branch: it has records with ops or checkpoints.
fn makes_branch(forest: &Forest, g: &Group) -> bool {
    g.rows.iter().any(|&i| {
        forest.rows[i]
            .record
            .as_ref()
            .is_some_and(|r| r.kind.has_ops() || r.kind == RecordKind::Checkpoint)
    })
}

/// The groups over `conn`, planned, named (unique among the branches and each other, in
/// listing order, for the recoverable ones), as listed.
/// `measurer`: what the plan's re-runs check caption overflow with, as the write's will.
fn listing(
    conn: &Connection,
    writers: &Writers,
    author: &str,
    measurer: Option<Arc<dyn TextMeasurer>>,
) -> Result<Listing, StoreError> {
    let forest = Forest::load(conn)?;
    let (groups, order) = forest.groups();
    let (problems, notes, shown) = {
        let mut r = Runner::new(conn, &forest, &groups, author, "plan")?.measuring(measurer);
        r.run(&order)?;
        (r.problems, r.notes, r.shown)
    };
    let ix = index::load(conn)?;
    let mut reserved: HashSet<String> = HashSet::new();
    let mut names = HashMap::new();
    let mut out = Vec::with_capacity(order.len());
    for &g in &order {
        let group = &groups[g];
        let first = &forest.rows[group.rows[0]];
        let rec = first.record.as_ref();
        let writer = writers.of(first);
        let name = if makes_branch(&forest, group) {
            let base = format!(
                "recovered-{}-{}",
                server8(writer.as_deref()),
                utc_stamp(rec.map_or(0, |r| r.committed_at))
            );
            let mut name = base.clone();
            let mut k = 2;
            while ix.holder(&name).is_some() || reserved.contains(&name.to_lowercase()) {
                name = format!("{base}-{k}");
                k += 1;
            }
            if problems[g].is_empty() {
                reserved.insert(name.to_lowercase());
                names.insert(g, name.clone());
            }
            name
        } else {
            String::new()
        };
        let mut reasons = BTreeMap::new();
        for &i in &group.rows {
            *reasons.entry(forest.rows[i].reason.clone()).or_default() += 1;
        }
        let fork = &shown[g];
        let fork_group = fork
            .group
            .map(|p| group_id(&forest.rows[groups[p].rows[0]].hash));
        let branch = match fork.group {
            // Listed before: its name is known.
            Some(p) => names.get(&p).cloned().unwrap_or_default(),
            None => fork.branch.clone(),
        };
        out.push(RecoverGroup {
            id: group_id(&first.hash),
            records_branch: first.branch.clone(),
            branch,
            fork_seq: fork.seq,
            fork_hash: fork.hash.clone(),
            fork_group,
            records: group.rows.len(),
            reasons,
            origin: first.origin.clone(),
            server: writer.unwrap_or_else(|| "unknown".to_string()),
            first_committed_at: rec.map_or(0, |r| r.committed_at),
            name,
            children: group
                .children
                .iter()
                .map(|&c| group_id(&forest.rows[groups[c].rows[0]].hash))
                .collect(),
            recoverable: problems[g].is_empty(),
            problems: problems[g].clone(),
            notes: notes[g].clone(),
        });
    }
    Ok(Listing {
        forest,
        groups,
        order,
        problems,
        names,
        out,
    })
}

fn group_id(hash: &str) -> String {
    crate::names::hash12(hash).unwrap_or(hash).to_string()
}

/// `IDEM_KEY_REUSED` when another record holds `tx_id`, a txId the recovery would write.
pub(crate) fn taken(conn: &Connection, tx_id: &str, key: &str) -> Result<(), StoreError> {
    crate::history_ops::merge_id_taken(conn, tx_id)?;
    let Some(existing) = db::find_tx(conn, tx_id)? else {
        return Ok(());
    };
    Err(CoreError::new(
        "IDEM_KEY_REUSED",
        format!("recovery {key} would write txId {tx_id}, which another record holds"),
    )
    .with_details(json!({ "txId": tx_id, "rev": existing.rev, "branch": existing.tx.branch }))
    .with_fix("use a new idempotency key (txId) for the recovery")
    .into())
}

/// The manual procedure for a read-only state recovery does not lift (Decision 29).
fn manual(reason: Option<ReadOnlyReason>) -> Option<String> {
    let first = match reason? {
        ReadOnlyReason::ChainFork => {
            "CHAIN_FORK: two histories exist under .finisher/segments/<branch>/. Decide which one to keep. Stop Finisher on every machine that has the project open, then move the other history's segment files (the `pending` ones past the fork) out of the project folder, to a place outside it."
        }
        ReadOnlyReason::ConflictCopy => {
            "CONFLICT_COPY: a sync client made a conflict copy of a Finisher file (listed in `foreign`). Move the copy out of .finisher/ to a place outside the project folder."
        }
        _ => return None,
    };
    Some(format!(
        "{first} Reopen the project; once `fin project status` shows `readOnly: null`, run `fin project recover --import <that folder> --create all` to bring the moved records back as recovered-* branches."
    ))
}

/// What a recovery wrote: the groups as found, the branches made, the records.
type Created = (Vec<RecoverGroup>, Vec<BranchView>, Vec<CommittedTx>);

impl Store {
    /// The quarantined records not recovered (nor dismissed) yet, as groups (module docs),
    /// parents before their children.
    pub fn recover_groups(&self) -> Result<Vec<RecoverGroup>, StoreError> {
        Ok(listing(&self.conn, &self.writers()?, "", self.measurer.clone())?.out)
    }

    fn writers(&self) -> Result<Writers, StoreError> {
        Writers::new(&self.conn, &self.server_id, &self.project_dir)
    }

    /// Recovers quarantined records (module docs): reads `import_dir` into quarantine,
    /// dismisses `dismiss`, lists the groups and, with `create`, makes their branches in one
    /// write.
    ///
    /// Errors: `INVALID_OP` (empty txId with `create`; a group that cannot be created, with
    /// `details.problems`; a child group whose parent group is not created; dismissing a
    /// group that can be recovered); `NOT_FOUND` (`details.kind` `group`); `IO_ERROR`
    /// (`import_dir`); for `create` as a commit: a retried txId answers from the log
    /// (`IDEM_KEY_REUSED` when `<txId>-1` holds no recovery record), `PROJECT_READ_ONLY`;
    /// `IDEM_KEY_REUSED` when another record holds a txId it would write. `import_dir` and
    /// `dismiss` are allowed while read-only. A dry run plans everything (the import
    /// included) and keeps nothing, on a follower too.
    pub fn recover(&mut self, r: RecoverReq) -> Result<RecoverReport, StoreError> {
        let manual = manual(self.read_only());
        if r.create.is_some() {
            check_key(&r.tx_id)?;
        }
        let mut imported = import_dir::Imported::default();
        let mut dismissed = 0;
        if !r.dry_run {
            let txn = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if let Some(dir) = &r.import_dir {
                imported = import_dir::run(&txn, &self.project_id, dir)?;
            }
            if let Some(id) = &r.dismiss {
                let writers = Writers::new(&txn, &self.server_id, &self.project_dir)?;
                dismissed = dismiss_group(&txn, &writers, id, self.measurer.clone())?;
            }
            txn.commit()?;
        }
        let report = |(groups, created, committed): Created,
                      imported: import_dir::Imported,
                      dismissed,
                      dry_run| RecoverReport {
            groups,
            created,
            committed,
            imported: imported.rows,
            skipped_files: imported.skipped,
            dismissed,
            manual: manual.clone(),
            dry_run,
        };
        let dry_extra = r.dry_run && (r.import_dir.is_some() || r.dismiss.is_some());
        if r.create.is_none() && !dry_extra {
            let groups = self.recover_groups()?;
            return Ok(report(
                (groups, vec![], vec![]),
                imported,
                dismissed,
                r.dry_run,
            ));
        }
        // Folder writers are read before the write lock is taken.
        let writers = self.writers()?;
        let first = format!("{}-1", r.tx_id);
        let project_id = self.project_id.clone();
        loop {
            let wrote = self.write(&first, r.dry_run, |ctx| {
                let (mut imp, mut dis) = (import_dir::Imported::default(), 0);
                if r.dry_run {
                    if let Some(dir) = &r.import_dir {
                        imp = import_dir::run(ctx.txn, &project_id, dir)?;
                    }
                    if let Some(id) = &r.dismiss {
                        dis = dismiss_group(ctx.txn, &writers, id, ctx.measurer.clone())?;
                    }
                }
                Ok((create(ctx, &r, &writers)?, imp, dis))
            })?;
            if let Some((made, imp, dis)) = wrote {
                if r.dry_run {
                    (imported, dismissed) = (imp, dis);
                }
                return Ok(report(made, imported, dismissed, r.dry_run));
            }
            // Deduped; gone again (an import rewound it since): the recovery runs again.
            if db::find_tx(&self.conn, &first)?.is_some() {
                let (created, committed) = self.recovered_before(&r.tx_id)?;
                let groups = self.recover_groups()?;
                return Ok(report(
                    (groups, created, committed),
                    imported,
                    dismissed,
                    false,
                ));
            }
        }
    }

    /// The answer to a retried recovery: its records `<txId>-<n>` and the branches they made.
    fn recovered_before(
        &self,
        key: &str,
    ) -> Result<(Vec<BranchView>, Vec<CommittedTx>), StoreError> {
        let mut committed: Vec<CommittedTx> = vec![];
        while let Some(c) = db::find_tx(&self.conn, &format!("{key}-{}", committed.len() + 1))? {
            committed.push(c);
        }
        // A recovery writes a `branch` record first, or a record standing for a row.
        let stands = |l: &Link| match l {
            Link::Branch { .. } => true,
            Link::CherryPick { recovers, .. }
            | Link::Checkpoint { recovers, .. }
            | Link::BranchSet { recovers, .. } => recovers.is_some(),
            _ => false,
        };
        if !committed[0].link.as_deref().is_some_and(stands) {
            return Err(key_reused(&committed[0]));
        }
        let ix = index::load(&self.conn)?;
        let mut created = vec![];
        for c in &committed {
            if let Some(Link::Branch { branch_id, .. }) = c.link.as_deref()
                && let Some(info) = ix.get(branch_id)
            {
                created.push(view(info, ix.display(info), self.head_of(branch_id)));
            }
        }
        Ok((created, committed))
    }
}

/// Dismisses group `id` (it must not be recoverable); returns the rows newly dismissed.
fn dismiss_group(
    conn: &Connection,
    writers: &Writers,
    id: &str,
    measurer: Option<Arc<dyn TextMeasurer>>,
) -> Result<usize, StoreError> {
    let l = listing(conn, writers, "", measurer)?;
    let (g, _) = find(&l, id)?;
    if l.problems[g].is_empty() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("recover group {id} can be recovered; only a group that cannot is dismissed"),
        )
        .with_details(json!({ "group": id }))
        .with_fix(format!(
            "fin project recover --create {id} (archive the branch it makes if it is not wanted)"
        ))
        .into());
    }
    let hashes: Vec<String> = l.groups[g]
        .rows
        .iter()
        .map(|&i| l.forest.rows[i].hash.clone())
        .collect();
    dismiss::add(conn, &hashes)
}

/// The groups `r.create` selects, written through `ctx`.
fn create(
    ctx: &mut WriteCtx<'_>,
    r: &RecoverReq,
    writers: &Writers,
) -> Result<Created, StoreError> {
    let txn = ctx.txn;
    let l = listing(txn, writers, &r.author, ctx.measurer.clone())?;
    let Some(sel) = &r.create else {
        return Ok((l.out, vec![], vec![]));
    };
    let take = match sel {
        RecoverSel::All => run::closure(&l.groups, &l.problems, &l.order),
        RecoverSel::Group(id) => {
            let (g, above) = one(&l, id)?;
            let mut take = run::closure(&l.groups, &l.problems, &[g]);
            take.extend(above);
            take
        }
    };
    let create: HashMap<usize, String> = take
        .iter()
        .map(|&g| (g, l.names.get(&g).cloned().unwrap_or_default()))
        .collect();
    let (created_groups, committed) = {
        let mut runner = Runner::new(txn, &l.forest, &l.groups, &r.author, &r.tx_id)?
            .measuring(ctx.measurer.clone())
            .writing(ctx, create);
        runner.run(&l.order)?;
        (runner.created, runner.committed)
    };
    ctx.settle()?;
    let ix = index::load(txn)?;
    let mut created = vec![];
    for (_, id) in &created_groups {
        let info = ix.get(id).ok_or_else(|| index::missing(id))?;
        let head = ctx.head(id)?.rev;
        created.push(view(info, ix.display(info), head));
    }
    Ok((l.out, created, committed))
}

/// The group `id` names, and its listing position.
fn find(l: &Listing, id: &str) -> Result<(usize, usize), StoreError> {
    l.out
        .iter()
        .position(|g| g.id == id)
        .map(|i| (l.order[i], i))
        .ok_or_else(|| {
            CoreError::new("NOT_FOUND", format!("no recover group {id}"))
                .with_details(json!({ "kind": "group", "group": id }))
                .with_fix("list the groups with fin project recover")
                .into()
        })
}

/// The group `id` names, when `create` can make it, and the groups it forks from, which make
/// no branch (it forks where they do).
fn one(l: &Listing, id: &str) -> Result<(usize, Vec<usize>), StoreError> {
    let (g, _) = find(l, id)?;
    if !l.problems[g].is_empty() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("recover group {id} cannot be created"),
        )
        .with_details(json!({ "group": id, "problems": l.problems[g] }))
        .with_fix("resolve its problems first (each says how), or dismiss it: fin project recover --dismiss <group>; the records stay in quarantine")
        .into());
    }
    // Its parent groups must be made with it: only those that make no branch are.
    let mut above = vec![];
    let mut p = l.groups[g].parent;
    while let Some((pg, _)) = p {
        if makes_branch(&l.forest, &l.groups[pg]) {
            let parent =
                &l.out[find(l, &group_id(&l.forest.rows[l.groups[pg].rows[0]].hash))?.1].id;
            return Err(CoreError::new(
                "INVALID_OP",
                format!("recover group {id} forks from group {parent}, which is not created"),
            )
            .with_details(json!({ "group": id, "parent": parent }))
            .with_fix(format!("fin project recover --create {parent}"))
            .into());
        }
        above.push(pg);
        p = l.groups[pg].parent;
    }
    Ok((g, above))
}
