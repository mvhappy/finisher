//! Conflict footprints: what one op touches, per track and per entity (spec §4.3).
//!
//! A footprint comes from an op plus its own diff, never from a project, so a committed record
//! is judged by what it stored. Each op module builds its own from what the op names
//! (`ops::<op>::footprint`); [`footprint`] dispatches and adds what the op's diff records
//! ([`Footprint::with_changes`], plan 4C Decision 20), so an op that changed entities it does
//! not name (an overwrite's neighbours, a link group) is judged on them too. [`split_diff`] /
//! [`committed_footprints`] cut a record's diff into its ops' own diffs by each entry's `op`.
//!
//! Rules ([`entity_conflict`]), judged against the intervening records the caller passes (the
//! unmirrored ones, Decision 24):
//! - ours touches or uses X, and the last intervening record that removed or created X removed
//!   it → [`reasons::REMOVED`] (a stale insert of a removed asset included);
//! - the same [`Edge`] of X → [`reasons::SAME_EDGE`]; the same parameter → [`reasons::SAME_PARAM`];
//! - ours puts X back whole ([`Footprint::overwrites`]: an inverse that re-creates X from a
//!   snapshot, such as `item.restore`, `effect.restore`, `track.restore`, `timeline.restore`, or the
//!   `marker.add`, `asset.add`, `bin.add` and `style.add` of a removal's inverse) and an
//!   intervening record changed X, an entity nested in it (`X/…`: an item's effects) or what
//!   lies on it, in any way, touching, creating or removing it → `SAME_EDGE` when it moved an
//!   edge of a put-back entity, else `SAME_PARAM` ([`overwrite_conflict`]). What lies on it is
//!   any put-back entity among the record's `tracks` or `uses`: an insert or move onto a
//!   put-back track, a `marker.add` or `track.add` on a put-back timeline, a placement in a
//!   put-back ordered list. Putting the snapshot back would take that edit away without a
//!   word: an undo of a move after another author's trim, split or effect edit of the item; a
//!   merged delete and undo over the target's edit. The engine judges it after the op re-ran,
//!   so an op that cannot run at all keeps the re-run's reason (`ALREADY_PRESENT`,
//!   `ID_COLLISION`, `CLIP_OVERLAP`, …) and only one that would succeed is refused. A
//!   `$placeholder` re-creation makes a new entity and puts nothing back;
//! - an ordered list is an entity of its own (`ops::util::lists`): every op that adds, moves
//!   or removes a member uses it, and an op that puts a member back at its old index
//!   (`track.restore`, `timeline.restore`, `effect.restore`, a placing `font.pin`) puts the
//!   list back. So a later placement by someone else meets that put-back (`SAME_PARAM`), while
//!   two placements still merge;
//! - a removal whose content the next op takes back into another item (a lift `item.remove`
//!   followed by a growing `strict` trim: the inverse of a split, [`absorbed`]) is judged as a
//!   put-back of the removed item. "Removing an entity someone else edited is not a conflict"
//!   does not hold there, as that edit would be lost inside the item that absorbs the range;
//! - an entity an earlier op of our own transaction created is ours: it is never judged. At a
//!   stale base our placeholders bind the ids the intervening records allocated, and those name
//!   other entities; the re-run at the head decides whether the creation itself still fits.
//!
//! Entity rules ignore tracks: an entity lives on one track unless it moved (4C), and a move
//! must still meet the edits made on the other track. So an edit on track B never conflicts
//! with edits on track A by entity unless they share an entity; `tracks` is for the track-level
//! rules (link groups and moves between tracks, 4C).
//!
//! `REMOVED` wins over `SAME_EDGE`/`SAME_PARAM`; within one reason the earliest record is named.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::CoreError;
use crate::ops::{self, Edge, Op};
use crate::record::{invalid_format, resolve_ops};
use crate::tx::{Change, ChangeKind, CommittedTx, Diff, Shift};

use super::conflict::reasons;

/// How an op touches an entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Touch {
    /// Needs it to exist (and changes it as a whole, e.g. removes it).
    Exists,
    /// Moves one edge of it.
    Edge(Edge),
    /// Sets one parameter of it (4C).
    Param(String),
}

