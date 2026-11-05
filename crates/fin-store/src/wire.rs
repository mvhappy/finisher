//! The JSON the engine (`fin-node`) sends to JavaScript that is not a `fin-core` type: events
//! and the set-aside answer of `tx.get`. fin-node serialises exactly these types, and the
//! schema export (`examples/export_schema.rs`) puts them in `@finisher/core-types`, so
//! `finisherd` and `@finisher/contracts` read generated types, never hand-written mirrors.

use fin_core::branch::BranchState;
use fin_core::{CommittedTx, CoreError};
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
/// `MALFORMED`, `CONFLICT_COPY`; `origin`: `local`, `folder`, `foreign`, `salvage`.
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
/// `dry_run` false), and what they act on: the units an undo undoes, or the undo a redo
/// reverses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HistoryResult {
    pub committed: Vec<CommittedTx>,
    pub undone: Vec<String>,
    pub dry_run: bool,
}
