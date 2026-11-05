//! Making, renaming, archiving and restoring branches (spec §4.1; plan 4B Decisions 3 and
//! 11–14), each one record on the parent's log through the one write path
//! ([`Store::write`]): a `branch` record makes a branch, a `branchSet` record renames,
//! archives or restores it. Both carry no ops. The index row follows from the record
//! ([`super::index::absorb`]); a new branch also gets the parent's state at its fork as its
//! own snapshot there, so its head is the fork seq until its first record.

use std::collections::BTreeMap;

use fin_core::branch::{BranchIndex, BranchInfo, id_number, validate_branch_name};
use fin_core::{
    CommittedTx, CoreError, Diff, IdAlloc, IdKind, Link, SCHEMA_VERSION, Tx, kind_name,
};
use serde_json::json;

use super::{index, lineage};
use crate::db::{self, Counters};
use crate::write::{Draft, WriteCtx, writable};
use crate::{BranchCreated, BranchView, Store, StoreError};

/// Where a new branch forks from its parent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum At {
    /// The parent's seq (at most its head).
    Seq(u64),
    /// A checkpoint on the parent (Task 15; until then `NOT_FOUND`, `details.kind`
    /// `checkpoint`).
    Checkpoint(String),
}

/// [`Store::create_branch`]'s request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCreate {
    /// The idempotency key: the `branch` record's txId.
    pub tx_id: String,
    pub author: String,
    /// The display name; `None`: `try-<n>` for the new id `br<n>`.
    pub name: Option<String>,
    /// The parent, as a branch reference.
    pub from: String,
    /// The fork point; `None`: the parent's head.
    pub at: Option<At>,
    pub dry_run: bool,
}

/// [`Store::set_branch`]'s request: a new name, archive (`true`) or restore (`false`), or
/// both (`fin branch archive <b> --restore --as <new>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchSet {
    /// The idempotency key: the `branchSet` record's txId.
    pub tx_id: String,
    pub author: String,
    /// The branch, as a reference (an archived one by id or `<name>#<id>`).
    pub branch: String,
    pub name: Option<String>,
    pub archived: Option<bool>,
    pub dry_run: bool,
}

impl Store {
    /// Every branch (with `all`, archived ones too): `main` first, then by numeric id.
    pub fn branches(&self, all: bool) -> Result<Vec<BranchView>, StoreError> {
        let ix = index::load(&self.conn)?;
        Ok(ix
            .list()
            .into_iter()
            .filter(|(_, i)| all || !i.archived)
            .map(|(display, i)| view(i, display, self.head_of(&i.id)))
            .collect())
    }

    /// The id branch reference `r` names (Decision 12): an id, the display name of a
    /// non-archived branch (case-insensitively), or `<name>#<id>`. `BRANCH_NOT_FOUND`
    /// otherwise.
    pub fn resolve_branch(&self, r: &str) -> Result<String, StoreError> {
        Ok(index::resolve(&self.conn, r)?.id)
    }

    /// Makes a branch: the next `br<n>`, forking from `from` at `at` (default: its head), by a
    /// `branch` record appended at the parent's head; returns the branch (head = its fork
    /// seq) and the record.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId), `INVALID_NAME`; then as a commit: a
    /// retried txId answers with the branch it made (`IDEM_KEY_REUSED` when that txId holds
    /// another kind of record), `PROJECT_READ_ONLY`; `BRANCH_NOT_FOUND`, `BRANCH_ARCHIVED`
    /// (the parent), `INVALID_OP` (`at` past the parent's head), `NOT_FOUND` (a checkpoint),
    /// `NAME_TAKEN` (an active or merged branch holds the name). A dry run plans it against
    /// the local database and keeps nothing, on a follower too (Decision 26).
    pub fn create_branch(&mut self, r: BranchCreate) -> Result<BranchCreated, StoreError> {
        check_key(&r.tx_id)?;
        if let Some(name) = &r.name {
            validate_branch_name(name)?;
        }
        loop {
            if let Some(c) = self.write(&r.tx_id, r.dry_run, |ctx| plan_create(ctx, &r))? {
                return Ok(c);
            }
            // Deduped; gone again (an import rewound it since): the create runs again.
            if let Some(existing) = db::find_tx(&self.conn, &r.tx_id)? {
                return self.created(existing);
            }
        }
    }