/// What an op touches. `tracks`: the tracks it changes or places on (4C: every track its link
/// groups span, and both tracks of a move between tracks); `touches`: entities it changes;
/// `removes`/`creates`: entities it takes out or brings in; `uses`: entities it refers to
/// without changing them (an insert's asset and track); `overwrites`: entities it puts back
/// whole, as its snapshot holds them (with what is nested in them), judged against any
/// intervening change of them ([`Footprint::puts_back`], module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    pub tracks: BTreeSet<String>,
    pub touches: Vec<(String, Touch)>,
    pub removes: Vec<String>,
    pub creates: Vec<String>,
    pub uses: Vec<String>,
    pub overwrites: Vec<String>,
}

impl Footprint {
    /// An empty footprint on `track` (if any), every track `diff` shifts and every track its
    /// changes name (an item's, plan 4C Decision 4), so ops that record no shift (a lift, a
    /// strict or overwrite trim, a roll, a slip, a split) are on their tracks too.
    pub(crate) fn on(track: Option<&str>, diff: &Diff) -> Footprint {
        let mut tracks: BTreeSet<String> = diff.shifts.iter().map(|s| s.track.clone()).collect();
        tracks.extend(diff.changes.iter().filter_map(|c| c.track.clone()));
        tracks.extend(track.map(str::to_string));
        Footprint {
            tracks,
            ..Footprint::default()
        }
    }

    /// Declares that the op puts entity `id` back whole from a snapshot ([`Self::overwrites`]).
    /// A `$placeholder` id is a new entity, never one put back, so it is left out.
    pub(crate) fn puts_back(&mut self, id: &str) {
        if !id.starts_with('$') && !self.overwrites.iter().any(|x| x == id) {
            self.overwrites.push(id.to_string());
        }
    }

    /// Adds what `own` (the op's own diff) records (Decision 20): `removed` → `removes`,
    /// `added` → `creates`, `updated` with an `edge` → [`Touch::Edge`], with a `path` →
    /// [`Touch::Param`], any other `updated` → [`Touch::Exists`]. What the op's own rules
    /// already list is not repeated.
    pub(crate) fn with_changes(mut self, own: &Diff) -> Footprint {
        fn add<T: PartialEq>(v: &mut Vec<T>, x: T) {
            if !v.contains(&x) {
                v.push(x);
            }
        }
        for c in &own.changes {
            match c.change {
                ChangeKind::Removed => add(&mut self.removes, c.id.clone()),
                ChangeKind::Added => add(&mut self.creates, c.id.clone()),
                ChangeKind::Updated => {
                    let touch = match (c.edge, &c.path) {
                        (Some(edge), _) => Touch::Edge(edge),
                        (None, Some(path)) => Touch::Param(path.clone()),
                        (None, None) => Touch::Exists,
                    };
                    add(&mut self.touches, (c.id.clone(), touch));
                }
            }
        }
        self
    }
}

/// The footprint of `op` from its own diff (`op_diff`: what it recorded): the op's own rules
/// plus [`Footprint::with_changes`]. A committed op is passed with its placeholders resolved;
/// one of ours may keep its `$name`s.
pub fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    ops::footprint(op, op_diff).with_changes(op_diff)
}

/// Each op's own diff out of the diff of the transaction `ops` made, by each change's and
/// shift's `op` (entries keep their index). `INVALID_FORMAT` when an entry names an op past
/// the end, or a list is not in op order.
pub fn split_diff(ops: &[Op], diff: &Diff) -> Result<Vec<Diff>, CoreError> {
    let mut out = vec![Diff::default(); ops.len()];
    let slot = |op: u32, last: &mut u32, what: &str| -> Result<usize, CoreError> {
        let i = usize::try_from(op).unwrap_or(usize::MAX);
        if i >= ops.len() {
            return Err(invalid_format(
                "diff",
                format!(
                    "a {what} names op {op}; the transaction has {} ops",
                    ops.len()
                ),
            ));
        }
        if op < *last {
            return Err(invalid_format(
                "diff",
                format!("its {what}s are not in op order"),
            ));
        }
        *last = op;
        Ok(i)
    };
    let mut last = 0;
    for c in &diff.changes {
        out[slot(c.op, &mut last, "change")?]
            .changes
            .push(c.clone());
    }
    let mut last = 0;
    for s in &diff.shifts {
        out[slot(s.op, &mut last, "shift")?].shifts.push(s.clone());
    }
    Ok(out)
}

