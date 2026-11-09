//! The JSON the engine (`fin-node`) sends to JavaScript that is not a `fin-core` type: events
//! and the set-aside answer of `tx.get`. fin-node serialises exactly these types, and the
//! schema export (`examples/export_schema.rs`) puts them in `@finisher/core-types`, so
//! `finisherd` and `@finisher/contracts` read generated types, never hand-written mirrors.

use std::collections::BTreeMap;

use fin_core::branch::BranchState;
use fin_core::rebase::placement::Placement;
use fin_core::{CommittedTx, Conflict, CoreError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::StoreEvent;

/// One engine event (`Engine.onEvent`), tagged by `type`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum EngineEvent {
    /// A record imported from the folder.
    Applied { tx: CommittedTx },
    /// The status changed (fin-store's `ProjectStatus`).
    Status {
        #[schemars(with = "Value")]
        status: Value,
    },
    /// An import took `branch`'s revs from `from_rev` on out of its history: `quarantined`
    /// records were set aside (`own`: some were this server's) and the branch is at
    /// `generation` of database `incarnation` now.
    Rewound {
        branch: String,
        from_rev: u64,
        quarantined: u32,
        own: bool,
        generation: u64,
        incarnation: String,
    },
    /// The database was rebuilt: a new `incarnation`, and every rev read before may hold
    /// something else now.
    Rebuilt { incarnation: String },
    /// A background tick, backup or compaction failed (`fatal`: the engine is gone).
    Error {
        #[schemars(with = "CoreError")]
        error: Value,
        fatal: bool,
    },
    /// The first event after registration when the buffer overflowed: `dropped` events
    /// were lost.
    Lagged { dropped: u64 },
    /// The project file's `collect` flag changed (`default` or `never`).
    Collect { collect: crate::CollectPolicy },
}

impl EngineEvent {
    /// The event for `e`; `incarnation` is the database's (for `rewound`).
    pub fn from_store(e: &StoreEvent, incarnation: Option<&str>) -> EngineEvent {
        match e {
            StoreEvent::Applied(tx) => EngineEvent::Applied { tx: tx.clone() },
            StoreEvent::Status(status) => EngineEvent::Status {
                status: serde_json::to_value(status).unwrap_or(Value::Null),
            },
            StoreEvent::Rewound {
                branch,
                from_rev,
                quarantined,
                own,
                generation,
            } => EngineEvent::Rewound {
                branch: branch.clone(),
                from_rev: *from_rev,
                quarantined: *quarantined,
                own: *own,
                generation: *generation,
                incarnation: incarnation.unwrap_or_default().to_string(),
            },
            StoreEvent::Rebuilt { incarnation } => EngineEvent::Rebuilt {
                incarnation: incarnation.clone(),
            },
            StoreEvent::Collect(collect) => EngineEvent::Collect { collect: *collect },
        }
    }
}

/// `tx.get` for a transaction that is not (or no longer) on its branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TxSetAside {
    pub set_aside: SetAside,
}

/// A record kept in quarantine ([`TxStatus::SetAside`](crate::TxStatus)). `reason`:
/// `SUPERSEDED`, `DIVERGED_TAIL`, `SALVAGED`, `MISSING_FROM_FOLDER`, `STALE_EPOCH`,
/// `MALFORMED`, `CONFLICT_COPY`, `ID_COLLISION`, `IMPORTED`; `origin`: `local`, `folder`,
/// `foreign`, `salvage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetAside {
    pub tx_id: String,
    pub branch: String,
    pub reason: String,
    pub origin: String,
}

/// A branch as listings show it ([`Store::branches`](crate::Store::branches), `branch.list`):
/// `display` is the reference that names it (its name, or `<name>#<id>` for an archived
/// branch or the second holder of a duplicate name); `head` its head seq (its fork seq until
/// its first record).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchView {
    pub id: String,
    pub name: String,
    pub display: String,
    pub parent: Option<String>,
    pub fork_seq: u64,
    pub head: u64,
    pub state: BranchState,
    pub created_by: String,
}

/// The answer of [`Store::create_branch`](crate::Store::create_branch): the new branch and
/// its `branch` record on the parent. `dry_run`: planned and rolled back, nothing kept (a
/// retry of a committed create answers with that create, `dry_run` false).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BranchCreated {
    pub branch: BranchView,
    pub record: CommittedTx,
    pub dry_run: bool,
}

