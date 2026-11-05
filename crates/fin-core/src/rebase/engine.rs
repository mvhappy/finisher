//! The rebase engine: map, judge, re-run, one op at a time (module docs in `rebase`).

use std::collections::BTreeSet;

use serde_json::json;

use crate::apply::{Applier, ApplyMode, apply_tx_mode};
use crate::error::CoreError;
use crate::ids::{IdAlloc, MemIds};
use crate::model::Project;
use crate::ops::Op;
use crate::record::resolve_ops;
use crate::tx::{Applied, CommittedTx, Diff, Tx};

use super::conflict::{Conflict, Conflicts, reasons};
use super::footprint::{Footprint, entity_conflict, footprint, split_diff};
use super::map::{Assoc, Mapping, StepMap, Unmapped};

/// One intervening record, as the engine sees it: its ops (placeholders resolved) and diff.
///
/// - `invert`: the step stands for the record undone (its map is the inverse of `diff`'s, and
///   its footprint swaps `creates` and `removes`).
/// - `mirror_of: Some(j)`: this step exactly inverts step `j < self` (an undo of it, a copy of
///   its inverted original, Decision 24). Positions deleted by one are recovered through the
///   other, and neither is judged for entity conflicts.
#[derive(Debug, Clone)]
pub struct Step {
    pub tx_id: String,
    pub ops: Vec<Op>,
    pub diff: Diff,
    pub mirror_of: Option<usize>,
    pub invert: bool,
}

impl Step {
    /// A committed record as a forward, unmirrored step. Fails when its placeholders have no
    /// recorded binding (`UNBOUND_PLACEHOLDER`).
    pub fn from_committed(c: &CommittedTx) -> Result<Step, CoreError> {
        Ok(Step {
            tx_id: c.tx.tx_id.clone(),
            ops: resolve_ops(&c.tx.ops, &c.bindings)?,
            diff: c.diff.clone(),
            mirror_of: None,
            invert: false,
        })
    }

    fn map(&self) -> Result<StepMap, CoreError> {
        let map = StepMap::from_diff(&self.diff);
        if self.invert { map.invert() } else { Ok(map) }
    }

    fn footprints(&self) -> Result<Vec<Footprint>, CoreError> {
        let diffs = split_diff(&self.ops, &self.diff)?;
        let mut out: Vec<Footprint> = self
            .ops
            .iter()
            .zip(&diffs)
            .map(|(op, d)| footprint(op, d))
            .collect();
        if self.invert {
            out.reverse();
            for f in &mut out {
                std::mem::swap(&mut f.creates, &mut f.removes);
            }
        }
        Ok(out)
    }
}

static NO_IDS: BTreeSet<String> = BTreeSet::new();

/// What to rebase: `tx` as written, `own` its diff in the frame it was written for (at its
/// base, or the source record's stored diff for a copy), and the `steps` between that frame
/// and the head, in order.
///
/// `same_origin`: ids on the head bound by records of the same origin chain as `tx` (earlier
/// copies of it, Decision 18). A re-run that meets one of them (`DUPLICATE_ID`) is
/// `ALREADY_PRESENT`; any other duplicate is `ID_COLLISION`.
#[derive(Debug, Clone, Copy)]
pub struct Rebase<'a> {
    pub tx: &'a Tx,
    pub own: &'a Diff,
    pub steps: &'a [Step],
    pub same_origin: &'a BTreeSet<String>,
}

