//! The per-branch loop of [`Store::import`](crate::Store::import): skip check, resolve, rewind,
//! apply.
//!
//! **Worklist** (plan 4B Decision 15): the branches the index knows, parents first (depth,
//! then numeric id; [`fin_core::branch::BranchIndex::parents_first`]), then any branch with
//! rows but no index row (a root). A `branch` record applied on the way adds its child at the
//! end, so a child whose segments arrived with its parent's record is followed in the same
//! pass. A 4B branch is resolved from the segments that name its `branch` record
//! (`branchRecord`, Decision 33); every other segment is classified after the worklist
//! ([`super::aside::leftovers`]).

use std::collections::{HashMap, HashSet, VecDeque};

use fin_core::CommittedTx;
#[cfg(any(test, feature = "test-hooks"))]
use fin_core::CoreError;
use rusqlite::Transaction;

use super::ctx::Ctx;
use super::{Key, Verdict, apply, cascade, known, rewind};
use crate::branch::index;
use crate::chain::{SegRef, resolve};
use crate::storage_kind::StorageKind;
use crate::{Head, Rewind, StoreError, db, refresh};

/// What [`resolve_branches`] hands back to the pass.
pub(super) struct PassOut {
    pub(super) cache: HashMap<String, (Key, Verdict)>,
    pub(super) new_heads: Vec<(String, Option<Head>)>,
    pub(super) resolves: u64,
    pub(super) applied: Vec<CommittedTx>,
    pub(super) rewound: Vec<Rewind>,
    /// Every folder segment by branch directory (the leftovers are classified from it).
    pub(super) dirs: HashMap<String, Vec<SegRef>>,
}

/// Resolves and applies every branch the DB knows, inside the pass's transaction.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolve_branches(
    txn: &Transaction,
    ctx: &mut Ctx,
    prior: &HashMap<String, (Key, Verdict)>,
    #[cfg(any(test, feature = "test-hooks"))] fail_rewind: &mut bool,
    heads: &mut HashMap<String, Head>,
    by_branch: &mut HashMap<String, Vec<SegRef>>,
    held: Option<u64>,
    holds: bool,
    lease_top: Option<u64>,
    kind: StorageKind,
) -> Result<PassOut, StoreError> {
    let mut cache = HashMap::new();
    let mut new_heads: Vec<(String, Option<Head>)> = vec![];
    let mut resolves = 0;
    let mut applied = vec![];
    let mut out_rewound = vec![];
    let ix = index::load(txn)?;
    let mut work: VecDeque<String> = ix.parents_first().into_iter().map(String::from).collect();
    for b in db::branches(txn)? {
        if ix.get(&b).is_none() {
            work.push_back(b);
        }
    }
    ctx.dirs = by_branch.clone();
    let mut seen = HashSet::new();
    let mut born_now = HashSet::new();
    while let Some(branch) = work.pop_front() {
        if seen.contains(&branch) || db::latest_rev(txn, &branch)?.is_none() {
            // An index row without any state is damage; its directory waits (leftover). A
            // branch a cascade took out has none either; it is not marked seen, so a `branch`
            // record later in the pass that makes the same id again is followed.
            continue;
        }
        seen.insert(branch.clone());
        // A 4B branch takes the segments that name its `branch` record (Decision 33); the
        // others are classified after the worklist (`aside::leftovers`).
        let record = index::get(txn, &branch)?.and_then(|i| i.record);
        let segs: Vec<SegRef> = by_branch
            .get(&branch)
            .map(|all| {
                all.iter()
                    .filter(|s| record.is_none() || s.header().branch_record == record)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let line = known::line(txn, &branch)?;
        let fork = known::fork_point(txn, &branch)?;
        let key = Key {
            segs: segs.iter().map(|s| s.hash().to_string()).collect(),
            line,
            held,
            lease_top,
            kind,
            fork: fork.clone(),
            record,
        };
        if let Some((k, v)) = prior.get(&branch)
            && *k == key
        {
            cache.insert(branch, (key, v.clone()));
            continue;
        }
        let mut key = key;
        let build = |ctx: &mut Ctx, line: &known::Line| -> Result<_, StoreError> {
            known::build(txn, &branch, line, ctx.all_tx()?, holds, kind, fork.clone())
        };
        let mut res = resolve(&segs, &build(ctx, &key.line)?);
        resolves += 1;
        let mut rewound = vec![];
        if let Some(lost) = rewind::eligible(ctx, &branch, &res, held, lease_top, kind)? {
            rewound.push(rewind::run(ctx, &branch, &lost, &segs)?);
            let cause = cascade::Cause::applied_line(lost.winner_epoch());
            rewound.extend(cascade::cascade(ctx, by_branch, lost.children(), &cause)?);
            #[cfg(any(test, feature = "test-hooks"))]
            if std::mem::take(&mut *fail_rewind) {
                // Dropping `txn` rolls the rewind back, as a crash before the commit.
                return Err(CoreError::new("TEST_FAULT", "injected failure in a rewind").into());
            }
            key.line = known::line(txn, &branch)?;
            res = resolve(&segs, &build(ctx, &key.line)?);
            resolves += 1;
        }
        let head = if born_now.contains(&branch) {
            // Made by this pass: its fork snapshot is uncommitted, so it stays off the cache.
            let mut fresh = HashMap::new();
            refresh(txn, &mut fresh, &branch)?;
            fresh.remove(&branch)
        } else {
            refresh(txn, heads, &branch)?;
            heads.get(&branch).cloned()
        };
        let mut out = apply::branch(ctx, &branch, &key.line, &segs, &res, head, by_branch)?;
        let changed = out.changed || !rewound.is_empty() || born_now.contains(&branch);
        rewound.append(&mut out.rewound);
        // Branches an imported `branch` record took the id from (`branches::fold`).
        rewound.append(&mut ctx.rewound);
        // A cascaded branch leaves the head cache (a later record may make its id again).
        for w in rewound.iter().filter(|w| w.branch != branch) {
            new_heads.push((w.branch.clone(), None));
        }
        out_rewound.append(&mut rewound);
        applied.extend(out.applied);
        if changed {
            new_heads.push((branch.clone(), out.head));
        }
        for child in std::mem::take(&mut ctx.born) {
            // A branch made again (its id taken by a winning record) is resolved again.
            seen.remove(&child);
            born_now.insert(child.clone());
            work.push_back(child);
        }
        cache.insert(branch, (key, out.verdict));
    }
    Ok(PassOut {
        cache,
        new_heads,
        resolves,
        applied,
        rewound: out_rewound,
        dirs: std::mem::take(by_branch),
    })
}
