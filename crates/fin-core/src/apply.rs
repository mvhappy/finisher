//! Atomic transaction apply: all ops succeed on a copy of the project, or nothing changes.

use serde_json::{Value, json};

use std::collections::BTreeSet;

use crate::captions::{LayoutInputs, check_overflow, is_restore};
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::layout::TextMeasurer;
use crate::model::{Project, SCHEMA_VERSION};
use crate::ops::util::keys;
use crate::ops::{self, Bindings, Op};
use crate::tx::{Applied, Diff, Tx};

/// Who is applying: clients may only send client ops (the `clip.*` aliases included, each
/// rewritten to its `item.*` op first, [`crate::normalize`]); internal callers (undo, replaying
/// inverses) may also send `item.remove`, `item.restore`, `track.restore`, `timeline.restore`
/// and strict `item.trim`s, and may name new entities with literal ids, but never an alias.
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

/// What an apply may use beyond the project (plan 4C Task 20), passed explicitly (no global
/// state). With a `measurer`, every apply but a replay ends with the caption overflow check
/// ([`check_overflow`]): once per transaction, never per op. Without one (offline, planning,
/// replay) nothing is measured.
#[derive(Clone, Copy, Default)]
pub struct ApplyEnv<'a> {
    pub measurer: Option<&'a dyn TextMeasurer>,
}

impl<'a> ApplyEnv<'a> {
    /// An env measuring with `m`, if any.
    pub fn measuring(m: Option<&'a dyn TextMeasurer>) -> ApplyEnv<'a> {
        ApplyEnv { measurer: m }
    }
}

impl std::fmt::Debug for ApplyEnv<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApplyEnv")
            .field("measurer", &self.measurer.map(|m| m.fingerprint()))
            .finish()
    }
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
    apply_tx_env(project, tx, ids, mode, ApplyEnv::default())
}

/// [`apply_tx_mode`] with `env`: its measurer checks caption overflow at the end of the
/// transaction (`CAPTION_OVERFLOW`, except in [`ApplyMode::Replay`]).
pub fn apply_tx_env(
    project: &Project,
    tx: &Tx,
    ids: &mut dyn IdAlloc,
    mode: ApplyMode,
    env: ApplyEnv<'_>,
) -> Result<Applied, CoreError> {
    let mut run = Applier::new(project, tx.schema_version, mode, env)?;
    run.reserve(&tx.ops);
    for (i, op) in tx.ops.iter().enumerate() {
        run.op(i, op, ids)?;
    }
    run.finish()
}

/// `tx` in canonical form (spec §5.12): each client time-form `keyframe.set`/`remove` replaced
/// by the key form it takes on `frame` (where the tx is first applied: the head for a fresh
/// commit, the base for a stale one), so the committed intent names keys, not times. Applied
/// as a client tx (internally when it holds an internal op), it errors as that apply would; a
/// tx with no time-form op is returned as it is.
pub fn canonicalize(frame: &Project, tx: &Tx) -> Result<Tx, CoreError> {
    if !tx.ops.iter().any(keys::is_time_form) {
        return Ok(tx.clone());
    }
    let mut ids = crate::ids::MemIds::after(frame);
    let mode = if tx.ops.iter().any(Op::is_internal) {
        ApplyMode::Internal
    } else {
        ApplyMode::Client
    };
    let mut run = Applier::new(frame, tx.schema_version, mode, ApplyEnv::default())?;
    run.reserve(&tx.ops);
    for (i, op) in tx.ops.iter().enumerate() {
        run.op(i, op, &mut ids)?;
    }
    Ok(Tx {
        ops: run.ran,
        ..tx.clone()
    })
}

/// One transaction applied op by op (the rebase engine re-runs each op as soon as its
/// positions are mapped): the evolving project, bindings, diff and inverses. Dropped on error,
/// like [`apply_tx_mode`]'s copy.
pub(crate) struct Applier<'e> {
    project: Project,
    bindings: Bindings,
    diff: Diff,
    inverses: Vec<Vec<Op>>,
    mode: ApplyMode,
    env: ApplyEnv<'e>,
    /// What the overflow check compares the result with: the layout inputs of the project
    /// the transaction started from (only when it will measure).
    before: Option<LayoutInputs>,
    /// The indices of the restore ops applied ([`is_restore`]).
    restores: BTreeSet<u32>,
    /// The ops as they ran: a client time-form keyframe op in its key form (spec §5.12).
    ran: Vec<Op>,
}

