//! The reference model of the undo/redo properties (spec §4.6 prop 4; plan 4B Task 20, plan
//! 4C Task 26), shared by `props_history` (4B ops) and `props_history_4c` (with moves and
//! splits). 2–4 authors interleave edits, undos, redos, cherry-picks (side branch → main) and
//! merges (side branch → main, and main → side branch, so a merge can carry copies back and
//! exclude undos of them) on `main` and a side branch `br1`, through the real planners over a
//! `MemLog`. After every step the heads, the stacks and every conflict reason match the model.
//!
//! **Edits** are gapless ripple edits, one op each: inserts at a clip boundary or at the track
//! end, deletes and end trims (4B); in 4C also moves (`item.move`, ripple, of a clip to either
//! track, before one of its clips or at its end, its own place included) and splits
//! (`item.split` at a whole frame strictly inside the clip; the right part gets a new id). Both
//! tracks hold clips of `a1`, which has video and audio, so a clip fits either track. A track
//! is then exactly its ordered list of `(clipId, duration)`.
//!
//! **The model** knows nothing of positions. It keeps:
//! - per branch, the ordered clip lists, changed by list operations: insert an id before its
//!   recorded successor id (or at the end), remove an id, add to a duration; a move removes
//!   the id and inserts it on its target track before its target clip (the clip itself: where
//!   it was) or at the end; a split shortens the clip and inserts the right part after it;
//! - per branch and clip, the end trims in effect (each by the edit whose effect it is: a copy
//!   keeps it, an undo takes it back); a split moves its clip's end edge, so it counts as one;
//! - per author and branch, undo and redo stacks of unit ids (Decisions 22–23);
//! - per record, the list operations it made, so a unit's inverse is a list of list
//!   operations (a removal records the successor it had, and its inverse reinserts before it).
//!   A move inverts to a removal and a reinsertion before the successor it had, with the
//!   length it had then (`item_move`'s inverse: `item.remove`, then `item.restore` of the
//!   item as it was). A split inverts to a *lift* removal of the right part (a gap unless it is
//!   last) and a *strict* trim of the left part by the right part's length, which fills the
//!   gap and overlaps (`CLIP_OVERLAP`) whatever else is there (`item_split`'s inverse, B4); a
//!   redo of that shrinks the left part and lift-restores the right part into the gap.
//!
//! **Placement.** A reinsertion is certain while its successor is present and was neither
//! removed nor moved since (a copy: never on either side); an insertion at the end, while its
//! track saw no no-shift op (a split or its inverse: those leave positions where they were, so
//! a removed boundary is not mapped away) since (a copy: ever, on either side). A lift restore
//! goes right after the left part it was cut from (else at the end) and is certain on the same
//! terms (right after: also while that track saw no no-shift op but the undo's and the redo's
//! own strict trims), never in a copy. Otherwise the model checks membership and durations only, then
//! takes the real order (the ProseMirror tie-break: Bob deletes c3, Alice deletes c2, Bob
//! undoes, Alice undoes gives `c1, c3, c2`, which the recorded successors predict exactly).
//!
//! **Two oracles for conflicts.**
//! - *Independent* (`MBranch::verdict`): from the clip lists and edge histories alone, an op
//!   cannot be carried when its clip is gone (remove, trim, move, split), is there already (a
//!   restore, a split's right part), or its end edge was changed by someone else since (a trim
//!   or a split; an undo or redo: since the edit it inverts; a copy: since what the source had
//!   before it); and a copied undo or redo cannot be carried when the target does not hold its
//!   unit. An edge history is the trims in effect: a restore re-creates a clip without touching
//!   its edge (spec §4.3: the last removal or creation decides `REMOVED`, only trims make
//!   `SAME_EDGE`), and a clip the record itself re-created at an earlier op is not judged (60k
//!   cases found both: an undo of a merge group that removes, restores, then trims a clip
//!   another author trimmed since; and an undo of a trim whose clip a merged delete and restore
//!   re-created). Wherever it says so, the real planner must refuse, with a reason of that
//!   kind. It allows the planner to refuse more (it is conservative: two unmirrored records may
//!   cancel in effect), so it cannot predict every conflict. A move's `start` has no history
//!   here: an undo of a move is a removal and a restore, which never conflict (spec §4.3), so
//!   no history of moves in effect predicts `SAME_PARAM` soundly (main's move and its undo
//!   cancel; a side undo of a merged move does not count).
//! - *Transcribed* (shared with the implementation's rules, so a bug in those rules could be
//!   shared too): the exact reason and blamed record come from abstract footprints (`LOp::fp`:
//!   a restore creates its clip, a removal needs and removes it, a trim needs its end edge, a
//!   move sets its `start`, a split needs its clip, moves its end edge and creates the right
//!   part) over the records between a unit and the head, or over a copy's merge steps (the
//!   source records before it inverted, the target's records, the copies so far), paired as
//!   Decision 24 says (an undo with its units, cut by each unit's inverse length; a redo with
//!   its undo; a copy with its inverted original; a target record with the inverted source
//!   record of the same origin root), plus the candidate rules (present by root;
//!   `UNDOES_TARGET_RECORDS`; an undo or redo of a skipped unit is `DEPENDS_ON_SKIPPED`, found
//!   by this property: a skipped delete's undo re-created a clip the target had deleted). A
//!   re-run failure blames the one unmirrored record that touched, created or removed a clip
//!   the op touches or its error names (`rerun_conflict`). The heads, which the transcribed
//!   part never computes, are checked by the list model alone.
//!
//! **Exclusions** (4C; none of them arises from 4B ops):
//! - *Gaps.* The run stops at the first state with a gap: a lift removal the strict trim does
//!   not fill (an undo of a split after the right part was lengthened, or moved elsewhere
//!   than a track end), or a redo whose left part shrinks with no right part to fill it. The
//!   model predicts those gaps and the outcome of the step that makes them (and the run stops
//!   on any real gap on a track whose order it is unsure of); what follows a gap is not
//!   modelled.
//! - *Positions the model cannot follow* (`MBranch::risky`): a split and its inverse record no
//!   shift, so a boundary a split added is not there for a position mapped back past it (a
//!   copy whose source split a clip; a restore before a right part whose split was undone),
//!   and the position lies inside a clip. Such an op, when its placement is uncertain on a
//!   track that saw a no-shift op (any lift restore, in a copy), may be refused with
//!   `REMOVED` (mapped strictly inside a removed range), `INSERT_INSIDE_CLIP`,
//!   `MOVE_INTO_ITSELF` or `CLIP_OVERLAP` (or `DEPENDS_ON_SKIPPED` when blamed on a skipped
//!   record); the model takes that refusal as it is (`Rerun::takes`). Its success is checked
//!   as an uncertain placement.
//! - *Undo of a move* drops what others did to the clip since (it restores the clip as it was:
//!   another author's trim or move of it is lost, with no conflict). The model transcribes it;
//!   `props_history_4c` keeps the repro (`undo_of_move_keeps_a_later_trim`, ignored).
//!
//! Not driven here: `revert` (its chains have their own oracle, `pick_props`), `undo --to`
//! (`undo_to_props`: equal to the undos in a row), and lineages deeper than one fork
//! (`merge.rs` cases; the store's prop 5, `props_replay`, drives all of them end to end).
#![allow(dead_code)]

