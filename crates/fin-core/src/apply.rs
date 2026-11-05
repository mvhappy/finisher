//! Atomic transaction apply: all ops succeed on a copy of the project, or nothing changes.

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, SCHEMA_VERSION};
use crate::ops::{self, Bindings, Op};
use crate::tx::{Applied, Diff, Shift, Tx};

/// Who is applying: clients may only send client ops; internal callers (undo, replaying
/// inverses) may also send `asset.remove`, `clip.remove` and `clip.restore`, and may name new
/// entities with literal ids.
///
/// `Replay` re-applies committed history (see [`crate::apply_committed`]): like `Internal`, but
/// policy checks (the one-frame minimum, `sourceIn + duration` within the asset, trim limits,
/// `INSERT_INSIDE_CLIP`) are skipped and only structural ones run: referenced entities exist,
/// new ids are new and fit their kind, track kinds match, assets have a positive duration, and
/// after every op no track holds overlapping, empty or negative-start/`sourceIn` clips.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Client,
    Internal,
    Replay,
}

/// Applies a client transaction. See [`apply_tx_mode`].
pub fn apply_tx(project: &Project, tx: &Tx, ids: &mut dyn IdAlloc) -> Result<Applied, CoreError> {
    apply_tx_mode(project, tx, ids, ApplyMode::Client)
}

/// Applies an internal transaction, e.g. one built from `Applied::inverse`.
pub fn apply_internal(
    project: &Project,
    tx: &Tx,
    ids: &mut dyn IdAlloc,
) -> Result<Applied, CoreError> {
    apply_tx_mode(project, tx, ids, ApplyMode::Internal)
}

/// Applies `tx.ops` in order to a copy of `project`. Any error is returned with
/// `details.opIndex` and leaves `project` untouched. `$name` placeholders bind in op order; the
/// inverse lists each op's inverse in reverse op order, with bound ids instead of placeholders.
/// Ids taken from `ids` by a failing transaction are not returned (gaps are harmless).
pub fn apply_tx_mode(
    project: &Project,
    tx: &Tx,
    ids: &mut dyn IdAlloc,
    mode: ApplyMode,
) -> Result<Applied, CoreError> {
    let mut run = Applier::new(project, tx.schema_version, mode)?;
    for (i, op) in tx.ops.iter().enumerate() {
        run.op(i, op, ids)?;
    }
    Ok(run.finish())
}

/// One transaction applied op by op (the rebase engine re-runs each op as soon as its
/// positions are mapped): the evolving project, bindings, diff and inverses. Dropped on error,
/// like [`apply_tx_mode`]'s copy.
pub(crate) struct Applier {
    project: Project,
    bindings: Bindings,
    diff: Diff,
    inverses: Vec<Vec<Op>>,
    mode: ApplyMode,
}

impl Applier {
    /// `SCHEMA_VERSION` when `schema_version` is not the current one.
    pub(crate) fn new(
        project: &Project,
        schema_version: u32,
        mode: ApplyMode,
    ) -> Result<Applier, CoreError> {
        if schema_version != SCHEMA_VERSION {
            return Err(CoreError::new(
                "SCHEMA_VERSION",
                format!("transaction schema version {schema_version} is not supported"),
            )
            .with_details(json!({ "expected": SCHEMA_VERSION, "actual": schema_version }))
            .with_fix(format!("send schemaVersion {SCHEMA_VERSION}")));
        }
        Ok(Applier {
            project: project.clone(),
            bindings: Bindings::new(mode),
            diff: Diff::default(),
            inverses: Vec::new(),
            mode,
        })
    }

    /// Applies op `index` of the transaction; returns the shifts it recorded. An error carries
    /// `details.opIndex`.
    pub(crate) fn op(
        &mut self,
        index: usize,
        op: &Op,
        ids: &mut dyn IdAlloc,
    ) -> Result<&[Shift], CoreError> {
        let before = self.diff.shifts.len();
        let applied = if self.mode == ApplyMode::Client && op.is_internal() {
            Err(CoreError::new(
                "INVALID_OP",
                format!("{} is internal and cannot be sent by clients", op.name()),
            )
            .with_details(json!({ "op": op.name() }))
            .with_fix("use asset.add, clip.insert, clip.trim or clip.delete"))
        } else {
            ops::apply(
                &mut self.project,
                op,
                &mut self.bindings,
                ids,
                &mut self.diff,
            )
            .and_then(|inverse| {
                if self.mode == ApplyMode::Replay {
                    ops::util::check_tracks(&self.project, &self.diff.shifts[before..])?;
                }
                Ok(inverse)
            })
        };
        self.inverses.push(applied.map_err(|e| at_op(e, index))?);
        Ok(&self.diff.shifts[before..])
    }

    /// The result: the inverse lists each op's inverse in reverse op order.
    pub(crate) fn finish(self) -> Applied {
        Applied {
            project: self.project,
            diff: self.diff,
            inverse: self.inverses.into_iter().rev().flatten().collect(),
            bindings: self.bindings.map,
        }
    }
}

fn at_op(mut e: CoreError, index: usize) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("opIndex".to_string(), json!(index));
    }
    e
}
