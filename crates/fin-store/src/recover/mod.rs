//! Recovered branches (spec §3.6.2 step 8, §3.7; plan 4B Decisions 5, 8, 27–29): the records
//! set aside in quarantine come back as `recovered-*` branches.
//!
//! - [`Store::recover_groups`] lists the current quarantine rows not recovered yet as groups
//!   ([`groups`]): `parentHash` chains, each forking at its last common record with the log,
//!   with child groups for the branches set aside with them. Each is planned
//!   ([`run`]) to tell whether it can be created (`problems`: `ID_COLLISION`,
//!   `NO_COMMON_HISTORY`, a policy failure, `NOTHING_TO_RECOVER`, `DEPENDS_ON_SKIPPED`).
//! - [`Store::recover`] with `create` makes branches in one write: per group, a `branch`
//!   record on the parent, then each record re-run under fresh ids ([`remap`]) as a
//!   `cherryPick` naming the quarantined record (`origin`, `from`, `originHash`), then its
//!   child groups. The records are `<txId>-<n>`; a retry of the txId answers from the log.
//!   `import_dir` first reads `.fseg` files moved out of the folder into quarantine
//!   (`IMPORTED`, origin `foreign`; [`import_dir`]), also while the project is read-only.
//!
//! A quarantine row is recovered while a `cherryPick` in the log names it (`originHash`):
//! derived on import and rebuild alike, and compared by verify through `record_links`.

mod groups;
mod import_dir;
mod remap;
mod run;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use fin_core::{CommittedTx, Conflict, CoreError, Link};
use rusqlite::Connection;
use serde_json::json;

pub(crate) use groups::recovered_hashes;
use groups::{Forest, Group, Row};
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

/// [`Store::recover`]'s request: `tx_id` (the idempotency key: the records are
/// `<txId>-<n>`), `author` (the records' author), `create`, `import_dir` (a folder of `.fseg`
/// files moved out of the project folder), `dry_run` (plan against the local database and
/// keep nothing, on a follower too).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverReq {
    pub tx_id: String,
    pub author: String,
    pub create: Option<RecoverSel>,
    pub import_dir: Option<PathBuf>,
    pub dry_run: bool,
}

/// The writer of each row's record, for names (Decision 27).
pub(crate) struct Writers {
    ours: String,
    project_dir: PathBuf,
    folder: Option<HashMap<String, String>>,
}

impl Writers {
    fn of(&mut self, row: &Row) -> Option<String> {
        match row.origin.as_str() {
            "local" | "salvage" => Some(self.ours.clone()),
            "foreign" => {
                let path = PathBuf::from(&row.detail);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.project_dir.join(path)
                };
                let seg = crate::segment::decode(&std::fs::read(path).ok()?).ok()?;
                seg.records
                    .iter()
                    .any(|b| db::hash_bytes(b) == row.hash)
                    .then_some(seg.header.writer_id)
            }
            _ => {
                let dir = &self.project_dir;
                self.folder
                    .get_or_insert_with(|| {
                        let mut m = HashMap::new();
                        for e in crate::scan::scan(dir).unwrap_or_default() {
                            if let crate::scan::Entry::Segment { seg, .. } = e {
                                for b in &seg.records {
                                    m.insert(db::hash_bytes(b), seg.header.writer_id.clone());
                                }
                            }
                        }
                        m
                    })
                    .get(&row.hash)
                    .cloned()
            }
        }
    }
}