/// The answer of a history operation ([`Store::undo`](crate::Store::undo),
/// [`Store::redo`](crate::Store::redo)): the records it committed, in order (or planned and
/// rolled back, `dry_run`; a retry of a committed operation answers with its records,
/// `dry_run` false), and what they act on: the units an undo undoes, the undo a redo
/// reverses, or the merge group a revert reverts. `no_op`: why a cherry-pick or revert
/// committed nothing (`ALREADY_PRESENT`: the target holds the record's origin root;
/// `ALREADY_REVERTED`: the merge group's effect is gone there); absent otherwise.
/// `origins`: the txIds the operation acted on, each with the records its `origin` chain
/// leads to (a copy of a copy), first seen first: an undo's units, a cherry-pick's origin, a
/// reverted group's records; empty (and omitted) for a redo and for nothing done (7A
/// Decision 27).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryResult {
    pub committed: Vec<CommittedTx>,
    pub undone: Vec<String>,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_op: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<String>,
    /// Where the record put each item back that a ripple restore or move placed (owner
    /// decision 2026-11-09; `fin_core::rebase::placement`): `clean`, or `relocated` with where
    /// and why. Always in the answer (the generated type marks it optional, as `origins`); a
    /// retry answers with what the record stored.
    #[serde(default)]
    pub placements: Vec<Placement>,
}

/// The answer of [`Store::merge`](crate::Store::merge): merge group `merge_id`'s records on
/// the target, in order (or planned and rolled back, `dry_run`); `merged`: the source txIds
/// they copy; `skipped`: the candidates that conflict and were left on the source (with
/// `skipConflicting`; they are offered again by the next merge); `excluded`: source records
/// never offered (`UNDOES_TARGET_RECORDS`); `source_state`: the source branch's state after
/// the merge; `origins`: `merged` with the records their `origin` chains lead to (a copy of a
/// copy), first seen first (7A Decision 27). A retry of a committed merge answers from the
/// log: its records and their origins, `skipped` and `excluded` empty, `dry_run` false.
/// Nothing left to merge is an empty answer, not an error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MergeReport {
    pub merge_id: String,
    pub committed: Vec<CommittedTx>,
    pub merged: Vec<String>,
    pub skipped: Vec<Conflict>,
    pub excluded: Vec<Conflict>,
    pub source_state: BranchState,
    pub dry_run: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub origins: Vec<String>,
    /// As [`HistoryResult::placements`], each with its copy's txId, plus the `conflict`
    /// placements of skipped candidates (a retry answers with the copies' stored ones only, as
    /// its `skipped` is empty).
    #[serde(default)]
    pub placements: Vec<Placement>,
}

/// One group of quarantined records ([`Store::recover_groups`](crate::Store::recover_groups)).
///
/// - `id`: the first record's `txHash`, 12 hex digits (stable on every server);
/// - `records_branch`: the branch the records were on;
/// - `branch`, `fork_seq`, `fork_hash`: the fork `create` uses: an existing branch, the seq
///   and its record; or, with `fork_group`, the branch that group makes (`branch` its name,
///   `fork_hash` absent: its record is written then); none for a group that cannot be made
///   for want of a fork (`NO_COMMON_HISTORY`);
/// - `records`, `reasons`: how many, and per quarantine reason; `origin`: the first row's;
/// - `server`: the writer's server id (`unknown` when it cannot be told);
/// - `first_committed_at`: unix ms; `name`: the branch `create` makes (empty for a group
///   that makes none: it has neither records with ops nor checkpoints, and its child groups
///   fork where it does);
/// - `children`: the groups forking from it, or made by its `branch` records;
/// - `recoverable`, `problems`: why `create` leaves it whole (fix each, or dismiss it);
/// - `notes`: rows `create` leaves in quarantine while making the rest (`NAME_TAKEN`,
///   `BRANCH_NOT_FOUND`, …; they come back as a group of their own), and copies whose source
///   is not here (`SOURCE_NOT_FOUND`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverGroup {
    pub id: String,
    pub records_branch: String,
    pub branch: String,
    pub fork_seq: u64,
    pub fork_hash: Option<String>,
    pub fork_group: Option<String>,
    pub records: usize,
    pub reasons: BTreeMap<String, usize>,
    pub origin: String,
    pub server: String,
    pub first_committed_at: i64,
    pub name: String,
    pub children: Vec<String>,
    pub recoverable: bool,
    pub problems: Vec<Conflict>,
    pub notes: Vec<Conflict>,
}

/// A file `import_dir` did not read: its path and why (`not a segment: …`, `another
/// project`, `too large: …`, `unreadable: …`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

/// [`Store::recover`](crate::Store::recover)'s answer: the groups as found (before `create`),
/// the branches made and every record written (`branch` records on the parents, the copies,
/// checkpoints, `branchSet` records), in order, for publishing; `imported`: quarantine rows
/// `import_dir` added or made current again, and `skipped_files` the files it did not read;
/// `dismissed`: rows `dismiss` set aside from listing and counting; `manual`: the procedure
/// for a read-only state recovery cannot lift by itself (`CHAIN_FORK`, `CONFLICT_COPY`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoverReport {
    pub groups: Vec<RecoverGroup>,
    pub created: Vec<BranchView>,
    pub committed: Vec<CommittedTx>,
    pub imported: usize,
    pub skipped_files: Vec<SkippedFile>,
    pub dismissed: usize,
    pub manual: Option<String>,
    pub dry_run: bool,
}
