//! List operations, their inverses and their abstract footprints (module docs in `mod.rs`).

use fin_core::rebase::reasons;

/// A list operation on one branch. Stamps say how sure a later re-placement is (module docs,
/// *Placement*): `stamp` how often the anchor clip had been removed (or moved) on its branch
/// when the operation was planned or made, [`UNSURE`] when the model did not know it; `ns`
/// how many no-shift ops (splits and their inverses) its track had seen.
#[derive(Debug, Clone, PartialEq)]
pub enum LOp {
    /// `item.insert` (ripple), or the ripple `item.restore` of an inverse: before `before`,
    /// or at the track end. A restore puts the clip back whole, its end edge included:
    /// `expect`, the edge history it needs (as the removal or move it inverts left it);
    /// `pre`, the history as made (what a copy needs). A restore (`expect` set) goes right
    /// after `after`, the clip before it when it was taken out, while that is on the track
    /// (`rebase::placement`, owner decision 2026-11-09); else before `before`.
    Insert {
        track: usize,
        id: String,
        dur: i64,
        after: Option<String>,
        before: Option<String>,
        stamp: u32,
        ns: u32,
        expect: Option<Vec<String>>,
        pre: Vec<String>,
    },
    /// `item.delete` (ripple), or the ripple `item.remove` of an inverse; `succ`: the clip
    /// after it when made, `pred` the clip before it (none after a gap, or on a track whose
    /// order the model did not know); `post`: its edge history then.
    Remove {
        track: usize,
        id: String,
        dur: i64,
        pred: Option<String>,
        succ: Option<String>,
        stamp: u32,
        ns: u32,
        post: Vec<String>,
    },
    /// An end trim (`strict`: the internal trim of a split's inverse, which moves nothing
    /// else). `eff`: the edit whose effect it is (a copy keeps it, an undo takes it back:
    /// `push` false); `expect`: the edge history it needs (an undo or redo: as the edit it
    /// inverts left it); `pre`/`post`: the history around it as made.
    Trim {
        id: String,
        delta: i64,
        strict: bool,
        eff: String,
        push: bool,
        expect: Option<Vec<String>>,
        pre: Vec<String>,
        post: Vec<String>,
    },
    /// `item.move` (ripple) to track `to`, before `before` (the clip itself: where it is) or
    /// at the end. As made: the track it left (`from`), its length (`dur`) and the clip
    /// after it there (`succ`, with `sstamp`/`sns`), which its inverse restores it before;
    /// `post`: its edge history then (its inverse's restore needs it unchanged). `after`: the
    /// clip before its target as made (never the clip itself), which a copy goes right after
    /// while it is there; `spred`: the clip before it where it left, which its inverse's
    /// restore goes right after.
    Move {
        id: String,
        to: usize,
        after: Option<String>,
        spred: Option<String>,
        before: Option<String>,
        stamp: u32,
        ns: u32,
        from: usize,
        dur: i64,
        succ: Option<String>,
        sstamp: u32,
        sns: u32,
        post: Vec<String>,
    },
    /// `item.split` of `id` at `off` frames; the right part `right` (`rlen` frames, as made)
    /// goes right after it. Its end edge moves: `eff`, `pre`, `post` as for a trim.
    Split {
        id: String,
        right: String,
        off: i64,
        rlen: i64,
        eff: String,
        pre: Vec<String>,
        post: Vec<String>,
    },
    /// The lift `item.remove` of a split's inverse (the right part, `partner` the left one):
    /// it leaves a gap unless the clip is last. As made: `after`, the left part when it was
    /// right before the clip (else the clip was last: its restore goes to the track end).
    RemoveLift {
        track: usize,
        id: String,
        dur: i64,
        partner: String,
        after: Option<String>,
        stamp: u32,
        ns: u32,
    },
    /// The lift `item.restore` of a redo: into the gap right after `after`, or at the end.
    RestoreLift {
        track: usize,
        id: String,
        dur: i64,
        partner: String,
        after: Option<String>,
        stamp: u32,
        ns: u32,
    },
}

pub const UNSURE: u32 = u32::MAX;