/// `<server8>` of a writer id: its first 8 characters when they are letters or digits.
fn server8(writer: Option<&str>) -> String {
    match writer {
        Some(w)
            if w.len() >= 8
                && w.is_char_boundary(8)
                && w[..8].bytes().all(|b| b.is_ascii_alphanumeric()) =>
        {
            w[..8].to_ascii_lowercase()
        }
        _ => "unknown".to_string(),
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) of unix ms `ms`.
fn utc_stamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = crate::backup::civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
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

fn has_ops(forest: &Forest, g: &Group) -> bool {
    g.rows.iter().any(|&i| {
        forest.rows[i]
            .record
            .as_ref()
            .is_some_and(|r| r.kind.has_ops())
    })
}

/// The groups over `conn`, planned, named (unique among the branches and each other, in
/// listing order, for the recoverable ones), as listed.
fn listing(conn: &Connection, writers: &mut Writers, author: &str) -> Result<Listing, StoreError> {
    let forest = Forest::load(conn)?;
    let (groups, order) = forest.groups();
    let problems = {
        let mut r = Runner::new(conn, &forest, &groups, author, "plan")?;
        r.run(&order)?;
        r.problems
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
        let name = if has_ops(&forest, group) {
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
        out.push(RecoverGroup {
            id: group_id(&first.hash),
            branch: first.branch.clone(),
            fork_seq: rec.map_or(0, |r| r.seq.saturating_sub(1)),
            fork_hash: rec.and_then(|r| r.parent_hash.clone()),
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

impl Store {
    /// The quarantined records not recovered yet, as groups (module docs), parents before
    /// their children.
    pub fn recover_groups(&self) -> Result<Vec<RecoverGroup>, StoreError> {
        Ok(listing(&self.conn, &mut self.writers(), "")?.out)
    }

    fn writers(&self) -> Writers {
        Writers {
            ours: self.server_id.clone(),
            project_dir: self.project_dir.clone(),
            folder: None,
        }
    }

    /// Recovers quarantined records (module docs): reads `import_dir` into quarantine, lists
    /// the groups and, with `create`, makes their branches in one write.
    ///
    /// Errors: `INVALID_OP` (empty txId with `create`; a group that cannot be created, with
    /// `details.problems`; a child group whose parent group is not created); `NOT_FOUND`
    /// (`details.kind` `group`); `IO_ERROR` (`import_dir`); for `create` as a commit: a
    /// retried txId answers from the log (`IDEM_KEY_REUSED` when `<txId>-1` holds no
    /// `branch` record), `PROJECT_READ_ONLY`; `IDEM_KEY_REUSED` when another record holds a
    /// txId it would write. `import_dir` alone is allowed while read-only. A dry run plans
    /// everything (the import included) and keeps nothing, on a follower too.
    pub fn recover(&mut self, r: RecoverReq) -> Result<RecoverReport, StoreError> {
        let manual = manual(self.read_only());
        if r.create.is_some() {
            check_key(&r.tx_id)?;
        }
        let mut imported = 0;
        if !r.dry_run
            && let Some(dir) = &r.import_dir
        {
            let txn = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            imported = import_dir::run(&txn, &self.project_id, dir)?;
            txn.commit()?;
        }
        if r.create.is_none() && !(r.dry_run && r.import_dir.is_some()) {
            return Ok(RecoverReport {
                groups: self.recover_groups()?,
                created: vec![],
                committed: vec![],
                imported,
                manual,
                dry_run: r.dry_run,
            });
        }
        let first = format!("{}-1", r.tx_id);
        let project_id = self.project_id.clone();
        loop {
            let mut writers = self.writers();
            let wrote = self.write(&first, r.dry_run, |ctx| {
                if r.dry_run
                    && let Some(dir) = &r.import_dir
                {
                    imported = import_dir::run(ctx.txn, &project_id, dir)?;
                }
                create(ctx, &r, &mut writers)
            })?;
            if let Some((groups, created, committed)) = wrote {
                return Ok(RecoverReport {
                    groups,
                    created,
                    committed,
                    imported,
                    manual,
                    dry_run: r.dry_run,
                });
            }
            // Deduped; gone again (an import rewound it since): the recovery runs again.
            if db::find_tx(&self.conn, &first)?.is_some() {
                let (created, committed) = self.recovered_before(&r.tx_id)?;
                return Ok(RecoverReport {
                    groups: self.recover_groups()?,
                    created,
                    committed,
                    imported,
                    manual,
                    dry_run: false,
                });
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
        if !matches!(committed[0].link.as_deref(), Some(Link::Branch { .. })) {
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

type Created = (Vec<RecoverGroup>, Vec<BranchView>, Vec<CommittedTx>);

/// The groups `r.create` selects, written through `ctx`.
fn create(
    ctx: &mut WriteCtx<'_>,
    r: &RecoverReq,
    writers: &mut Writers,
) -> Result<Created, StoreError> {
    let txn = ctx.txn;
    let l = listing(txn, writers, &r.author)?;
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
        let mut runner =
            Runner::new(txn, &l.forest, &l.groups, &r.author, &r.tx_id)?.writing(ctx, create);
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

/// The group `id` names, when `create` can make it, and the groups it forks from, which make
/// no branch (it forks where they do).
fn one(l: &Listing, id: &str) -> Result<(usize, Vec<usize>), StoreError> {
    let Some(g) = l.out.iter().position(|g| g.id == id).map(|i| l.order[i]) else {
        return Err(
            CoreError::new("NOT_FOUND", format!("no recover group {id}"))
                .with_details(json!({ "kind": "group", "group": id }))
                .with_fix("list the groups with fin project recover")
                .into(),
        );
    };
    if !l.problems[g].is_empty() {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("recover group {id} cannot be created"),
        )
        .with_details(json!({ "group": id, "problems": l.problems[g] }))
        .with_fix("resolve its problems first; the records stay in quarantine")
        .into());
    }
    // Its parent groups must be made with it: only those that make no branch are.
    let mut above = vec![];
    let mut p = l.groups[g].parent;
    while let Some((pg, _)) = p {
        if has_ops(&l.forest, &l.groups[pg]) {
            let parent = &l.out[pos(l, pg)].id;
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

/// The listing position of group `g`.
fn pos(l: &Listing, g: usize) -> usize {
    l.order.iter().position(|&x| x == g).unwrap_or(0)
}