/// Our own ops' diffs (`own`, from [`split_diff`]) with the ids our transaction allocated
/// given back as the placeholders that allocated them. `own` was recorded where the tx was
/// first applied (at its base, or on the source branch for a copy), so those ids are the ones
/// bound *there*: on the head the same ids may name other entities (an intervening record
/// allocated them), and a footprint must not claim them. An op that allocates `$p` and records
/// one `added` change maps that change's id to `$p`; when it records several, which one is
/// `$p`'s is unknown, so they are dropped (the op's own rules still name `$p`). Every change of
/// every op naming such an id follows. Shifts carry no ids and stay.
pub(crate) fn unbind_own(ops: &[Op], mut own: Vec<Diff>) -> Vec<Diff> {
    let mut bound: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (op, d) in ops.iter().zip(&own) {
        let Some(p) = op.allocates() else { continue };
        let added: Vec<&str> = d
            .changes
            .iter()
            .filter(|c| c.change == ChangeKind::Added && c.id != p)
            .map(|c| c.id.as_str())
            .collect();
        let to = (added.len() == 1).then(|| p.to_string());
        for id in added {
            bound.insert(id.to_string(), to.clone());
        }
    }
    if bound.is_empty() {
        return own;
    }
    for d in &mut own {
        d.changes.retain_mut(|c| match bound.get(&c.id) {
            Some(Some(p)) => {
                c.id.clone_from(p);
                true
            }
            Some(None) => false,
            None => true,
        });
    }
    own
}

/// The diff of consecutive ops from their own diffs ([`split_diff`]'s), each entry's `op`
/// renumbered to its op's position in `diffs`.
pub(crate) fn join_diffs(diffs: &[Diff]) -> Diff {
    let mut out = Diff::default();
    for (i, d) in diffs.iter().enumerate() {
        let op = u32::try_from(i).unwrap_or(u32::MAX);
        out.changes
            .extend(d.changes.iter().cloned().map(|c| Change { op, ..c }));
        out.shifts
            .extend(d.shifts.iter().cloned().map(|s| Shift { op, ..s }));
    }
    out
}

/// A committed record's ops, placeholders resolved, each with its own diff.
pub fn committed_ops(c: &CommittedTx) -> Result<Vec<(Op, Diff)>, CoreError> {
    let ops = resolve_ops(&c.tx.ops, &c.bindings)?;
    let diffs = split_diff(&c.tx.ops, &c.diff)?;
    Ok(ops.into_iter().zip(diffs).collect())
}

/// The footprints of a committed record's ops, in op order.
pub fn committed_footprints(c: &CommittedTx) -> Result<Vec<Footprint>, CoreError> {
    Ok(committed_ops(c)?
        .iter()
        .map(|(op, d)| footprint(op, d))
        .collect())
}

/// The first entity conflict of `ours` with `theirs` (intervening records in order, as
/// `(txId, footprint)`): its reason code and the txId to blame. `None` when there is none.
/// `own` holds the entities earlier ops of our transaction created (their `creates`, as
/// placeholders or ids); those are never judged.
pub fn entity_conflict(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &[(&str, Footprint)],
) -> Option<(&'static str, String)> {
    let theirs: Vec<(&str, &Footprint)> = theirs.iter().map(|(t, f)| (*t, f)).collect();
    entity_conflict_in(ours, own, &theirs)
}

