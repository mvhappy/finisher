//! `Store::commit`: dedupe by `txId`, rebase a stale tx over what was committed since its base,
//! apply it, and append it in one SQLite transaction together with the id counters it used,
//! through the shared write path ([`Store::write`]).

use std::time::{SystemTime, UNIX_EPOCH};

use fin_core::{CommittedTx, CoreError, Link, Tx, apply_tx, rebase_tx};
use serde_json::{Value, json};

use crate::branch::lineage;
use crate::db::{self, Counters};
use crate::write::{Draft, WriteCtx};
use crate::{Generation, Store, StoreError};

impl Store {
    /// Commits `tx` to its branch and returns it as committed.
    ///
    /// - An empty `txId` or empty `ops` is `INVALID_OP`, checked before anything else.
    /// - `tx.branch` is a branch reference (an id, an active name or `<name>#<id>`); the
    ///   stored intent holds the id. An unknown one is `BRANCH_NOT_FOUND`, an archived one
    ///   `BRANCH_ARCHIVED` (both checked after the dedupe, the read-only state and the fence).
    /// - A `txId` that is already committed returns that `CommittedTx` unchanged (idempotent
    ///   retries); the head does not move. When both carry an `argsHash` and they differ, the
    ///   key was reused for another command: `IDEM_KEY_REUSED` (`details {txId, rev,
    ///   committedOps}`). A malformed `argsHash` (not `blake3:<64 hex>`) is `INVALID_OP`.
    /// - `baseRev` behind the head: the tx is rebased over the txs committed since, in the
    ///   frame of the state at `baseRev` (`fin_core::rebase_tx`); a conflict is `CONFLICT` with
    ///   `details.txId` (incoming), `details.againstTx` (the committed tx to blame, or null when
    ///   the re-run at the head failed) and `details.conflicts`. A tx that does not apply at
    ///   its own base fails with that error, unless the op that failed there (or one before
    ///   it) conflicts with what was committed since.
    /// - `baseRev` ahead of the head is `BASE_REV_AHEAD`.
    /// - While the project is read-only ([`Store::read_only`]: a fork, a conflict copy, a
    ///   newer format, an import error, the lease held elsewhere or lost, or not yet caught up
    ///   after acquiring it): `PROJECT_READ_ONLY` with `details.reason` and `fix`.
    /// - Without a held lease: `PROJECT_READ_ONLY` with `details.reason` `LEASE_LOST`. The full
    ///   fence (`lease.check()`, folder I/O) runs before the database lock is taken; under the
    ///   lock, right before the SQLite commit, only the in-memory part
    ///   (`lease.quick_check()`). The row records the epoch the fence returned (`txs.epoch`).
    ///
    /// The stored tx is the rebased one (`baseRev` = the head it was applied to), with the diff,
    /// inverse and bindings of that same apply. Ids are allocated from the `ids` table inside the
    /// same SQLite transaction as the `txs` insert, so a failed commit consumes none.
    pub fn commit(&mut self, tx: Tx) -> Result<CommittedTx, StoreError> {
        self.commit_at_generation(tx, None)
    }

    /// [`Store::commit`], for a caller that read the branch at `generation`
    /// ([`Store::generation_token`], [`Store::ops_since_at`]). A generation of another
    /// database (this one was rebuilt since) is refused with `HISTORY_REWOUND`
    /// (`details.reason` `REBUILT`, `rev` 0) whatever its base. When an import has rewound the branch
    /// since then from a rev at or below `baseRev` (the caller built the tx on records that
    /// are gone), the commit is refused with `HISTORY_REWOUND` (`details {branch, rev,
    /// generation, expected, baseRev}`) instead of being rebased onto other history; a
    /// `baseRev` before every such rewind rebases as usual. `None` skips the check. Checked
    /// after the dedupe, the read-only state and the fence, under the database lock.
    ///
    /// A superseded tx is in quarantine, not in `txs`, so its `txId` is not deduped: a retry
    /// of it whose `baseRev` is below the fork (or one sent without a generation) is committed
    /// again, rebased on top of the winning line. Callers that must not re-apply an edit the
    /// project lost pass the generation, or ask [`Store::tx_status`] first.
    pub fn commit_at_generation(
        &mut self,
        tx: Tx,
        generation: Option<&Generation>,
    ) -> Result<CommittedTx, StoreError> {
        self.commit_in(tx, generation, false)
    }

    /// [`Store::commit_at_generation`], or with `dry_run` its plan: the commit as it would be
    /// made, against the local database, rolled back, with no read-only or lease check (so on
    /// a follower too; Decision 26).
    pub(crate) fn commit_in(
        &mut self,
        tx: Tx,
        generation: Option<&Generation>,
        dry_run: bool,
    ) -> Result<CommittedTx, StoreError> {
        check_shape(&tx)?;
        // Everything in `plan` runs under the write lock, against the head as stored: another
        // `Store` on this database may have committed since our cache was filled.
        loop {
            if let Some(c) = self.write(&tx.tx_id, dry_run, |ctx| plan(ctx, &tx, generation))? {
                return Ok(c);
            }
            // Deduped: answer with the committed tx. Gone again (rewound by an import between
            // the two reads): it is not committed any more, so the commit runs again.
            if let Some(existing) = db::find_tx(&self.conn, &tx.tx_id)? {
                return same_args(&tx, existing);
            }
        }
    }
}