    /// Renames, archives or restores a branch by a `branchSet` record on its parent's log
    /// (the parent may be archived). Returns the record.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, nothing to set), `INVALID_NAME`; then as a
    /// commit (dedupe, `PROJECT_READ_ONLY`); `BRANCH_NOT_FOUND`; `INVALID_OP` for `main` (it
    /// is never renamed or archived, Decision 13); `NAME_TAKEN` when the branch would be
    /// active under a name another non-archived branch holds (restoring one: `fix` names
    /// `--restore --as <new>`).
    pub fn set_branch(&mut self, r: BranchSet) -> Result<CommittedTx, StoreError> {
        check_key(&r.tx_id)?;
        if r.name.is_none() && r.archived.is_none() {
            return Err(CoreError::new("INVALID_OP", "nothing to set on the branch")
                .with_details(json!({ "branch": r.branch, "txId": r.tx_id }))
                .with_fix("give a new name, archive or restore")
                .into());
        }
        if let Some(name) = &r.name {
            validate_branch_name(name)?;
        }
        loop {
            if let Some(c) = self.write(&r.tx_id, r.dry_run, |ctx| plan_set(ctx, &r))? {
                return Ok(c);
            }
            if let Some(existing) = db::find_tx(&self.conn, &r.tx_id)? {
                return match existing.link.as_deref() {
                    Some(Link::BranchSet { .. }) => Ok(existing),
                    _ => Err(key_reused(&existing)),
                };
            }
        }
    }

    /// The answer to a retried create: the branch its record made, as it is now.
    fn created(&self, existing: CommittedTx) -> Result<BranchCreated, StoreError> {
        let Some(Link::Branch { branch_id, .. }) = existing.link.as_deref() else {
            return Err(key_reused(&existing));
        };
        let ix = index::load(&self.conn)?;
        let info = ix.get(branch_id).ok_or_else(|| index::missing(branch_id))?;
        Ok(BranchCreated {
            branch: view(info, ix.display(info), self.head_of(&info.id)),
            record: existing,
            dry_run: false,
        })
    }
}

fn view(info: &BranchInfo, display: String, head: u64) -> BranchView {
    BranchView {
        id: info.id.clone(),
        name: info.name.clone(),
        display,
        parent: info.parent.clone(),
        fork_seq: info.fork_seq,
        head,
        state: info.state(),
        created_by: info.created_by.clone(),
    }
}

fn check_key(tx_id: &str) -> Result<(), CoreError> {
    if tx_id.trim().is_empty() {
        return Err(CoreError::new("INVALID_OP", "the request's txId is empty")
            .with_details(json!({ "field": "txId" }))
            .with_fix("send a non-empty txId (a ULID, or the caller's idempotency key)"));
    }
    Ok(())
}

fn key_reused(existing: &CommittedTx) -> StoreError {
    let kind = existing
        .link
        .as_deref()
        .map_or_else(|| "edit".to_string(), |l| kind_name(l.kind()));
    CoreError::new(
        "IDEM_KEY_REUSED",
        format!(
            "txId {} already committed a record of kind {kind} at rev {} on {}",
            existing.tx.tx_id, existing.rev, existing.tx.branch
        ),
    )
    .with_details(json!({
        "txId": existing.tx.tx_id,
        "rev": existing.rev,
        "branch": existing.tx.branch,
        "kind": kind,
    }))
    .with_fix("use a new idempotency key (txId) for a different command")
    .into()
}

fn name_taken(name: &str, ix: &BranchIndex, holder: &BranchInfo, fix: String) -> StoreError {
    CoreError::new(
        "NAME_TAKEN",
        format!(
            "branch name {name:?} is taken by branch {}",
            ix.display(holder)
        ),
    )
    .with_details(json!({ "name": name, "branch": holder.id }))
    .with_fix(fix)
    .into()
}

