//! `review.verdict` v1 (5E Decision 20; agent spec §1.7 item 5): what a person, an agent, the
//! critic or a policy decided about a review item or, for the outcome tracker's entries,
//! about one agent transaction. Metadata only, except the training-only `reason`.
//!
//! The wire form ([`ReviewVerdictIn`]) carries raw ids (an idempotency key is not a ULID);
//! the server keys them.

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

use crate::events::approx::approx_struct;
use crate::events::class::{Content, IdRef, RawId, ScrubCtx, Scrubbed};
use crate::events::wire::WireForm;

/// The most txIds one verdict lists; a decision on more is several entries.
pub const MAX_VERDICT_TX_IDS: usize = 64;

/// What the verdict is about (`none` for the outcome tracker's entries).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ReviewItemKind {
    BranchReview,
    MergeConflict,
    QaFinding,
    CriticSummary,
    PluginSuggestion,
    None,
}

/// What was decided or became of the work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Verdict {
    Accepted,
    Rejected,
    Merged,
    Dismissed,
    ChangesRequested,
    Undone,
    Reverted,
    Abandoned,
}

/// How the outcome came about; absent for a person's decision on an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum VerdictVia {
    Undo,
    Revert,
    Archive,
    Merge,
    Critic,
    AutoMerge,
}

/// Who brought it about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum VerdictBy {
    User,
    Agent,
    Critic,
    Policy,
}

/// `review.verdict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVerdict {
    pub item_kind: ReviewItemKind,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VerdictVia")]
    pub via: Option<VerdictVia>,
    pub by: VerdictBy,
    #[serde(deserialize_with = "capped_tx_ids")]
    #[schemars(length(max = 64))]
    pub tx_ids: Vec<IdRef>,
    /// Counts of the transactions accepted and rejected by the decision.
    pub accepted: u32,
    pub rejected: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub session_id: Option<IdRef>,
    /// Seconds since the item opened (or since the transaction, for the tracker's entries).
    pub open_s: u64,
    /// Training only: the reason the person gave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Scrubbed>", extend("x-fin-content" = "userText"))]
    pub reason: Option<Content<Scrubbed>>,
}
approx_struct!(ReviewVerdict {
    item_kind,
    verdict,
    via,
    by,
    tx_ids,
    accepted,
    rejected,
    session_id,
    open_s,
    reason
});

fn too_many<E: serde::de::Error>(n: usize) -> E {
    E::custom(format!("txIds holds {n} ids, at most {MAX_VERDICT_TX_IDS}"))
}

fn capped_tx_ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<IdRef>, D::Error> {
    let ids = Vec::<IdRef>::deserialize(d)?;
    if ids.len() > MAX_VERDICT_TX_IDS {
        return Err(too_many(ids.len()));
    }
    Ok(ids)
}

fn capped_raw_ids<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<RawId>, D::Error> {
    let ids = Vec::<RawId>::deserialize(d)?;
    if ids.len() > MAX_VERDICT_TX_IDS {
        return Err(too_many(ids.len()));
    }
    Ok(ids)
}

/// `review.verdict` as `finisherd` sends it: ids raw, the reason scrubbed again here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewVerdictIn {
    pub item_kind: ReviewItemKind,
    pub verdict: Verdict,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<VerdictVia>,
    pub by: VerdictBy,
    #[serde(deserialize_with = "capped_raw_ids")]
    pub tx_ids: Vec<RawId>,
    // 7A's outcome tracker names one transaction and sends no counts: they default to 0.
    #[serde(default)]
    pub accepted: u32,
    #[serde(default)]
    pub rejected: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<RawId>,
    pub open_s: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<Content<Scrubbed>>,
}

impl WireForm for ReviewVerdictIn {
    type Payload = ReviewVerdict;

    fn into_payload(self, key: &dyn Fn(&[u8]) -> [u8; 16], ctx: &ScrubCtx) -> ReviewVerdict {
        ReviewVerdict {
            item_kind: self.item_kind,
            verdict: self.verdict,
            via: self.via,
            by: self.by,
            tx_ids: self
                .tx_ids
                .into_iter()
                .map(|id| id.into_id_ref(key))
                .collect(),
            accepted: self.accepted,
            rejected: self.rejected,
            session_id: self.session_id.map(|id| id.into_id_ref(key)),
            open_s: self.open_s,
            reason: self.reason.map(|r| Content(r.0.rescrub(ctx))),
        }
    }
}