impl<'a> Rebase<'a> {
    /// A rebase with no earlier copies (`same_origin` empty).
    pub fn new(tx: &'a Tx, own: &'a Diff, steps: &'a [Step]) -> Rebase<'a> {
        Rebase {
            tx,
            own,
            steps,
            same_origin: &NO_IDS,
        }
    }
}

/// The rebased tx (positions mapped, placeholders kept: the intent to store) and the result of
/// applying it at the head.
#[derive(Debug, Clone, PartialEq)]
pub struct Rebased {
    pub tx: Tx,
    pub applied: Applied,
}

/// Op by op: map positions (Assoc::After) through [inverse(our earlier ops), steps, our rebased
/// earlier ops] with mirrors; check entity conflicts against unmirrored steps; apply the op to
/// the evolving head with full policy (`mode`); a failure → `Conflict`.
///
/// The conflict names the op and, when a step is to blame, its txId. A position strictly inside
/// a range a step removed (with no mirror to recover it) is `REMOVED`; a re-run error is its
/// own code (`DUPLICATE_ID` as `ALREADY_PRESENT` / `ID_COLLISION`). Bad input (a diff that does
/// not match its ops, a mirror that does not point back) is a conflict with that error's code.
/// Ids are taken from `ids` only by ops that ran.
pub fn rebase_apply(
    head: &Project,
    r: &Rebase<'_>,
    ids: &mut dyn IdAlloc,
    mode: ApplyMode,
) -> Result<Rebased, Conflict> {
    let tx_id = r.tx.tx_id.as_str();
    let bad = |e: CoreError| Conflict::from_rerun(&e, tx_id, 0, None);
    let own = split_diff(&r.tx.ops, r.own).map_err(bad)?;
    let n = own.len();
    let mut mapping = Mapping::new();
    for d in own.iter().rev() {
        mapping.push(StepMap::from_diff(d).invert().map_err(bad)?);
    }
    let theirs = unmirrored_footprints(r.steps).map_err(bad)?;
    for step in r.steps {
        let at = mapping.push(step.map().map_err(bad)?);
        if let Some(j) = step.mirror_of {
            mapping.set_mirror(n + j, at);
        }
    }
    let blame = |i: usize| {
        (n..n + r.steps.len())
            .contains(&i)
            .then(|| r.steps[i - n].tx_id.clone())
    };
    let mut run = Applier::new(head, r.tx.schema_version, mode).map_err(bad)?;
    let mut ops = Vec::with_capacity(n);
    let mut created = BTreeSet::new();
    for (k, op) in r.tx.ops.iter().enumerate() {
        let mut op = op.clone();
        for (track, pos) in op.positions_mut() {
            *pos = mapping
                .map_from(n - k, &track, *pos, Assoc::After)
                .map_err(|u| match u {
                    Unmapped::Deleted { step } => {
                        Conflict::new(tx_id, k, reasons::REMOVED, blame(step))
                    }
                    Unmapped::Overflow => Conflict::from_rerun(&overflow(), tx_id, k, None),
                })?;
        }
        let fp = footprint(&op, &own[k]);
        if let Some((reason, against)) = entity_conflict(&fp, &created, &theirs) {
            return Err(Conflict::new(tx_id, k, reason, Some(against)));
        }
        created.extend(fp.creates);
        let shifts = run
            .op(k, &op, ids)
            .map_err(|e| rerun_conflict(&e, r, k))?
            .to_vec();
        let at = mapping.push(StepMap::from_shifts(shifts));
        mapping.set_mirror(n - 1 - k, at);
        ops.push(op);
    }
    let tx = Tx {
        ops,
        ..r.tx.clone()
    };
    Ok(Rebased {
        tx,
        applied: run.finish(),
    })
}

/// Stale commit: `own` = `tx` applied at `base` with `MemIds::after(base)`; steps = `since`
/// with rev > base_rev.
///
/// A conflict is `CONFLICT` (`details.conflicts` lists the one conflict). The result's
/// `base_rev` is the newest rev of `since`, or `tx.base_rev` when nothing is newer.
///
/// `tx` must apply at `base` as a client tx. When op i does not, the conflict rules still come
/// first, as for an op that applies: ops before i are rebased and op i is judged against the
/// steps, and a conflict there is the answer (its clip was deleted since, say, which no other
/// delta would fix). Otherwise the error at the base is returned as is.
pub fn rebase_tx(
    base: &Project,
    head: &Project,
    tx: &Tx,
    since: &[CommittedTx],
    ids: &mut dyn IdAlloc,
) -> Result<Rebased, CoreError> {
    let mut newer: Vec<&CommittedTx> = since.iter().filter(|c| c.rev > tx.base_rev).collect();
    newer.sort_by_key(|c| c.rev);
    let steps = newer
        .iter()
        .map(|c| Step::from_committed(c))
        .collect::<Result<Vec<_>, _>>()?;
    let own = match apply_tx_mode(base, tx, &mut MemIds::after(base), ApplyMode::Client) {
        Ok(own) => own,
        Err(e) => return Err(failed_at_base(base, head, tx, &steps, ids, e)),
    };
    let r = Rebase::new(tx, &own.diff, &steps);
    let mut out = rebase_apply(head, &r, ids, ApplyMode::Client)
        .map_err(|c| CoreError::from(Conflicts::one(c)))?;
    if let Some(last) = newer.last() {
        out.tx.base_rev = last.rev;
    }
    Ok(out)
}

/// `e`, the error of `tx` at its base, unless the ops before the one that failed conflict
/// when rebased, or that op conflicts by entity with `steps`.
fn failed_at_base(
    base: &Project,
    head: &Project,
    tx: &Tx,
    steps: &[Step],
    ids: &mut dyn IdAlloc,
    e: CoreError,
) -> CoreError {
    let Some(i) = e.details["opIndex"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
    else {
        return e;
    };
    let Some(failed) = tx.ops.get(i) else {
        return e;
    };
    let prefix = Tx {
        ops: tx.ops[..i].to_vec(),
        ..tx.clone()
    };
    let Ok(own) = apply_tx_mode(base, &prefix, &mut MemIds::after(base), ApplyMode::Client) else {
        return e;
    };
    if let Err(c) = rebase_apply(
        head,
        &Rebase::new(&prefix, &own.diff, steps),
        ids,
        ApplyMode::Client,
    ) {
        return Conflicts::one(c).into();
    }
    let Ok(own) = split_diff(&prefix.ops, &own.diff) else {
        return e;
    };
    let created: BTreeSet<String> = prefix
        .ops
        .iter()
        .zip(&own)
        .flat_map(|(op, d)| footprint(op, d).creates)
        .collect();
    let Ok(theirs) = unmirrored_footprints(steps) else {
        return e;
    };
    match entity_conflict(&footprint(failed, &Diff::default()), &created, &theirs) {
        Some((reason, against)) => {
            Conflicts::one(Conflict::new(&tx.tx_id, i, reason, Some(against))).into()
        }
        None => e,
    }
}

/// The footprints of every op of the steps that are in no mirrored pair, with their txIds
/// (what [`entity_conflict`] judges against). `INVALID_OP` for a bad `mirror_of`.
fn unmirrored_footprints(steps: &[Step]) -> Result<Vec<(&str, Footprint)>, CoreError> {
    let mut out = Vec::new();
    for (step, paired) in steps.iter().zip(mirrored(steps)?) {
        if !paired {
            let fps = step.footprints()?;
            out.extend(fps.into_iter().map(|f| (step.tx_id.as_str(), f)));
        }
    }
    Ok(out)
}

/// Which steps are in a mirrored pair. A `mirror_of` must name an earlier step that is in no
/// other pair (`INVALID_OP`).
fn mirrored(steps: &[Step]) -> Result<Vec<bool>, CoreError> {
    let mut out = vec![false; steps.len()];
    for (s, step) in steps.iter().enumerate() {
        let Some(j) = step.mirror_of else { continue };
        if j >= s || out[j] || out[s] {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("step {s} mirrors step {j}, which is not an earlier unpaired step"),
            )
            .with_details(json!({ "step": s, "mirrorOf": j })));
        }
        out[j] = true;
        out[s] = true;
    }
    Ok(out)
}

/// A re-run failure of op `k`. `DUPLICATE_ID` is a copy meeting an id that is already taken
/// (Decision 18).
fn rerun_conflict(e: &CoreError, r: &Rebase<'_>, k: usize) -> Conflict {
    if e.code == "DUPLICATE_ID" {
        let id = e.details["id"].as_str().unwrap_or_default();
        let reason = if r.same_origin.contains(id) {
            reasons::ALREADY_PRESENT
        } else {
            reasons::ID_COLLISION
        };
        return Conflict::new(r.tx.tx_id.as_str(), k, reason, None);
    }
    Conflict::from_rerun(e, &r.tx.tx_id, k, None)
}

fn overflow() -> CoreError {
    CoreError::new("TIME_OVERFLOW", "time overflowed while mapping a position")
        .with_fix("use smaller times, or a frame rate with a smaller denominator")
}