/// How an op touches a clip (spec §4.3, `rebase::Touch`): needs it, one edge of it (the end:
/// only end edges are edited here), one parameter of it (`start`: a move).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum T {
    Exists,
    Edge,
    Param,
}

/// An op's abstract footprint: what it touches, creates and removes, and what it puts back
/// whole (`Footprint::overwrites`: a restore's clip).
#[derive(Debug, Clone, Default)]
pub struct Fp {
    pub touches: Vec<(String, T)>,
    pub creates: Vec<String>,
    pub removes: Vec<String>,
    pub overwrites: Vec<String>,
}

/// What the engine may answer when the independent oracle says an op is impossible. A clip
/// gone may also be the re-run's own `NOT_FOUND` (Decision 17): a copy's entity judgement
/// leaves out mirrored pairs, and when one pair's two steps lie around another removal of the
/// clip (an inverted source undo that `UNDOES_TARGET_RECORDS` excluded), the judgement misses
/// that the clip is gone (Task 22 follow-up: name the excluded record instead).
pub const GONE: &[&str] = &[reasons::REMOVED, reasons::DEPENDS_ON_SKIPPED, "NOT_FOUND"];
/// [`GONE`] for an op that also touches the end edge (a trim, a split): when the judgement
/// misses that the clip is gone (as above), an unmirrored trim of its end still meets it (an
/// undo, in the source's path back, of a merged delete whose own pair lies around it).
pub const GONE_EDGE: &[&str] = &[
    reasons::REMOVED,
    reasons::DEPENDS_ON_SKIPPED,
    "NOT_FOUND",
    reasons::SAME_EDGE,
];
/// What a restore may meet when its clip's end edge changed since the edit it inverts: the
/// change itself (`SAME_EDGE`), or a move of the clip met first (`SAME_PARAM`), as for
/// [`EDGE`].
pub const RESTORE: &[&str] = &[
    reasons::SAME_EDGE,
    reasons::SAME_PARAM,
    reasons::REMOVED,
    reasons::DEPENDS_ON_SKIPPED,
];
/// [`GONE`] for a move: likewise, an unmirrored move of the clip still meets it.
pub const GONE_PARAM: &[&str] = &[
    reasons::REMOVED,
    reasons::DEPENDS_ON_SKIPPED,
    "NOT_FOUND",
    reasons::SAME_PARAM,
];
pub const THERE: &[&str] = &[
    reasons::ALREADY_PRESENT,
    reasons::ID_COLLISION,
    reasons::DEPENDS_ON_SKIPPED,
];
pub const EDGE: &[&str] = &[
    reasons::SAME_EDGE,
    reasons::REMOVED,
    reasons::DEPENDS_ON_SKIPPED,
];
pub const UNHELD: &[&str] = &[reasons::DEPENDS_ON_SKIPPED];
/// What a position the model cannot follow may meet (module docs, *Exclusions*): mapped
/// strictly inside a removed range, or re-run inside an item; a lift restore that overlaps;
/// any of them blamed on a skipped record in a merge.
pub const POSITION: &[&str] = &[
    reasons::REMOVED,
    "INSERT_INSIDE_CLIP",
    "MOVE_INTO_ITSELF",
    "CLIP_OVERLAP",
    reasons::DEPENDS_ON_SKIPPED,
];

/// An independent verdict: op `k` cannot be carried, the reasons the engine may give, and
/// whether its position is one the model cannot follow (then [`POSITION`] is allowed too).
pub type Verdict = Option<(usize, &'static [&'static str], bool)>;

