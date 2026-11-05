//! The per-branch loop of [`Store::import`](crate::Store::import): skip check, resolve, rewind,
//! apply.

use std::collections::HashMap;

use fin_core::CommittedTx;
#[cfg(any(test, feature = "test-hooks"))]
use fin_core::CoreError;
use rusqlite::Transaction;

use super::ctx::Ctx;
use super::{Key, Verdict, apply, known, rewind};
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
    for branch in db::branches(txn)? {
        let segs = by_branch.remove(&branch).unwrap_or_default();
        let line = known::line(txn, &branch)?;
        let key = Key {
            segs: segs.iter().map(|s| s.hash().to_string()).collect(),
            line,
            held,
            lease_top,
            kind,
        };
        if let Some((k, v)) = prior.get(&branch)
            && *k == key
        {
            cache.insert(branch, (key, v.clone()));
            continue;
        }
        let mut key = key;
        let known = known::build(txn, &branch, &key.line, ctx.all_tx()?, holds, kind)?;
        let mut res = resolve(&segs, &known);
        resolves += 1;
        let mut rewound = None;
        if let Some(lost) = rewind::eligible(ctx, &branch, &res, held, lease_top, kind)? {
            rewound = Some(rewind::run(ctx, &branch, lost, &segs)?);
            #[cfg(any(test, feature = "test-hooks"))]
            if std::mem::take(&mut *fail_rewind) {
                // Dropping `txn` rolls the rewind back, as a crash before the commit.
                return Err(CoreError::new("TEST_FAULT", "injected failure in a rewind").into());
            }
            key.line = known::line(txn, &branch)?;
            let known = known::build(txn, &branch, &key.line, ctx.all_tx()?, holds, kind)?;
            res = resolve(&segs, &known);
            resolves += 1;
        }
        refresh(txn, heads, &branch)?;
        let head = heads.get(&branch).cloned();
        let mut out = apply::branch(ctx, &branch, &key.line, &segs, &res, head)?;
        out_rewound.extend(rewound.iter().cloned());
        out_rewound.extend(out.rewound.take());
        applied.extend(out.applied);
        if out.changed || rewound.is_some() {
            new_heads.push((branch.clone(), out.head));
        }
        cache.insert(branch, (key, out.verdict));
    }
    Ok(PassOut {
        cache,
        new_heads,
        resolves,
        applied,
        rewound: out_rewound,
    })
}