/// A no-op record (`branch`, `branchSet`) on `branch` at its head.
fn metadata_draft(
    ctx: &mut WriteCtx<'_>,
    branch: &str,
    tx_id: &str,
    author: &str,
    link: Link,
) -> Result<Draft, StoreError> {
    let head = ctx.head(branch)?;
    Ok(Draft {
        branch: branch.to_string(),
        link,
        intent: Tx {
            tx_id: tx_id.to_string(),
            author: author.to_string(),
            branch: branch.to_string(),
            base_rev: head.rev,
            schema_version: SCHEMA_VERSION,
            ops: vec![],
            args_hash: None,
        },
        inverse: vec![],
        bindings: BTreeMap::new(),
        diff: Diff::default(),
        project: head.project.clone(),
    })
}

fn plan_create(ctx: &mut WriteCtx<'_>, r: &BranchCreate) -> Result<BranchCreated, StoreError> {
    let txn = ctx.txn;
    let parent = writable(txn, &r.from)?.id;
    let head = ctx.head(&parent)?;
    let head_rev = head.rev;
    let fork_seq = match &r.at {
        None => head_rev,
        Some(At::Seq(seq)) => *seq,
        Some(At::Checkpoint(name)) => {
            return Err(CoreError::new(
                "NOT_FOUND",
                format!("no checkpoint {name:?} on branch {parent}"),
            )
            .with_details(json!({ "kind": "checkpoint", "name": name, "branch": parent }))
            .with_fix("fork at a seq (--from <branch>@<seq>)")
            .into());
        }
    };
    if fork_seq > head_rev {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("branch {parent} has no seq {fork_seq} to fork at (its head is {head_rev})"),
        )
        .with_details(json!({ "branch": parent, "at": fork_seq, "head": head_rev }))
        .with_fix(format!("fork at a seq from 0 to {head_rev}"))
        .into());
    }
    let fork_state = if fork_seq == head_rev {
        head.project.clone()
    } else {
        let project = head.project.clone();
        lineage::state_near_head(txn, &parent, fork_seq, head_rev, &project)?
    };
    let fork_hash = lineage::hash_at(txn, &parent, fork_seq)?;
    let mut ids = Counters::load(txn)?;
    let id = ids.next(IdKind::Branch);
    ids.save(txn)?;
    let name = match &r.name {
        Some(name) => name.clone(),
        None => format!("try-{}", id_number(&id).unwrap_or_default()),
    };
    let ix = index::load(txn)?;
    if let Some(holder) = ix.holder(&name) {
        let fix = format!(
            "choose another name, or rename or archive branch {} first",
            ix.display(holder)
        );
        return Err(name_taken(&name, &ix, holder, fix));
    }
    let link = Link::Branch {
        branch_id: id.clone(),
        name,
        fork_seq,
        fork_hash,
    };
    let draft = metadata_draft(ctx, &parent, &r.tx_id, &r.author, link)?;
    let record = ctx.append(draft)?;
    db::insert_snapshot(txn, &id, fork_seq, &fork_state)?;
    let ix = index::load(txn)?;
    let info = ix.get(&id).ok_or_else(|| index::missing(&id))?;
    Ok(BranchCreated {
        branch: view(info, ix.display(info), fork_seq),
        record,
        dry_run: ctx.dry_run,
    })
}

fn plan_set(ctx: &mut WriteCtx<'_>, r: &BranchSet) -> Result<CommittedTx, StoreError> {
    let txn = ctx.txn;
    let info = index::resolve(txn, &r.branch)?;
    let Some(parent) = info.parent.clone() else {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("branch {} cannot be renamed or archived", info.id),
        )
        .with_details(json!({ "branch": info.id }))
        .with_fix("make a branch from it instead (fin branch new <name>)")
        .into());
    };
    let archived = r.archived.unwrap_or(info.archived);
    let name = r.name.as_deref().unwrap_or(&info.name);
    let ix = index::load(txn)?;
    if !archived && let Some(holder) = ix.holder(name).filter(|h| h.id != info.id) {
        let fix = if info.archived && r.name.is_none() {
            format!("fin branch archive {} --restore --as <new>", info.id)
        } else {
            format!(
                "choose another name, or rename or archive branch {} first",
                ix.display(holder)
            )
        };
        return Err(name_taken(name, &ix, holder, fix));
    }
    let link = Link::BranchSet {
        branch_id: info.id.clone(),
        name: r.name.clone(),
        archived: r.archived,
    };
    let draft = metadata_draft(ctx, &parent, &r.tx_id, &r.author, link)?;
    ctx.append(draft)
}