/// The commit of `tx` inside a write: the head, the generation and base checks, the rebase of
/// a stale tx (or the apply of a fresh one), the ids it used, and the record.
fn plan(
    ctx: &mut WriteCtx<'_>,
    tx: &Tx,
    generation: Option<&Generation>,
) -> Result<CommittedTx, StoreError> {
    let txn = ctx.txn;
    // The branch by id (the stored intent holds it); an archived one takes no commits.
    let tx = &Tx {
        branch: crate::write::writable(txn, &tx.branch)?.id,
        ..tx.clone()
    };
    // One head for the whole plan (`txn` is a copy of the reference, so it reads beside it).
    let head = ctx.head(&tx.branch)?;
    if let Some(g) = generation {
        crate::history::check(txn, &tx.branch, g, tx.base_rev)?;
    }
    if tx.base_rev > head.rev {
        return Err(CoreError::new("BASE_REV_AHEAD", format!("baseRev {} is ahead of head {}", tx.base_rev, head.rev))
            .with_details(json!({ "txId": tx.tx_id, "baseRev": tx.base_rev, "head": head.rev, "branch": tx.branch }))
            .with_fix("read the project head and send its rev as baseRev")
            .into());
    }
    let mut ids = Counters::load(txn)?;
    // A stale tx is rebased by the engine in the frame of its base: the tx applied there
    // gives the maps of its own ops, and the re-run at the head is the apply. Below a
    // branch's fork, the frame and the records since come from its ancestors.
    let (tx, applied) = if tx.base_rev < head.rev {
        // `records_since` fills pending diffs first (a seeded rebuild), and the snapshots
        // that writes then serve `state_near_head`.
        let since = lineage::records_since(txn, &tx.branch, tx.base_rev)?;
        let base = lineage::state_near_head(txn, &tx.branch, tx.base_rev, head.rev, &head.project)?;
        let rebased = rebase_tx(&base, &head.project, tx, &since, &mut ids)
            .map_err(|e| with_tx_id(e, &tx.tx_id))?;
        (rebased.tx, rebased.applied)
    } else {
        (tx.clone(), apply_tx(&head.project, tx, &mut ids)?)
    };
    ids.save(txn)?;
    ctx.append(Draft {
        branch: tx.branch.clone(),
        link: Link::Edit,
        intent: tx,
        inverse: applied.inverse,
        bindings: applied.bindings,
        diff: applied.diff,
        project: applied.project,
    })
}

pub(crate) fn now_ms() -> i64 {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    i64::try_from(ms).unwrap_or(i64::MAX)
}

fn with_tx_id(mut e: CoreError, tx_id: &str) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("txId".to_string(), json!(tx_id));
    }
    e
}

/// `existing`, the commit `tx`'s id names, unless both hold an `argsHash` and they differ.
fn same_args(tx: &Tx, existing: CommittedTx) -> Result<CommittedTx, StoreError> {
    match (&tx.args_hash, &existing.tx.args_hash) {
        (Some(ours), Some(theirs)) if ours != theirs => Err(CoreError::new(
            "IDEM_KEY_REUSED",
            format!(
                "txId {} already committed a different command at rev {}",
                existing.tx.tx_id, existing.rev
            ),
        )
        .with_details(json!({
            "txId": existing.tx.tx_id,
            "rev": existing.rev,
            "committedOps": existing.tx.ops,
        }))
        .with_fix("use a new idempotency key (txId) for a different command")
        .into()),
        _ => Ok(existing),
    }
}

/// A tx needs an id to dedupe on and at least one op to be worth a rev.
fn check_shape(tx: &Tx) -> Result<(), CoreError> {
    if tx.tx_id.trim().is_empty() {
        return Err(CoreError::new("INVALID_OP", "transaction txId is empty")
            .with_details(json!({ "field": "txId" }))
            .with_fix("send a non-empty txId (a ULID, or the caller's idempotency key)"));
    }
    if tx.ops.is_empty() {
        return Err(CoreError::new("INVALID_OP", "transaction has no ops")
            .with_details(json!({ "field": "ops", "txId": tx.tx_id }))
            .with_fix("send at least one op"));
    }
    if let Some(h) = &tx.args_hash {
        let hex = h.strip_prefix("blake3:").unwrap_or_default();
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(CoreError::new(
                "INVALID_OP",
                "transaction argsHash is not blake3:<64 lowercase hex>",
            )
            .with_details(json!({ "field": "argsHash", "txId": tx.tx_id }))
            .with_fix(
                "send the blake3 hash of the command input as blake3:<hex>, or no argsHash",
            ));
        }
    }
    Ok(())
}