impl<'e> Applier<'e> {
    /// `SCHEMA_VERSION` when `schema_version` is not the current one.
    pub(crate) fn new(
        project: &Project,
        schema_version: u32,
        mode: ApplyMode,
        env: ApplyEnv<'e>,
    ) -> Result<Applier<'e>, CoreError> {
        if schema_version != SCHEMA_VERSION {
            return Err(CoreError::new(
                "SCHEMA_VERSION",
                format!("transaction schema version {schema_version} is not supported"),
            )
            .with_details(json!({ "expected": SCHEMA_VERSION, "actual": schema_version }))
            .with_fix(format!("send schemaVersion {SCHEMA_VERSION}")));
        }
        let measures = env.measurer.is_some() && mode != ApplyMode::Replay;
        Ok(Applier {
            before: measures.then(|| LayoutInputs::of(project)),
            restores: BTreeSet::new(),
            ran: Vec::new(),
            project: project.clone(),
            bindings: Bindings::new(mode),
            diff: Diff::default(),
            inverses: Vec::new(),
            mode,
            env,
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
        // A time-form keyframe op runs (and is committed) as its key form; history (replay)
        // holds key forms only.
        let keyed;
        let op = if self.mode != ApplyMode::Replay && keys::is_time_form(op) {
            keyed = keys::key_form(&self.project, &self.bindings, op, index)
                .map_err(|e| at_op(e, index))?;
            &keyed
        } else {
            op
        };
        if is_restore(op) {
            self.restores.insert(op_index(index));
        }
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
                 track.set, timeline.add, timeline.set, timeline.remove, marker.add, \
                 marker.set, marker.remove, bin.add, bin.move, bin.remove, asset.move, \
                 asset.remove, asset.reprobe, param.set, effect.add, effect.remove, \
                 effect.move, keyframe.set, keyframe.remove, style.add, style.set, style.remove, font.pin, \
                 font.unpin, font.repin, caption.setText, caption.split, caption.merge or \
                 caption.setStyle",
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
        self.ran.push(op.clone());
        Ok(self.own_diff(changes_before, before))
    }

    /// Reserves every placeholder `ops` (the whole transaction) names, so a key placeholder the
    /// applier generates for a time-form insert never is one the client uses later.
    pub(crate) fn reserve(&mut self, ops: &[Op]) {
        let names = ops.iter().flat_map(Op::ids).filter(|x| x.starts_with('$'));
        self.bindings.reserved.extend(names.map(str::to_string));
    }

    /// The entries recorded since `changes` changes and `shifts` shifts.
    fn own_diff(&self, changes: usize, shifts: usize) -> Diff {
        Diff {
            changes: self.diff.changes[changes..].to_vec(),
            shifts: self.diff.shifts[shifts..].to_vec(),
        }
    }

    /// The project as the ops applied so far left it.
    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    /// The placeholders bound so far, to the ids they name.
    pub(crate) fn bound(&self) -> &std::collections::BTreeMap<String, String> {
        &self.bindings.map
    }

    /// The result: the inverse lists each op's inverse in reverse op order. With a measurer
    /// (and not replaying), the captions the transaction changed are measured once here
    /// (`CAPTION_OVERFLOW` with `details.opIndex`).
    pub(crate) fn finish(self) -> Result<Applied, CoreError> {
        if let (Some(m), Some(before)) = (self.env.measurer, &self.before) {
            check_overflow(before, &self.project, &self.diff, &self.restores, m)?;
        }
        Ok(Applied {
            project: self.project,
            diff: self.diff,
            inverse: self.inverses.into_iter().rev().flatten().collect(),
            bindings: self.bindings.map,
        })
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
