//! Durable rejections (sub-project 5 spec §8.3; plan 5E Decisions 1–5): `Store::drop` appends
//! a `drop` record on the target through the one write path, so the write guard applies inside
//! the transaction; `Store::dropped` answers what is dropped for a branch, from the `drops`
//! index along its lineage ([`crate::drops_index`]).
//!
//! Every id a drop names in `drops` must be the origin root of a record a merge of `from` into
//! `into` would still copy (`fin_core::history::check_drop`; Decision 15: a rejection is of an
//! entry not yet merged): never the target's own records, what is already merged or picked
//! there, or an unknown id. Every id in `undrops` must be dropped for `into` now, whatever
//! became of it on the source since, so a rejection can always be lifted.
//! Like a checkpoint, the record has no ops and is no unit on any undo stack; it is written at
//! record format 3, so the segment that exports it is format 3 (`crate::segment::version_for`).

use std::collections::BTreeSet;

use fin_core::history::{LogView, check_drop, dropped_for};
use fin_core::{CommittedTx, CoreError, Link};

use super::SqlLog;
use crate::branch::create::{check_key, key_reused, metadata_draft};
use crate::branch::index;
use crate::commit::{check_args_hash, same_args_hash};
use crate::db;
use crate::write::WriteCtx;
use crate::{DropReq, Store, StoreError};

impl Store {
    /// Appends a `drop` record on `into` dropping (and undropping) origins of `from` and
    /// returns it.
    ///
    /// Errors, in order: `INVALID_OP` (empty txId, bad `argsHash`, no id, more than
    /// `fin_core::MAX_DROP_IDS`, an id in both lists); a retried txId answers with its record
    /// (`IDEM_KEY_REUSED` when it holds another kind or another `argsHash`);
    /// `PROJECT_READ_ONLY`; `IDEM_KEY_REUSED` when the txId is a merge group's id;
    /// `BRANCH_NOT_FOUND`; `BRANCH_ARCHIVED` (the target); the guard's `BRANCH_PROTECTED`,
    /// `BRANCH_NOT_ALLOWED`; `INVALID_OP {reason, txId, from, into}` for a drop of an id that is
    /// not an origin root `from` would bring into `into` (`notOnSource`) or one already merged
    /// there (`alreadyPresent`), or an undrop of one not dropped for `into` (`notDropped`). A dry run plans it against the local
    /// database and keeps nothing, on a follower too.
    pub fn drop(&mut self, r: DropReq) -> Result<CommittedTx, StoreError> {
        check_key(&r.tx_id)?;
        check_args_hash(r.args_hash.as_deref(), &r.tx_id)?;
        // Validates the lists before anything is read (and the write builds its link again).
        Link::new_drop(r.from.clone(), r.drops.clone(), r.undrops.clone())?;
        loop {
            if let Some(c) = self.write(&r.tx_id, r.dry_run, |ctx| plan(ctx, &r))? {
                return Ok(c);
            }
            // Deduped; gone again (an import rewound it since): the drop runs again.
            if let Some(existing) = db::find_tx(&self.conn, &r.tx_id)? {
                return match existing.link.as_deref() {
                    Some(Link::Drop { .. }) => same_args_hash(r.args_hash.as_deref(), existing),
                    _ => Err(key_reused(&existing)),
                };
            }
        }
    }

    /// The origin roots dropped for `branch` (a reference), sorted
    /// (`fin_core::history::dropped_for`). `BRANCH_NOT_FOUND`.
    pub fn dropped(&self, branch: &str) -> Result<BTreeSet<String>, StoreError> {
        let id = index::resolve(&self.conn, branch)?.id;
        let log = SqlLog::new(&self.conn);
        Ok(dropped_for(&log, &log.lineage(&id)?)?)
    }
}

fn plan(ctx: &mut WriteCtx<'_>, r: &DropReq) -> Result<CommittedTx, StoreError> {
    let txn = ctx.txn;
    super::merge_id_taken(txn, &r.tx_id)?;
    let into = ctx.target(&r.into)?.id;
    let from = index::resolve(txn, &r.from)?.id;
    let link = Link::new_drop(from.clone(), r.drops.clone(), r.undrops.clone())?;
    let Link::Drop { drops, undrops, .. } = &link else {
        unreachable!("new_drop makes a drop link");
    };
    check_drop(&SqlLog::new(txn), &from, &into, drops, undrops)?;
    let mut draft = metadata_draft(ctx, &into, &r.tx_id, &r.author, link.clone())?;
    draft.intent.args_hash = r.args_hash.clone();
    let c = ctx.append(draft)?;
    // What was written reads back as the link it was built from.
    if c.link.as_deref() != Some(&link) {
        return Err(CoreError::new(
            "DB_ERROR",
            format!("the drop record {} does not read back as written", r.tx_id),
        )
        .into());
    }
    Ok(c)
}