/// [`entity_conflict`] over borrowed footprints (the engine's, shared between rebases).
pub(crate) fn entity_conflict_in(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &[(&str, &Footprint)],
) -> Option<(&'static str, String)> {
    // Entities ours needs, each with the record that removed it last (None once re-created).
    let mut needed: Vec<(&str, Option<usize>)> = ours
        .touches
        .iter()
        .map(|(x, _)| x.as_str())
        .chain(ours.uses.iter().map(String::as_str))
        .filter(|x| !own.contains(*x))
        .map(|x| (x, None))
        .collect();
    needed.sort_unstable();
    needed.dedup();
    let mut same: Option<(&'static str, usize)> = None;
    for (i, (_, f)) in theirs.iter().enumerate() {
        for (x, removed_by) in needed.iter_mut() {
            if f.creates.iter().any(|c| c == x) {
                *removed_by = None;
            }
            if f.removes.iter().any(|r| r == x) {
                *removed_by = Some(i);
            }
        }
        if same.is_none() {
            same = same_touch(ours, own, f).map(|reason| (reason, i));
        }
    }
    // The earliest record whose removal still stands.
    let removed = needed.iter().filter_map(|(_, by)| *by).min();
    removed
        .map(|i| (reasons::REMOVED, i))
        .or(same)
        .map(|(reason, i)| (reason, theirs[i].0.to_string()))
}

/// `SAME_EDGE` or `SAME_PARAM` when both touch the same edge or parameter of one entity.
fn same_touch(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &Footprint,
) -> Option<&'static str> {
    ours.touches.iter().find_map(|(x, t)| {
        let r = change_reason(t)?;
        (!own.contains(x) && theirs.touches.iter().any(|(y, u)| y == x && u == t)).then_some(r)
    })
}

/// The reason a change of an entity's edge or parameter makes; `None` for [`Touch::Exists`].
fn change_reason(t: &Touch) -> Option<&'static str> {
    match t {
        Touch::Exists => None,
        Touch::Edge(_) => Some(reasons::SAME_EDGE),
        Touch::Param(_) => Some(reasons::SAME_PARAM),
    }
}

/// The first intervening record (of `theirs`, in order) that changed an entity `ours` puts
/// back whole ([`Footprint::overwrites`]), one nested in it or what lies on it ([`changed`]),
/// with the txId to blame: `SAME_EDGE` when it moved an edge of any put-back entity, else
/// `SAME_PARAM`. Entities in
/// `own` are never judged (module docs).
pub(crate) fn overwrite_conflict(
    ours: &Footprint,
    own: &BTreeSet<String>,
    theirs: &[(&str, &Footprint)],
) -> Option<(&'static str, String)> {
    let put_back: Vec<&str> = ours
        .overwrites
        .iter()
        .filter(|x| !own.contains(*x))
        .map(String::as_str)
        .collect();
    if put_back.is_empty() {
        return None;
    }
    theirs.iter().find_map(|(tx, f)| {
        // An edge it moved names the change best, whichever put-back entity it was on (a trim
        // of an item on a put-back track also lists that track).
        let edge = put_back.iter().any(|x| moved_edge(x, f));
        let reason = if edge {
            reasons::SAME_EDGE
        } else if put_back.iter().any(|x| changed(x, f)) {
            reasons::SAME_PARAM
        } else {
            return None;
        };
        Some((reason, tx.to_string()))
    })
}

/// The item that op `k` of `ops` removes when the next op takes its range back into another
/// item: a lift `item.remove` followed by a `strict` trim that grows an end, as the inverse of
/// `item.split` and `caption.split` is. The trim puts the removed item's content back into the
/// left part, so a later change of the removed item by someone else would be lost without a
/// word: the engine judges the removal as a put-back of it ([`overwrite_conflict`]), and the
/// rule "removing an entity someone else edited is not a conflict" does not apply.
pub(crate) fn absorbed(ops: &[Op], k: usize) -> Option<&str> {
    let (Some(Op::ItemRemove(r)), Some(Op::ItemTrim(t))) = (ops.get(k), ops.get(k + 1)) else {
        return None;
    };
    let fills = t.mode == ops::TrimMode::Strict && t.edge == Edge::End && t.delta.is_positive();
    (r.mode == ops::DeleteMode::Lift && fills).then_some(r.id.as_str())
}

/// Whether `f` moved an edge of entity `x`.
fn moved_edge(x: &str, f: &Footprint) -> bool {
    f.touches
        .iter()
        .any(|(y, t)| y == x && matches!(t, Touch::Edge(_)))
}

