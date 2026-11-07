//! `caption.split`: cuts a caption in two before word `word` (plan 4C Task 19), where that
//! word starts. The left part keeps the id, the words before it, and ends there; the right
//! part (named by `id`, a `$name` placeholder bound to a new `cap<n>`) is a copy of the item
//! from there with the rest of the words, item-relative times moved by the cut (Decision 13),
//! and no link group (Decision 17).
//!
//! `word` lies strictly inside the word list and the cut strictly inside the item (structural,
//! `INVALID_RANGE`); each part is at least a frame long and the track unlocked (policy). The
//! word index is not a time: a stale split re-runs on the words it finds. The inverse is
//! internal: `item.remove` of the right part (lift), a `strict` trim of the left part's end
//! back, and `caption.setWords` with the old words.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::CoreError;
use crate::ids::{IdAlloc, IdKind};
use crate::model::{ItemBody, Project};
use crate::ops::caption_set_words::CaptionSetWords;
use crate::ops::util::{
    OpDiff, caption_at, check_unlocked, frame_of, invalid_range, place, shift_content, wrong_op,
};
use crate::ops::{Bindings, DeleteMode, Edge, ItemRemove, ItemTrim, Op, TrimMode};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::{ChangeKind, Diff};

/// `caption.split`: cuts `item` before word `word`; the right part is `id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSplit {
    pub item: String,
    pub word: u32,
    pub id: String,
}

impl From<CaptionSplit> for Op {
    fn from(op: CaptionSplit) -> Op {
        Op::CaptionSplit(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::CaptionSplit(s) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&s.item)?;
    let (ti, ci) = caption_at(project, &id)?;
    let track = ti.get(project);
    let it = &track.items[ci];
    let ItemBody::Caption { block } = &it.body else {
        return Err(wrong_op(op));
    };
    let k = usize::try_from(s.word).unwrap_or(usize::MAX);
    let n = block.words.len();
    if k == 0 || k >= n {
        return Err(invalid_range(
            format!(
                "word {} is not strictly inside caption {id}'s {n} words",
                s.word
            ),
            json!({ "item": id, "word": s.word, "min": 1, "max": n.saturating_sub(1) }),
            format!("split before a word from 1 to {}", n.saturating_sub(1)),
        ));
    }
    let offset = block.words[k].start;
    if !offset.is_positive() || offset >= it.duration {
        return Err(invalid_range(
            format!(
                "word {} starts at {offset}, not strictly inside item {id}",
                s.word
            ),
            json!({ "item": id, "word": s.word, "offset": offset, "min": "0", "max": it.duration,
                    "exclusive": true }),
            "split before a word that starts inside the caption",
        ));
    }
    if bindings.policy() {
        check_unlocked(track, Some(&id), bindings)?;
        let frame = frame_of(project, ti)?;
        let max = it.duration.checked_sub(frame)?;
        if offset < frame || offset > max {
            return Err(invalid_range(
                format!("splitting caption {id} at {offset} leaves a part under one frame"),
                json!({ "item": id, "word": s.word, "offset": offset, "min": frame, "max": max }),
                "split before another word, or trim the caption first",
            ));
        }
    }
    bindings.check_new(project, &s.id, IdKind::Caption)?;
    let right_id = bindings.create(project, &s.id, IdKind::Caption, ids)?;
    let track = ti.get_mut(project);
    let left = &mut track.items[ci];
    let mut right = left.clone();
    let mut old = Vec::new();
    if let (ItemBody::Caption { block: l }, ItemBody::Caption { block: r }) =
        (&mut left.body, &mut right.body)
    {
        old.clone_from(&l.words);
        r.words = l.words.split_off(k);
    }
    right.id.clone_from(&right_id);
    right.start = left.start.checked_add(offset)?;
    right.duration = left.duration.checked_sub(offset)?;
    shift_content(&mut right, offset.checked_neg()?)?;
    right.common.link = None;
    left.duration = offset;
    let length = right.duration;
    place(track, right);
    diff.record_edge(&track.id, &id, Edge::End, None);
    diff.record_item(&track.id, &right_id, ChangeKind::Added);
    Ok(vec![
        ItemRemove {
            id: right_id,
            mode: DeleteMode::Lift,
        }
        .into(),
        ItemTrim {
            item: id.clone(),
            edge: Edge::End,
            delta: length,
            mode: TrimMode::Strict,
            alone: false,
        }
        .into(),
        CaptionSetWords {
            item: id,
            words: old,
        }
        .into(),
    ])
}

/// Needs the item and moves its end (its diff's edge change); creates the right part.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::CaptionSplit(s) = op {
        f.touches.push((s.item.clone(), Touch::Exists));
        f.creates.push(s.id.clone());
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values: the cut is a word index.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// `item.remove`, a strict `item.trim` and `caption.setWords`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    3
}

/// The wire name.
pub(crate) const NAME: &str = "caption.split";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably: the item and the right part's new id.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::CaptionSplit(s) => vec![&mut s.item, &mut s.id],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::CaptionSplit(s) => vec![&s.item, &s.id],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(op: &Op) -> Option<&str> {
    match op {
        Op::CaptionSplit(s) => Some(s.id.as_str()).filter(|id| id.starts_with('$')),
        _ => None,
    }
}
