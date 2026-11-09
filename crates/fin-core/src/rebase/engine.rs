//! The rebase engine: map, judge, re-run, one op at a time (module docs in `rebase`).

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::apply::{Applier, ApplyEnv, ApplyMode, apply_tx_mode};
use crate::error::CoreError;
use crate::ids::{IdAlloc, MemIds};
use crate::model::Project;
use crate::ops::Op;
use crate::tx::{Applied, CommittedTx, Diff, Tx};

use super::anchors::{Anchor, Ours as AnchorOurs, Placed, split_parts};
use super::conflict::{Conflict, Conflicts, reasons};
use super::footprint::{
    Footprint, Touch, entity_conflict_in, footprint, overwrite_conflict, split_diff, unbind_own,
};
use super::map::{Assoc, Mapping, StepMap, Unmapped, content_of};
use super::places::{Resolved, Slot, set_index};
use super::steps::{Prepared, Step, steps_from};

static NO_IDS: BTreeSet<String> = BTreeSet::new();
static NO_OPS: BTreeSet<usize> = BTreeSet::new();

/// What to rebase: `tx` as written, `own` its diff in the frame it was written for (at its
/// base, or the source record's stored diff for a copy), and the `steps` between that frame
/// and the head, in order.
///
/// `same_origin`: ids on the head bound by records of the same origin chain as `tx` (earlier
/// copies of it, Decision 18). A re-run that meets one of them (`DUPLICATE_ID`) is
/// `ALREADY_PRESENT`; any other duplicate is `ID_COLLISION`.
///
/// `absorbs`: the indexes of `tx`'s lift `item.remove` ops whose item the next op takes back
/// into another item: the inverse of a split, as the planner knows it from the records'
/// links (`history::absorbs`). Each is judged as a put-back of the removed item.
///
/// `env`: what the re-run at the head may use (plan 4C Task 20): with a measurer, the rebased
/// tx ends with the caption overflow check, and `CAPTION_OVERFLOW` is a conflict.
#[derive(Debug, Clone, Copy)]
pub struct Rebase<'a> {
    pub tx: &'a Tx,
    pub own: &'a Diff,
    pub steps: &'a [Step],
    pub same_origin: &'a BTreeSet<String>,
    pub env: ApplyEnv<'a>,
    pub absorbs: &'a BTreeSet<usize>,
    /// Per op, the place an index op names in its own frame (`places::places`; empty when no
    /// op names one): the engine carries the index to the head by its neighbours.
    pub places: &'a [Option<Slot>],
    /// Per op, per position, the item edge it was written against in its own frame
    /// (`anchors::anchors`; empty when none is read): the engine resolves it at the head.
    pub anchors: &'a [Vec<Option<Anchor>>],
}

