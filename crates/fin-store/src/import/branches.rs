//! Imported `branch` and `branchSet` records and the branch index (plan 4B Decisions 14–15,
//! 33), inside the pass's DB transaction, right after the record's row is stored.
//!
//! - A `branch` record that makes a new id adds its branch: the index row and the child's
//!   fork state as its snapshot at `forkSeq` (as [`crate::Store::create_branch`] does), and
//!   the child joins the pass's worklist.
//! - **Id collision** (two servers across a takeover made the same `br<n>`, possibly on
//!   different parents): the higher `epoch`, then the lower `txHash`, holds the id, so every
//!   follower ends with the same holder whatever the arrival order. A record that loses is an
//!   inert row on its parent's chain: it has no ops, the index ignores it, and the parent
//!   keeps following. A record that wins takes the id from the branch applied before: that
//!   branch and its descendants are rewound whole as `ID_COLLISION`
//!   ([`super::cascade::subtree`], [`super::cascade::Cause::voided`]), and the winner's branch
//!   is made in its place. The loser's folder segments name its record (`branchRecord`), so
//!   [`super::aside::leftovers`] sets them aside as `ID_COLLISION`.
//! - A record the index refuses (a name the rules refuse, an unknown branch, a `forkHash`
//!   that is not its parent's record) is an inert row too, never an error of the pass and
//!   never a stop of its parent's chain.
//!
//! Inert records are marked in `record_links.ignored`; status warns
//! (`BRANCH_RECORDS_IGNORED`). [`crate::derived::recompute`] makes the same decisions from
//! the log alone.

use fin_core::branch::Absorbed;
use fin_core::{Project, Record, RecordKind};
use rusqlite::params;

use super::cascade::{self, Cause};
use super::ctx::Ctx;
use super::quarantine::ID_COLLISION;
use crate::StoreError;
use crate::branch::{index, lineage};
use crate::db::{self, to_i64};

/// Folds the record just stored at `seq` of `branch` (its bytes hash to `tx_hash`) into the
/// index (module docs). `before` is `branch`'s state at `seq - 1`.
pub(super) fn fold(
    ctx: &mut Ctx,
    branch: &str,
    seq: u64,
    rec: &Record,
    tx_hash: &str,
    before: &Project,
) -> Result<(), StoreError> {
    if !matches!(rec.kind, RecordKind::Branch | RecordKind::BranchSet) {
        return Ok(());
    }
    let txn = ctx.txn;
    let ix = index::load(txn)?;
    let mut next = ix.clone();
    let (id, replaced) = match next.absorb(branch, seq, rec, tx_hash) {
        Err(e) => return index::set_ignored(txn, tx_hash, Some(&index::unfit(&e.message))),
        Ok(Absorbed::Nothing) => return Ok(()),
        Ok(Absorbed::Set(id)) => {
            let info = next.get(&id).ok_or_else(|| index::missing(&id))?;
            return index::save(txn, info);
        }
        Ok(Absorbed::Lost { id, winner }) => {
            return index::set_ignored(txn, tx_hash, Some(&index::collision(&id, &winner)));
        }
        Ok(Absorbed::Made(id)) => (id, false),
        Ok(Absorbed::Replaced { id, .. }) => (id, true),
    };
    let info = next.get(&id).cloned().ok_or_else(|| index::missing(&id))?;
    if let Some(why) = index::bad_fork(txn, &ix, &info)? {
        return index::set_ignored(txn, tx_hash, Some(&index::unfit(&why)));
    }
    let state = if info.fork_seq + 1 == seq {
        before.clone()
    } else {
        lineage::state_at(txn, branch, info.fork_seq)?
    };
    if replaced {
        // The branch that held the id goes, with its descendants; recomputing the index then
        // gives the id to this record (its row is stored) and marks the old one inert.
        let lost = cascade::subtree(txn, &id)?;
        let dirs = std::mem::take(&mut ctx.dirs);
        let cause = Cause::voided("idCollision", info.epoch, ID_COLLISION);
        let rewound = cascade::cascade(ctx, &dirs, &lost, &cause);
        ctx.dirs = dirs;
        ctx.rewound.extend(rewound?);
    } else {
        index::save(txn, &info)?;
    }
    born(ctx, &id, info.fork_seq, &state)
}

/// Branch `id` starts (again) here: its fork state is stored as its snapshot at `fork_seq`,
/// the rewinds recorded under the id are forgotten (they were another branch's; its
/// generation stays, so a caller holding the old one is refused, `crate::history`), and it
/// joins the pass's worklist.
fn born(ctx: &mut Ctx, id: &str, fork_seq: u64, state: &Project) -> Result<(), StoreError> {
    let txn = ctx.txn;
    txn.execute(
        "DELETE FROM snapshots WHERE branch = ?1 AND seq = ?2",
        params![id, to_i64(fork_seq)?],
    )?;
    db::insert_snapshot(txn, id, fork_seq, state)?;
    crate::history::forget_rewinds(txn, id)?;
    if !ctx.born.iter().any(|b| b == id) {
        ctx.born.push(id.to_string());
    }
    Ok(())
}

/// After [`crate::derived::recompute`]: every branch the index holds without any state (its
/// `branch` record holds the id again because the record that took it is gone, re-review of
/// 9b) gets its fork state back and joins the worklist, parents first, so the same pass
/// resolves it from the folder as a follower that saw everything at once would.
pub(super) fn restore_stateless(ctx: &mut Ctx) -> Result<(), StoreError> {
    let ix = index::load(ctx.txn)?;
    for id in ix.parents_first() {
        let Some(info) = ix.get(id) else { continue };
        let Some(parent) = info.parent.as_deref() else {
            continue;
        };
        if db::latest_rev(ctx.txn, id)?.is_some() {
            continue;
        }
        let state = lineage::state_at(ctx.txn, parent, info.fork_seq)?;
        born(ctx, id, info.fork_seq, &state)?;
    }
    Ok(())
}