impl LOp {
    /// The inverse as planned, in the engine's op order (a move: remove, then restore where
    /// it was; a split: the right part's lift remove, then the left part's strict trim).
    pub fn inverse(&self) -> Vec<LOp> {
        match self.clone() {
            LOp::Insert { track, id, dur, .. } => vec![LOp::Remove {
                track,
                id,
                dur,
                pred: None,
                succ: None,
                stamp: 0,
                ns: 0,
                post: Vec::new(),
            }],
            LOp::Remove {
                track,
                id,
                dur,
                pred,
                succ,
                stamp,
                ns,
                post,
            } => vec![LOp::Insert {
                track,
                id,
                dur,
                after: pred,
                before: succ,
                stamp,
                ns,
                expect: Some(post),
                pre: Vec::new(),
            }],
            LOp::Trim {
                id,
                delta,
                strict,
                eff,
                push,
                post,
                ..
            } => vec![LOp::Trim {
                id,
                delta: -delta,
                strict,
                eff,
                push: !push,
                expect: Some(post),
                pre: Vec::new(),
                post: Vec::new(),
            }],
            LOp::Move {
                id,
                to,
                from,
                dur,
                spred,
                succ,
                sstamp,
                sns,
                post,
                ..
            } => vec![
                LOp::Remove {
                    track: to,
                    id: id.clone(),
                    dur,
                    pred: None,
                    succ: None,
                    stamp: 0,
                    ns: 0,
                    post: Vec::new(),
                },
                LOp::Insert {
                    track: from,
                    id,
                    dur,
                    after: spred,
                    before: succ,
                    stamp: sstamp,
                    ns: sns,
                    expect: Some(post),
                    pre: Vec::new(),
                },
            ],
            LOp::Split {
                id,
                right,
                rlen,
                eff,
                post,
                ..
            } => vec![
                LOp::RemoveLift {
                    track: 0,
                    id: right,
                    dur: rlen,
                    partner: id.clone(),
                    after: None,
                    stamp: 0,
                    ns: 0,
                },
                LOp::Trim {
                    id,
                    delta: rlen,
                    strict: true,
                    eff,
                    push: false,
                    expect: Some(post),
                    pre: Vec::new(),
                    post: Vec::new(),
                },
            ],
            LOp::RemoveLift {
                track,
                id,
                dur,
                partner,
                after,
                stamp,
                ns,
            } => vec![LOp::RestoreLift {
                track,
                id,
                dur,
                partner,
                after,
                stamp,
                ns,
            }],
            LOp::RestoreLift {
                track,
                id,
                dur,
                partner,
                ..
            } => vec![LOp::RemoveLift {
                track,
                id,
                dur,
                partner,
                after: None,
                stamp: 0,
                ns: 0,
            }],
        }
    }

    /// How many ops the inverse holds (`ops::inverse_len`): a move and a split invert to two.
    pub fn inverse_len(&self) -> usize {
        match self {
            LOp::Move { .. } | LOp::Split { .. } => 2,
            _ => 1,
        }
    }

    /// The footprint, transcribed from the op modules' `footprint` plus what their diffs
    /// record (`Footprint::with_changes`): an insert or restore creates its clip, and a
    /// restore puts it back whole (`item_restore::footprint`; a client insert's clip is new,
    /// so no record before it can have touched it, and the model need not tell them apart); a removal
    /// needs and removes it; a trim needs its end edge; a move sets the clip's `start`
    /// (`item_move::footprint`, and its diff's `path: "start"` changes); a split needs the
    /// clip, moves its end edge (its diff's edge change) and creates the right part
    /// (`item_split::footprint`).
    pub fn fp(&self) -> Fp {
        let mut f = Fp::default();
        match self {
            LOp::Insert { id, .. } | LOp::RestoreLift { id, .. } => {
                f.creates.push(id.clone());
                f.overwrites.push(id.clone());
            }
            LOp::Remove { id, .. } | LOp::RemoveLift { id, .. } => {
                f.touches.push((id.clone(), T::Exists));
                f.removes.push(id.clone());
            }
            LOp::Trim { id, .. } => f.touches.push((id.clone(), T::Edge)),
            LOp::Move { id, .. } => f.touches.push((id.clone(), T::Param)),
            LOp::Split { id, right, .. } => {
                f.touches.push((id.clone(), T::Exists));
                f.touches.push((id.clone(), T::Edge));
                f.creates.push(right.clone());
            }
        }
        f
    }

    /// The clip the op names first.
    pub fn clip(&self) -> &str {
        match self {
            LOp::Insert { id, .. }
            | LOp::Remove { id, .. }
            | LOp::Trim { id, .. }
            | LOp::Move { id, .. }
            | LOp::Split { id, .. }
            | LOp::RemoveLift { id, .. }
            | LOp::RestoreLift { id, .. } => id,
        }
    }
}