impl<'a> Rebase<'a> {
    /// A rebase with no earlier copies (`same_origin` empty).
    pub fn new(tx: &'a Tx, own: &'a Diff, steps: &'a [Step]) -> Rebase<'a> {
        Rebase {
            tx,
            own,
            steps,
            same_origin: &NO_IDS,
            env: ApplyEnv::default(),
            absorbs: &NO_OPS,
            places: &[],
            anchors: &[],
        }
    }

    /// The rebase with the position anchors `anchors` (see [`Rebase`]).
    pub fn with_anchors(self, anchors: &'a [Vec<Option<Anchor>>]) -> Rebase<'a> {
        Rebase { anchors, ..self }
    }

    /// The rebase with the index places `places` (see [`Rebase`]).
    pub fn with_places(self, places: &'a [Option<Slot>]) -> Rebase<'a> {
        Rebase { places, ..self }
    }

    /// The rebase with the split-undo removals `absorbs` (see [`Rebase`]).
    pub fn with_absorbs(self, absorbs: &'a BTreeSet<usize>) -> Rebase<'a> {
        Rebase { absorbs, ..self }
    }

    /// The rebase re-run with `env`.
    pub fn with_env(self, env: ApplyEnv<'a>) -> Rebase<'a> {
        Rebase { env, ..self }
    }
}

/// [`Rebase`] over [`Prepared`] steps: what a merge passes, sharing its steps between
/// candidates (plan 4B Task 20).
#[derive(Debug, Clone, Copy)]
pub struct PreparedRebase<'a> {
    pub tx: &'a Tx,
    pub own: &'a Diff,
    pub steps: &'a [Prepared],
    pub same_origin: &'a BTreeSet<String>,
    pub env: ApplyEnv<'a>,
    pub absorbs: &'a BTreeSet<usize>,
    pub places: &'a [Option<Slot>],
    pub anchors: &'a [Vec<Option<Anchor>>],
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
/// own code (`DUPLICATE_ID` as `ALREADY_PRESENT` / `ID_COLLISION`), with its details, and
/// blames the one unmirrored step that changed an entity the op changes or the error names,
/// when there is exactly one. Bad input (a diff that does
/// not match its ops, a mirror that does not point back) is a conflict with that error's code.
/// Ids are taken from `ids` only by ops that ran.
pub fn rebase_apply(
    head: &Project,
    r: &Rebase<'_>,
    ids: &mut dyn IdAlloc,
    mode: ApplyMode,
) -> Result<Rebased, Conflict> {
    let steps: Vec<Prepared> = r.steps.iter().map(Prepared::new).collect();
    let p = PreparedRebase {
        tx: r.tx,
        own: r.own,
        steps: &steps,
        same_origin: r.same_origin,
        env: r.env,
        absorbs: r.absorbs,
        places: r.places,
        anchors: r.anchors,
    };
    rebase_prepared(head, &p, ids, mode)
}

/// [`rebase_apply`] over steps prepared once ([`Prepared`]).
pub fn rebase_prepared(
    head: &Project,
    r: &PreparedRebase<'_>,
    ids: &mut dyn IdAlloc,
    mode: ApplyMode,
) -> Result<Rebased, Conflict> {
    let tx_id = r.tx.tx_id.as_str();
    let bad = |e: CoreError| Conflict::from_rerun(&e, tx_id, 0, None);
    // Ids our tx allocated where `own` was recorded go back to placeholders (`unbind_own`).
    let own = unbind_own(&r.tx.ops, split_diff(&r.tx.ops, r.own).map_err(bad)?);
    let n = own.len();
    let mut mapping = Mapping::new();
    for d in own.iter().rev() {
        mapping.push(StepMap::from_diff(d).invert().map_err(bad)?);
    }
    let theirs = unmirrored_footprints(r.steps).map_err(bad)?;
    // What the head placed since the frame, for the `End` walk of an anchor (`anchors`).
    let placed = Placed::join(
        r.steps
            .iter()
            .zip(mirrored(r.steps).map_err(bad)?)
            .filter(|(_, paired)| !paired)
            .map(|(s, _)| &*s.placed),
    );
    for step in r.steps {
        let at = mapping.push(step.map.clone().map_err(bad)?);
        if let Some(j) = step.mirror_of {
            mapping.set_mirror(n + j, at);
        }
    }
    let blame = |i: usize| {
        (n..n + r.steps.len())
            .contains(&i)
            .then(|| r.steps[i - n].tx_id.to_string())
    };
    let mut run = Applier::new(head, r.tx.schema_version, mode, r.env).map_err(bad)?;
    let mut ops = Vec::with_capacity(n);
    let mut created = BTreeSet::new();
    // Our earlier ops' footprints: an entity one of them created is blamed through what that
    // op touched (the item our split cut, for the right part it made).
    let mut earlier: Vec<Footprint> = Vec::with_capacity(n);
    for (k, op) in r.tx.ops.iter().enumerate() {
        let mut op = op.clone();
        let ours = AnchorOurs {
            bound: run.bound(),
            created: &created,
            parts: split_parts(&ops, run.bound()),
            placed: &placed,
        };
        let anchored = r.anchors.get(k);
        for (j, (track, pos)) in op.positions_mut().into_iter().enumerate() {
            // The edge it was written against, where it is now; else shifts (`anchors`).
            if let Some(Some(a)) = anchored.and_then(|v| v.get(j))
                && let Some(at) = a.resolve(run.project(), &track, &ours)
            {
                *pos = at;
                continue;
            }
            *pos = mapping
                .map_from(n - k, &track, *pos, Assoc::After)
                .map_err(|u| match u {
                    Unmapped::Deleted { step } => {
                        Conflict::new(tx_id, k, reasons::REMOVED, blame(step))
                    }
                    Unmapped::Overflow => Conflict::from_rerun(&overflow(), tx_id, k, None),
                })?;
        }
        for (item, v) in op.offsets_mut() {
            *v = mapping
                .map_content(n - k, &item, *v)
                .map_err(|_| Conflict::from_rerun(&overflow(), tx_id, k, None))?;
        }
        // An index carried to the head by its neighbours (`places`): another author's
        // placement where ours goes is `SAME_PARAM`, blamed on the first record that changed
        // the list.
        if let Some(Some(slot)) = r.places.get(k) {
            let list = slot.list().to_string();
            let against = theirs
                .iter()
                .find(|(_, f)| f.uses.contains(&list))
                .map(|(t, _)| t.to_string());
            // A member another record moved (its `index`) or re-created since anchors nothing.
            let placed_since = |e: &str| {
                theirs.iter().any(|(_, f)| {
                    f.creates.iter().any(|c| c == e)
                        || f.touches.iter().any(|(x, t)| {
                            x == e && *t == Touch::Param(crate::ops::util::lists::INDEX.into())
                        })
                })
            };
            let used = against.is_some();
            match slot.resolve(run.project(), &op, run.bound(), used, &placed_since) {
                Resolved::At(index) => set_index(&mut op, index),
                Resolved::Keep => {}
                Resolved::Taken => {
                    return Err(Conflict::new(tx_id, k, reasons::SAME_PARAM, against));
                }
            }
        }
        let mut fp = footprint(&op, &own[k]);
        // A removal whose content the next op takes back into another item (an undo of a
        // split, as the planner marked it) is judged as a put-back of the removed item.
        if r.absorbs.contains(&k)
            && let Op::ItemRemove(rm) = &op
        {
            fp.puts_back(&rm.id);
        }
        if let Some((reason, against)) = entity_conflict_in(&fp, &created, &theirs) {
            return Err(Conflict::new(tx_id, k, reason, Some(against)));
        }
        let rerun = match run.op(k, &op, ids) {
            Ok(d) => d,
            Err(e) => {
                let ours = Ours {
                    fp: &fp,
                    earlier: &earlier,
                    bound: run.bound(),
                };
                return Err(rerun_conflict(&e, r, k, &ours, &theirs, head));
            }
        };
        // An op that re-ran and puts an entity back whole would write its snapshot over a
        // later change of it (`overwrite_conflict`). One that wrote nothing at the head (a pin
        // of a file and place already held) loses nothing.
        let wrote = !(rerun.changes.is_empty() && rerun.shifts.is_empty());
        if wrote && let Some((reason, against)) = overwrite_conflict(&fp, &created, &theirs) {
            return Err(Conflict::new(tx_id, k, reason, Some(against)));
        }
        created.extend(fp.creates.iter().cloned());
        let at = mapping.push(rebased_map(rerun, run.bound()));
        mapping.set_own_mirror(n - 1 - k, at);
        earlier.push(fp);
        ops.push(op);
    }
    // The end-of-transaction overflow check: its error names the op (`opIndex`).
    let applied = run.finish().map_err(|e| {
        let k = e.details["opIndex"]
            .as_u64()
            .and_then(|k| usize::try_from(k).ok())
            .unwrap_or(n.saturating_sub(1));
        Conflict::from_rerun(&e, tx_id, k, None)
    })?;
    let tx = Tx {
        ops,
        ..r.tx.clone()
    };
    Ok(Rebased { tx, applied })
}

/// The map of one of our ops as re-run at the head (`rerun`: what it recorded there). Its
/// content offsets name the items our tx created by their placeholders, as our ops (and
/// [`unbind_own`]'s diffs of them) do, so a later op of ours naming `$x` meets the offsets of
/// both its own inverse and its rebased copy, and they cancel.
fn rebased_map(rerun: Diff, bound: &std::collections::BTreeMap<String, String>) -> StepMap {
    let content = content_of(&rerun)
        .into_iter()
        .map(|(id, by)| {
            let name = bound
                .iter()
                .find(|(_, v)| **v == id)
                .map(|(p, _)| p.clone());
            (name.unwrap_or(id), by)
        })
        .collect();
    StepMap::from_parts(rerun.shifts, content)
}

/// Stale commit: `own` = `tx` applied at `base` with `MemIds::after(base)`; steps = `since`
/// with rev > base_rev, mirrored from their links ([`steps_from`]: an undo with its units, a
/// redo with its undo).
///
/// A conflict is `CONFLICT` (`details.conflicts` lists the one conflict). The result's
/// `base_rev` is the newest rev of `since`, or `tx.base_rev` when nothing is newer.
///
/// `tx` must apply at `base` as a client tx. When op i does not, the conflict rules still come
/// first, as for an op that applies: ops before i are rebased and op i is judged against the
/// steps, and a conflict there is the answer (its clip was deleted since, say, which no other
/// delta would fix). Otherwise the error at the base is returned with `details.baseRev` and a
/// fix to send the edit again against the head (the entity it names may exist there).
pub fn rebase_tx(
    base: &Project,
    head: &Project,
    tx: &Tx,
    since: &[CommittedTx],
    ids: &mut dyn IdAlloc,
) -> Result<Rebased, CoreError> {
    rebase_tx_env(base, head, tx, since, ids, ApplyEnv::default())
}

/// [`rebase_tx`] re-run at the head with `env` (its measurer checks caption overflow there;
/// the apply at the base, which only records the frame, measures nothing).
pub fn rebase_tx_env(
    base: &Project,
    head: &Project,
    tx: &Tx,
    since: &[CommittedTx],
    ids: &mut dyn IdAlloc,
    env: ApplyEnv<'_>,
) -> Result<Rebased, CoreError> {
    let mut newer: Vec<&CommittedTx> = since.iter().filter(|c| c.rev > tx.base_rev).collect();
    newer.sort_by_key(|c| c.rev);
    let steps = steps_from(&newer)?;
    let own = match apply_tx_mode(base, tx, &mut MemIds::after(base), ApplyMode::Client) {
        Ok(own) => own,
        Err(e) => return Err(failed_at_base(base, head, tx, &steps, ids, e)),
    };
    let places = super::places::places(base, tx, ApplyMode::Client)?;
    let anchors = super::anchors::anchors(base, tx, ApplyMode::Client)?;
    let r = Rebase::new(tx, &own.diff, &steps)
        .with_env(env)
        .with_places(&places)
        .with_anchors(&anchors);
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
    let e = at_base(e, tx.base_rev);
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
    let anchors = super::anchors::anchors(base, &prefix, ApplyMode::Client).unwrap_or_default();
    if let Err(c) = rebase_apply(
        head,
        &Rebase::new(&prefix, &own.diff, steps).with_anchors(&anchors),
        ids,
        ApplyMode::Client,
    ) {
        return Conflicts::one(c).into();
    }
    let Ok(own) = split_diff(&prefix.ops, &own.diff) else {
        return e;
    };
    let own = unbind_own(&prefix.ops, own);
    let created: BTreeSet<String> = prefix
        .ops
        .iter()
        .zip(&own)
        .flat_map(|(op, d)| footprint(op, d).creates)
        .collect();
    let prepared: Vec<Prepared> = steps.iter().map(Prepared::new).collect();
    let Ok(theirs) = unmirrored_footprints(&prepared) else {
        return e;
    };
    match entity_conflict_in(&footprint(failed, &Diff::default()), &created, &theirs) {
        Some((reason, against)) => {
            Conflicts::one(Conflict::new(&tx.tx_id, i, reason, Some(against))).into()
        }
        None => e,
    }
}

/// `e` as the error of a tx at its base rev `base_rev`: `details.baseRev`, and a fix to read
/// the head and send the edit again (what it names may have been added since).
fn at_base(mut e: CoreError, base_rev: u64) -> CoreError {
    if let Value::Object(map) = &mut e.details {
        map.insert("baseRev".into(), json!(base_rev));
    }
    e.fix = Some(format!(
        "the edit does not apply at its baseRev {base_rev}; read the project at the current \
         head and send it again"
    ));
    e
}

/// The footprints of every op of the steps that are in no mirrored pair, with their txIds
/// (what [`entity_conflict_in`] judges against). `INVALID_OP` for a bad `mirror_of`.
fn unmirrored_footprints(steps: &[Prepared]) -> Result<Vec<(&str, &Footprint)>, CoreError> {
    let mut out = Vec::new();
    for (step, paired) in steps.iter().zip(mirrored(steps)?) {
        if !paired {
            let fps = step.footprints.as_ref().map_err(Clone::clone)?;
            out.extend(fps.iter().map(|f| (&*step.tx_id, f)));
        }
    }
    Ok(out)
}

/// Which steps are in a mirrored pair. A `mirror_of` must name an earlier step that is in no
/// other pair (`INVALID_OP`).
fn mirrored(steps: &[Prepared]) -> Result<Vec<bool>, CoreError> {
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

/// The op that failed to re-run, as [`rerun_conflict`] reads it: its footprint, the
/// footprints of our earlier ops, and the placeholders bound so far.
struct Ours<'a> {
    fp: &'a Footprint,
    earlier: &'a [Footprint],
    bound: &'a std::collections::BTreeMap<String, String>,
}

/// A re-run failure of op `k`. `DUPLICATE_ID` is a copy meeting an id that is already taken
/// (Decision 18). The step to blame is the one unmirrored step (if exactly one) that touched,
/// created or removed an entity ours touches, would create (another pin of the key, for a
/// `FONT_PINNED`) or the error names, or one an earlier op of ours touched to create such an
/// entity (the item our split cut, for an overwrite meeting the right part); tracks and the
/// assets ours only uses are left out, since nearly every step shares them.
fn rerun_conflict(
    e: &CoreError,
    r: &PreparedRebase<'_>,
    k: usize,
    ours: &Ours<'_>,
    theirs: &[(&str, &Footprint)],
    head: &Project,
) -> Conflict {
    let mut named: BTreeSet<&str> = ours.fp.touches.iter().map(|(x, _)| x.as_str()).collect();
    // What the op would create: a pin whose key another record pinned since (`FONT_PINNED`) is
    // blamed on that record. A placeholder names no entity here.
    named.extend(
        ours.fp
            .creates
            .iter()
            .map(String::as_str)
            .filter(|x| !x.starts_with('$')),
    );
    for key in ["clip", "item", "id", "asset"] {
        named.extend(e.details[key].as_str());
    }
    if let Some(clips) = e.details["clips"].as_array() {
        named.extend(clips.iter().filter_map(Value::as_str));
    }
    // An entity we created (by placeholder, or the id it bound) is named through what the op
    // that created it touched.
    let creators: Vec<&str> = named
        .iter()
        .flat_map(|x| {
            let placeholder = ours
                .bound
                .iter()
                .find(|(_, id)| id.as_str() == *x)
                .map(|(p, _)| p.as_str());
            [Some(*x), placeholder]
        })
        .flatten()
        .collect();
    for f in ours.earlier {
        if f.creates.iter().any(|c| creators.contains(&c.as_str())) {
            named.extend(f.touches.iter().map(|(x, _)| x.as_str()));
        }
    }
    for t in crate::ops::util::tracks(head) {
        named.remove(t.id.as_str());
    }
    let blamed: BTreeSet<&str> = theirs
        .iter()
        .filter(|(_, f)| {
            let changed = f
                .touches
                .iter()
                .map(|(x, _)| x)
                .chain(&f.creates)
                .chain(&f.removes);
            changed.into_iter().any(|x| named.contains(x.as_str()))
        })
        .map(|(tx, _)| *tx)
        .collect();
    let against = (blamed.len() == 1)
        .then(|| blamed.first().map(|t| t.to_string()))
        .flatten();
    if e.code == "DUPLICATE_ID" {
        let id = e.details["id"].as_str().unwrap_or_default();
        let reason = if r.same_origin.contains(id) {
            reasons::ALREADY_PRESENT
        } else {
            reasons::ID_COLLISION
        };
        return Conflict::new(r.tx.tx_id.as_str(), k, reason, against);
    }
    Conflict::from_rerun(e, &r.tx.tx_id, k, against)
}

fn overflow() -> CoreError {
    CoreError::new("TIME_OVERFLOW", "time overflowed while mapping a position")
        .with_fix("use smaller times, or a frame rate with a smaller denominator")
}
