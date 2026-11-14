//! Replay of committed history (sub-project 4 spec §2 rule 2, §3.2): a record's `intent` is
//! applied again with its recorded bindings feeding id allocation. Only structural checks run;
//! a rule added later never rejects an existing project.

use std::collections::{BTreeMap, VecDeque};

use serde_json::json;

use crate::apply::{ApplyMode, apply_tx_mode};
use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind, suffix};
use crate::model::Project;
use crate::ops::Op;
use crate::tx::{Applied, Tx};

/// Hands out a committed transaction's bound ids again, in placeholder (op) order, so replay
/// reproduces the same ids. Never invents an id: a request of the wrong kind or past the end
/// is recorded and reported by [`BoundIds::finished`] being false.
#[derive(Debug, Clone)]
pub struct BoundIds<'a> {
    queue: VecDeque<&'a str>,
    mismatch: bool,
}

impl<'a> BoundIds<'a> {
    /// The ids `bindings` gives the placeholders that `ops` allocate, in op order. A placeholder
    /// without a binding is `UNBOUND_PLACEHOLDER`.
    pub fn new(ops: &[Op], bindings: &'a BTreeMap<String, String>) -> Result<Self, CoreError> {
        // A placeholder is bound by the first op that names it (a later `keyframe.set` of it
        // edits that key).
        let mut seen = std::collections::BTreeSet::new();
        let queue = ops
            .iter()
            .enumerate()
            .filter_map(|(i, op)| op.allocates().map(|p| (i, p)))
            .filter(|(_, p)| seen.insert(*p))
            .map(|(i, p)| {
                bindings.get(p).map(String::as_str).ok_or_else(|| {
                    CoreError::new(
                        "UNBOUND_PLACEHOLDER",
                        format!("placeholder {p} has no recorded binding"),
                    )
                    .with_details(json!({ "placeholder": p, "opIndex": i }))
                })
            })
            .collect::<Result<_, _>>()?;
        Ok(BoundIds {
            queue,
            mismatch: false,
        })
    }

    /// Every bound id was handed out, each for its own kind.
    pub fn finished(&self) -> bool {
        !self.mismatch && self.queue.is_empty()
    }
}

impl IdAlloc for BoundIds<'_> {
    fn next(&mut self, kind: IdKind) -> String {
        match self.queue.pop_front() {
            Some(id) => {
                if suffix(id, kind.prefix()).is_none() {
                    self.mismatch = true;
                }
                id.to_string()
            }
            None => {
                self.mismatch = true;
                String::new()
            }
        }
    }
}

/// `intent` (a committed transaction) applied again to `project` in [`ApplyMode::Replay`],
/// with `bindings` fixing every new id. The result's bindings equal `bindings`.
///
/// Fails only on structural damage, as `REPLAY_FAILED` with `details {txId, cause}`; `cause`
/// is the structural error (`NOT_FOUND`, `DUPLICATE_ID`, `INVALID_ID`, `TRACK_KIND_MISMATCH`,
/// `ASSET_IN_USE`, `CLIP_OVERLAP`, `CLIP_EMPTY`, `CLIP_NEGATIVE`, `INVALID_RANGE` for a
/// non-positive asset or transition duration, `UNBOUND_PLACEHOLDER`, `DUPLICATE_PLACEHOLDER`,
/// `TIME_OVERFLOW`, and 4C's `NOT_ADJACENT`, `OVERWRITE_SPLITS_ITEM`, `SLIDE_BLOCKED`,
/// `MOVE_INTO_ITSELF`, `TRANSITION_INVALID`), or
/// `BINDINGS_MISMATCH` when the recorded bindings are not the ones the intent allocates.
pub fn apply_committed(
    project: &Project,
    intent: &Tx,
    bindings: &BTreeMap<String, String>,
) -> Result<Applied, CoreError> {
    let failed = |cause: CoreError| {
        CoreError::new(
            "REPLAY_FAILED",
            format!(
                "committed tx {} does not replay: {}",
                intent.tx_id, cause.message
            ),
        )
        .with_details(json!({ "txId": intent.tx_id, "cause": cause }))
        .with_fix("the stored history is damaged; restore the project from a backup")
    };
    let mut ids = BoundIds::new(&intent.ops, bindings).map_err(failed)?;
    let applied = apply_tx_mode(project, intent, &mut ids, ApplyMode::Replay).map_err(failed)?;
    if !ids.finished() || applied.bindings != *bindings {
        return Err(failed(
            CoreError::new(
                "BINDINGS_MISMATCH",
                "the recorded bindings are not the ids the transaction allocates",
            )
            .with_details(json!({ "recorded": bindings, "allocated": applied.bindings })),
        ));
    }
    Ok(applied)
}
