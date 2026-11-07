//! Atomic transaction apply: all ops succeed on a copy of the project, or nothing changes.

use serde_json::{Value, json};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, SCHEMA_VERSION};
use crate::ops::{self, Bindings, Op};
use crate::tx::{Applied, Diff, Tx};

/// Who is applying: clients may only send client ops (the `clip.*` aliases included, each
/// rewritten to its `item.*` op first, [`crate::normalize`]); internal callers (undo, replaying
/// inverses) may also send `asset.remove`, `item.remove`, `item.restore` and strict
/// `item.trim`s, and may name new entities with literal ids, but never an alias.
///
/// `Replay` re-applies committed history (see [`crate::apply_committed`]): like `Internal`, but
/// policy checks (the one-frame minimum, the source range, trim limits, `INSERT_INSIDE_CLIP`,
/// item kinds on track kinds other than a clip's, `TRACK_LOCKED`) are skipped and only
/// structural ones run: referenced entities exist, new ids are new and fit their kind, a clip's
/// asset has the track's stream, value shapes, and after every op no track the op's diff
/// touched ([`ops::util::touched_tracks`]) holds overlapping, empty or negative-start/`sourceIn`
/// items.
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

    /// Applies op `index` of the transaction, recording its changes and shifts under that
    /// index; returns what it recorded (its own diff). An error carries `details.opIndex`.
    pub(crate) fn op(
        &mut self,
        index: usize,
        op: &Op,
        ids: &mut dyn IdAlloc,
    ) -> Result<Diff, CoreError> {
        let before = self.diff.shifts.len();
        let changes_before = self.diff.changes.len();
        let applied = if self.mode == ApplyMode::Client && op.is_internal() {
            Err(CoreError::new(
                "INVALID_OP",
                format!("{} is internal and cannot be sent by clients", op.name()),
            )
            .with_details(json!({ "op": op.name() }))
            .with_fix(
                "use asset.add, item.insert, item.trim (ripple, roll or overwrite), item.delete, \
                 item.slip, item.split, item.move, item.slide, item.link, item.unlink, \
                 transition.set, transition.remove, track.add, track.remove, track.move, \
                 track.set, timeline.add, timeline.set or timeline.remove",
            ))
        } else if self.mode != ApplyMode::Client && op.is_alias() {
            Err(CoreError::new(
                "INVALID_OP",
                format!("{} is a client alias; history holds item ops", op.name()),
            )
            .with_details(json!({ "op": op.name() }))
            .with_fix("normalize the transaction (fin_core::normalize) before committing it"))
        } else {
            ops::apply(
                &mut self.project,
                op,
                &mut self.bindings,
                ids,
                &mut ops::util::OpDiff::new(&mut self.diff, op_index(index)),
            )
            .and_then(|inverse| {
                let own = self.own_diff(changes_before, before);
                debug_assert_eq!(
                    inverse.len(),
                    ops::inverse_len(op, &own),
                    "{}: inverse_len disagrees with the inverse built",
                    op.name()
                );
                if self.mode == ApplyMode::Replay {
                    let touched = ops::util::touched_tracks(&self.project, &own);
                    ops::util::check_tracks(&self.project, &touched)?;
                }
                Ok(inverse)
            })
        };
        self.inverses.push(applied.map_err(|e| at_op(e, index))?);
        Ok(self.own_diff(changes_before, before))
    }

    /// The entries recorded since `changes` changes and `shifts` shifts.
    fn own_diff(&self, changes: usize, shifts: usize) -> Diff {
        Diff {
            changes: self.diff.changes[changes..].to_vec(),
            shifts: self.diff.shifts[shifts..].to_vec(),
        }
    }

    /// The placeholders bound so far, to the ids they name.
    pub(crate) fn bound(&self) -> &std::collections::BTreeMap<String, String> {
        &self.bindings.map
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

/// An op index as a diff records it (a transaction never holds `u32::MAX` ops).
fn op_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

fn at_op(mut e: CoreError, index: usize) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("opIndex".to_string(), json!(index));
    }
    e
}
