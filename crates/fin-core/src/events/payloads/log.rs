//! `log.commit` (Decision 22): one entry per attempt to append to the op log, classified in
//! `fin-node` from typed results and the store's write note. Undo, redo and revert entries are
//! the rejection signal: `links` names the original txIds.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Counts, TxIntent};
use crate::events::approx::{ApproxBytes, approx_struct};
use crate::events::class::{Code, Content, IdRef, Name};
use crate::events::intent::MAX_INTENT_ENTRY_BYTES;
use crate::events::names;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LogCommit {
    pub record: CommitRecord,
    pub outcome: CommitOutcome,
    // The error code (`refused`, `conflict`) or the `noOp` reason. Field docs on a `$ref`
    // field would inline its type in the generated TypeScript, so these are plain comments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    /// Op kind → count (`OP_KINDS`).
    #[serde(
        default,
        skip_serializing_if = "Counts::is_empty",
        deserialize_with = "names::op_counts"
    )]
    pub ops: Counts<Name>,
    /// The request's `baseRev` (edits only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub base_rev: Option<u64>,
    /// Committed `baseRev` − request `baseRev`, when `rebased`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub rebased_over: Option<u64>,
    /// Conflict code → count, from `CONFLICT`'s `details.conflicts`.
    #[serde(default, skip_serializing_if = "Counts::is_empty")]
    pub conflicts: Counts<Code>,
    /// The txIds this record names (an undo's undone units, a merge's copies, a pick's or
    /// revert's target, a recovery's records).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<IdRef>,
    pub micros: u64,
    /// Training only: the intent as committed, or as requested on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<TxIntent>", extend("x-fin-content" = "txIntent"))]
    pub intent: Option<Content<TxIntent>>,
    // Why `intent` is absent although training is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IntentOmitted")]
    pub intent_omitted: Option<IntentOmitted>,
}
approx_struct!(LogCommit {
    record,
    outcome,
    code,
    ops,
    base_rev,
    rebased_over,
    conflicts,
    links,
    micros,
    intent,
    intent_omitted,
});

impl LogCommit {
    /// Sets `intent`, or, when the entry would pass [`MAX_INTENT_ENTRY_BYTES`] with it,
    /// `intentOmitted: tooLarge` (Decision 22), so an oversized intent is dropped before the
    /// entry is enqueued. `rest_bytes` is the rest of the entry as the caller counts it (the
    /// envelope's fields and `ctx`); the payload's own fields are counted here. Call it only
    /// when training is on. Returns whether the intent was kept.
    pub fn attach_intent(&mut self, intent: TxIntent, rest_bytes: usize) -> bool {
        self.intent = None;
        self.intent_omitted = None;
        let with = rest_bytes + self.approx_bytes() + "intent".len() + 4 + intent.bytes();
        if with <= MAX_INTENT_ENTRY_BYTES {
            self.intent = Some(Content(intent));
            true
        } else {
            self.intent_omitted = Some(IntentOmitted::TooLarge);
            false
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CommitRecord {
    Edit,
    Undo,
    Redo,
    Merge,
    Pick,
    Revert,
    Checkpoint,
    Branch,
    BranchSet,
    Recover,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum CommitOutcome {
    Accepted,
    Rebased,
    Deduped,
    NoOp,
    Conflict,
    Refused,
    DryRun,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum IntentOmitted {
    // The intent would push the entry past 32 KiB; the builder takes it from the op log.
    TooLarge,
}
