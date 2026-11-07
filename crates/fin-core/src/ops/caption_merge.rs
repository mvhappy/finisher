//! `caption.merge`: joins a caption with the next caption on its track (plan 4C Task 19). The
//! result keeps `item`'s id, style and fields, spans to `with`'s end, and holds both captions'
//! words (`with`'s moved to count from `item`'s start); `with` goes. Any other `with` (an
//! earlier item, a later one with an item between, another track, itself) is `NOT_ADJACENT`;
//! a locked track `TRACK_LOCKED` (policy).
//!
//! At the seam only the words each caption shows are joined (the rule measurement and eval
//! use, [`crate::captions::shown_words`]), so the result stays in order and shows what both
//! showed: `item`'s words a trim left past its end and `with`'s left before its start
//! (Decision 13) are dropped, and a word across the seam is cut at it. The hidden words at
//! the outer edges are kept as they are, hidden still. Both sources of one
//! transcript join into one word range ([`merge_source`]). The diff records `item`'s end and
//! `caption.words`, so a concurrent `caption.setText` of it is `SAME_PARAM`. The inverse is
//! internal: `caption.setWords` with the old words and source, a `strict` trim of the end
//! back, and `item.restore` of `with` (lift), so an undo meets later edits of that range as a
//! conflict (B4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::captions::{hidden_after_end, hidden_before_start};
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{ItemBody, Project, Word};
use crate::ops::caption_set_text::WORDS;
use crate::ops::caption_set_words::CaptionSetWords;
use crate::ops::util::{OpDiff, caption_at, check_unlocked, end, merge_source, wrong_op};
use crate::ops::{Bindings, DeleteMode, Edge, ItemRestore, ItemTrim, Op, TrimMode};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

/// `caption.merge`: `item` takes in `with`, the next caption on its track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionMerge {
    pub item: String,
    pub with: String,
}

impl From<CaptionMerge> for Op {
    fn from(op: CaptionMerge) -> Op {
        Op::CaptionMerge(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::CaptionMerge(m) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&m.item)?;
    let with = bindings.resolve(&m.with)?;
    let (ti, i) = caption_at(project, &id)?;
    let (tw, j) = caption_at(project, &with)?;
    if tw != ti || j != i + 1 {
        return Err(CoreError::new(
            "NOT_ADJACENT",
            format!("{with} is not the caption right after {id} on its track"),
        )
        .with_details(json!({ "item": id, "with": with }))
        .with_fix("merge a caption with the next caption on the same track"));
    }
    let track = ti.get_mut(project);
    check_unlocked(track, Some(&id), bindings)?;
    let next = track.items.remove(j);
    let (first, stop) = (&mut track.items[i], end(&next)?);
    let by = next.start.checked_sub(first.start)?;
    let delta = stop.checked_sub(end(first)?)?;
    let length = first.duration;
    let (mut old, mut old_source) = (Vec::new(), None);
    if let (ItemBody::Caption { block }, ItemBody::Caption { block: theirs }) =
        (&mut first.body, &next.body)
    {
        old.clone_from(&block.words);
        old_source.clone_from(&block.source);
        // At the seam, what either caption hides there goes and a word across it is cut;
        // the outer edges' hidden words stay as they are, hidden still.
        let mut words: Vec<_> = block
            .words
            .iter()
            .filter(|w| !hidden_after_end(w, length))
            .map(|w| Word {
                end: w.end.min(length),
                ..w.clone()
            })
            .collect();
        for w in theirs.words.iter().filter(|w| !hidden_before_start(w)) {
            words.push(Word {
                text: w.text.clone(),
                start: w.start.max(Rational::zero()).checked_add(by)?,
                end: w.end.checked_add(by)?,
            });
        }
        if words.is_empty() {
            return Err(CoreError::new(
                "INVALID_OP",
                format!("merging {id} with {with} leaves no word inside the caption"),
            )
            .with_details(json!({ "item": id, "with": with, "path": "words" }))
            .with_fix("edit the captions' text first, or delete one of them"));
        }
        block.words = words;
        block.source = merge_source(old_source.as_ref(), theirs.source.as_ref());
    }
    first.duration = stop.checked_sub(first.start)?;
    let track_id = track.id.clone();
    diff.record_edge(&track_id, &id, Edge::End, None);
    diff.record_item(&track_id, &with, ChangeKind::Removed);
    diff.record_path(&track_id, &id, WORDS);
    Ok(vec![
        CaptionSetWords {
            item: id.clone(),
            words: old,
            source: old_source,
        }
        .into(),
        ItemTrim {
            item: id,
            edge: Edge::End,
            delta: delta.checked_neg()?,
            mode: TrimMode::Strict,
            alone: false,
        }
        .into(),
        ItemRestore {
            item: next,
            track: track_id,
            mode: DeleteMode::Lift,
        }
        .into(),
    ])
}

/// Needs both captions, moves `item`'s end, sets its words and removes `with`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::CaptionMerge(m) = op {
        f.touches.push((m.item.clone(), Touch::Exists));
        f.touches.push((m.with.clone(), Touch::Exists));
        f.removes.push(m.with.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// `caption.setWords`, a strict `item.trim` and `item.restore`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    3
}

/// The wire name.
pub(crate) const NAME: &str = "caption.merge";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::CaptionMerge(m) => vec![&mut m.item, &mut m.with],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::CaptionMerge(m) => vec![&m.item, &m.with],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