pub mod branch;
pub mod driver;
pub mod lops;
pub mod model;
pub mod rules;

use fin_core::{Project, Rational};

use crate::common::{apply, av_asset, insert, project, secs};

pub const AUTHORS: [&str; 4] = ["alice", "bob", "carol", "dave"];
pub const BR: [&str; 2] = ["main", "br1"];
pub const TRACKS: [&str; 2] = ["t1", "t2"];
pub const MAIN: usize = 0;
pub const SIDE: usize = 1;
/// Asset `a1` is 100 s long and every clip starts 1 s into it; a split's right part starts
/// later in it but ends where its clip did, so a clip's source end never passes the asset's
/// within a script (at most 48 trims of 9 frames).
pub const MAX_FRAMES: i64 = 99 * 30;

pub fn frames(n: i64) -> Rational {
    Rational::new(n, 30).unwrap()
}

pub fn to_frames(r: Rational) -> i64 {
    assert_eq!((r.num() * 30) % r.den(), 0, "{r} is not a whole frame");
    r.num() * 30 / r.den()
}

/// `a1` (video and audio), and clips of 40, 50, 30 frames on `t1` (video) and 60, 45 on
/// `t2` (audio).
pub fn genesis() -> Project {
    let mut ops = vec![av_asset("$a", secs(100))];
    for (t, lens) in [[40, 50, 30].as_slice(), &[60, 45]].iter().enumerate() {
        let mut at = 0;
        for (i, n) in lens.iter().enumerate() {
            let id = format!("$c{t}{i}");
            ops.push(insert(
                &id,
                TRACKS[t],
                frames(at),
                "$a",
                secs(1),
                frames(*n),
            ));
            at += n;
        }
    }
    apply(&project(), ops).project
}
