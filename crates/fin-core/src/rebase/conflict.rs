//! Conflicts of a rebase, merge or cherry-pick (spec §4.3, Decision 17): one per op that cannot
//! be carried to the head, with a reason code.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::CoreError;

/// Conflict reason codes. A re-run that fails with full policy uses the policy error's code
/// instead ([`Conflict::from_rerun`]).
pub mod reasons {
    /// The op touches or uses an entity an intervening record removed.
    pub const REMOVED: &str = "REMOVED";
    /// An intervening record moved the same edge of the same entity.
    pub const SAME_EDGE: &str = "SAME_EDGE";
    /// An intervening record set the same parameter of the same entity (4C).
    pub const SAME_PARAM: &str = "SAME_PARAM";
    /// An earlier record of the same operation was skipped, and this one depends on it.
    pub const DEPENDS_ON_SKIPPED: &str = "DEPENDS_ON_SKIPPED";
    /// A copy's entity already exists, bound by a record with the same origin chain.
    pub const ALREADY_PRESENT: &str = "ALREADY_PRESENT";
    /// A copy's entity id is taken by an unrelated entity.
    pub const ID_COLLISION: &str = "ID_COLLISION";
    /// Undoing this unit would undo the target branch's own records (merge).
    pub const UNDOES_TARGET_RECORDS: &str = "UNDOES_TARGET_RECORDS";
}

/// One op that cannot be carried to the head: which op of which tx, why (a reason code), the
/// record to blame when there is one (else null), and what to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Conflict {
    pub tx_id: String,
    pub op_index: usize,
    pub reason: String,
    pub against_tx: Option<String>,
    pub fix: String,
}

impl Conflict {
    /// A conflict with the standard fix for `reason`.
    pub fn new(
        tx_id: impl Into<String>,
        op_index: usize,
        reason: impl Into<String>,
        against_tx: Option<String>,
    ) -> Conflict {
        let reason = reason.into();
        Conflict {
            fix: fix_for(&reason).to_string(),
            tx_id: tx_id.into(),
            op_index,
            reason,
            against_tx,
        }
    }

    /// An op whose re-run at the head failed with `e`: the reason is the policy code (or, for a
    /// `CONFLICT`, its `details.reason`), and the fix is the error's own when it has one.
    pub fn from_rerun(
        e: &CoreError,
        tx_id: &str,
        op_index: usize,
        against: Option<String>,
    ) -> Conflict {
        let reason = match (&*e.code, e.details.get("reason")) {
            ("CONFLICT", Some(Value::String(r))) => r.clone(),
            _ => e.code.clone(),
        };
        let mut c = Conflict::new(tx_id, op_index, reason, against);
        if let Some(fix) = &e.fix {
            c.fix = fix.clone();
        }
        c
    }
}

fn fix_for(reason: &str) -> &'static str {
    match reason {
        reasons::REMOVED => "the entity was removed since; read the project at the current head",
        reasons::SAME_EDGE | reasons::SAME_PARAM => {
            "the same edit was made since; read the project at the current head and decide"
        }
        reasons::DEPENDS_ON_SKIPPED => "resolve the skipped record's conflict first",
        reasons::ALREADY_PRESENT => "the change is already present; skip it",
        reasons::ID_COLLISION => "the entity id is taken; recreate the entity under a new id",
        reasons::UNDOES_TARGET_RECORDS => "undo the change on the target branch itself",
        _ => "read the project at the current head and send the edit again",
    }
}

/// Every conflict of one operation, in op order. Converts to one `CONFLICT` error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflicts(pub Vec<Conflict>);

/// `CONFLICT`: the 4A detail fields (`reason`, `opIndex`, `againstTx`, `txId`), message and fix
/// come from the first conflict; `details.conflicts` lists them all.
impl From<Conflicts> for CoreError {
    fn from(Conflicts(all): Conflicts) -> Self {
        let Some(first) = all.first() else {
            return CoreError::new("CONFLICT", "conflicts with committed history")
                .with_details(json!({ "conflicts": [] }))
                .with_fix(fix_for(""));
        };
        let against = first.against_tx.as_deref().map_or_else(
            || "the current head".to_string(),
            |t| format!("committed tx {t}"),
        );
        let message = format!(
            "op {} of {} conflicts with {against}: {}",
            first.op_index, first.tx_id, first.reason
        );
        let details = json!({
            "reason": first.reason,
            "opIndex": first.op_index,
            "againstTx": first.against_tx,
            "txId": first.tx_id,
            "conflicts": all,
        });
        let fix = first.fix.clone();
        CoreError::new("CONFLICT", message)
            .with_details(details)
            .with_fix(fix)
    }
}