/// Whether `f` changed entity `x`, one nested in it (`x/…`) or what lies on it (`x` among its
/// `tracks` or `uses`, for any put-back entity: an item put on a put-back track, a marker or
/// track on a put-back timeline, a placement in a put-back list, none of which the snapshot
/// holds): a touch of any kind, a creation or a removal.
fn changed(x: &str, f: &Footprint) -> bool {
    let within = |y: &str| y == x || y.strip_prefix(x).is_some_and(|r| r.starts_with('/'));
    f.touches.iter().any(|(y, _)| within(y))
        || f.creates.iter().chain(&f.removes).any(|y| within(y))
        || f.tracks.contains(x)
        || f.uses.iter().any(|y| y == x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::MemIds;
    use crate::model::{VideoInfo, new_project};
    use crate::time::Rational;
    use crate::tx::Tx;

    fn tx(ops: Vec<Op>) -> Tx {
        Tx {
            tx_id: "t".into(),
            author: "a".into(),
            branch: "main".into(),
            base_rev: 0,
            schema_version: crate::model::SCHEMA_VERSION,
            args_hash: None,
            ops,
        }
    }

    /// The ids a tx allocated at its base are given back as its placeholders before its
    /// diff joins a footprint: `c1` below is `$c`'s at the base only.
    #[test]
    fn own_footprints_name_placeholders_not_base_ids() {
        let s = Rational::from_int;
        let p = new_project("p".into(), "P".into(), s(30)).unwrap();
        let ops = vec![
            crate::ops::AssetAdd::media(
                "$a",
                "/m.mov",
                s(60),
                Some(VideoInfo {
                    width: 2,
                    height: 2,
                    rate: s(30),
                }),
                None,
                "f",
            )
            .into(),
            Op::ClipInsert {
                id: "$c".into(),
                track: "t1".into(),
                asset: "$a".into(),
                at: s(0),
                source_in: s(0),
                duration: s(5),
            },
            Op::ClipTrim {
                clip: "$c".into(),
                edge: Edge::End,
                delta: s(-1),
            },
        ];
        let applied = crate::apply_tx(&p, &tx(ops.clone()), &mut MemIds::after(&p)).unwrap();
        let split = split_diff(&ops, &applied.diff).unwrap();
        // As recorded: the base's ids.
        assert_eq!(footprint(&ops[2], &split[2]).touches.len(), 2);
        let own = unbind_own(&ops, split);
        let fps: Vec<Footprint> = ops.iter().zip(&own).map(|(o, d)| footprint(o, d)).collect();
        assert_eq!(fps[0].creates, ["$a"]);
        assert_eq!(fps[1].creates, ["$c"]);
        assert_eq!(fps[2].touches, [("$c".to_string(), Touch::Edge(Edge::End))]);
        let named = fps.iter().flat_map(|f| {
            f.touches
                .iter()
                .map(|(x, _)| x)
                .chain(&f.creates)
                .chain(&f.removes)
        });
        assert!(named.into_iter().all(|x| x.starts_with('$')), "{fps:?}");
        // Shifts are kept.
        assert_eq!(own[1].shifts.len(), 1);
        assert_eq!(own[2].shifts.len(), 1);
    }

    /// An allocating op that records several `added` changes: which is its placeholder's is
    /// unknown, so they are dropped.
    #[test]
    fn several_added_changes_of_an_allocating_op_are_dropped() {
        use crate::tx::{Change, EntityKind};
        let add = |id: &str| Change {
            kind: EntityKind::Item,
            id: id.into(),
            change: ChangeKind::Added,
            op: 0,
            track: None,
            edge: None,
            path: None,
            by: None,
        };
        let ops = vec![Op::ClipInsert {
            id: "$c".into(),
            track: "t1".into(),
            asset: "a1".into(),
            at: Rational::zero(),
            source_in: Rational::zero(),
            duration: Rational::from_int(1),
        }];
        let own = vec![Diff {
            changes: vec![add("c7"), add("c8")],
            shifts: vec![],
        }];
        assert!(unbind_own(&ops, own)[0].changes.is_empty());
        // Literal ids (an internal restore, no placeholder) are real and kept.
        let literal = vec![Op::ClipInsert {
            id: "c9".into(),
            track: "t1".into(),
            asset: "a1".into(),
            at: Rational::zero(),
            source_in: Rational::zero(),
            duration: Rational::from_int(1),
        }];
        let own = vec![Diff {
            changes: vec![add("c9")],
            shifts: vec![],
        }];
        assert_eq!(unbind_own(&literal, own.clone()), own);
    }
}
