//! Atomic transaction apply: all ops succeed on a copy of the project, or nothing changes.

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, SCHEMA_VERSION};
use crate::ops::{self, Bindings};
use crate::tx::{Applied, Diff, Tx};

/// Who is applying: clients may only send client ops; internal callers (undo, replaying
/// inverses) may also send `asset.remove`, `clip.remove` and `clip.restore`, and may name new
/// entities with literal ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Client,
    Internal,
}

/// Applies a client transaction. See [`apply_tx_mode`].
pub fn apply_tx(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc) -> Result<Applied, CoreError> {
    apply_tx_mode(project, tx, ids, ApplyMode::Client)
}

/// Applies an internal transaction, e.g. one built from `Applied::inverse`.
pub fn apply_internal(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc) -> Result<Applied, CoreError> {
    apply_tx_mode(project, tx, ids, ApplyMode::Internal)
}

/// Applies `tx.ops` in order to a copy of `project`. Any error is returned with
/// `details.opIndex` and leaves `project` untouched. `$name` placeholders bind in op order; the
/// inverse lists each op's inverse in reverse op order, with bound ids instead of placeholders.
/// Ids taken from `ids` by a failing transaction are not returned (gaps are harmless).
pub fn apply_tx_mode(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc, mode: ApplyMode) -> Result<Applied, CoreError> {
    if tx.schema_version != SCHEMA_VERSION {
        return Err(CoreError::new("SCHEMA_VERSION", format!("transaction schema version {} is not supported", tx.schema_version))
            .with_details(json!({ "expected": SCHEMA_VERSION, "actual": tx.schema_version }))
            .with_fix(format!("send schemaVersion {SCHEMA_VERSION}")));
    }
    let mut next = project.clone();
    let mut bindings = Bindings::new(mode == ApplyMode::Internal);
    let mut diff = Diff::default();
    let mut inverses = Vec::with_capacity(tx.ops.len());
    for (i, op) in tx.ops.iter().enumerate() {
        let applied = if mode == ApplyMode::Client && op.is_internal() {
            Err(CoreError::new("INVALID_OP", format!("{} is internal and cannot be sent by clients", op.name()))
                .with_details(json!({ "op": op.name() }))
                .with_fix("use asset.add, clip.insert, clip.trim or clip.delete"))
        } else {
            ops::apply(&mut next, op, &mut bindings, ids, &mut diff)
        };
        inverses.push(applied.map_err(|e| at_op(e, i))?);
    }
    let inverse = inverses.into_iter().rev().flatten().collect();
    Ok(Applied { project: next, diff, inverse, bindings: bindings.map })
}

fn at_op(mut e: CoreError, index: usize) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("opIndex".to_string(), json!(index));
    }
    e
}
