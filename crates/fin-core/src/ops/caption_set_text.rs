//! `caption.setText`: new text for a caption (plan 4C Task 19, spec §5.5). The words are the
//! text split on whitespace, timed against the old ones by [`crate::captions::align_text`]:
//! matching words keep their times, the rest share the time around them by characters, on a
//! millisecond grid. No
//! words, or a control character, is `INVALID_OP`; a locked track `TRACK_LOCKED` (policy).
//! The transcript source is kept: the caption still covers the same transcript words. The diff
//! records the item's `caption.words`; the inverse is the internal `caption.setWords` with the
//! old words.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::captions::align_text;
use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::Project;
use crate::ops::caption_set_words::CaptionSetWords;
use crate::ops::util::{
    OpDiff, block_mut, caption_at, check_caption_words, check_unlocked, wrong_op,
};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// The registry-style param key a caption's words are changed under (Decision 20).
pub(crate) const WORDS: &str = "caption.words";

/// `caption.setText`: `item`'s words become `text`'s.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSetText {
    pub item: String,
    pub text: String,
}

impl From<CaptionSetText> for Op {
    fn from(op: CaptionSetText) -> Op {
        Op::CaptionSetText(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::CaptionSetText(s) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&s.item)?;
    let (ti, i) = caption_at(project, &id)?;
    check_unlocked(ti.get(project), Some(&id), bindings)?;
    let track = ti.get(project).id.clone();
    let block = block_mut(project, ti, i)?;
    let words = align_text(&block.words, &s.text)?;
    check_caption_words(&words, None)?;
    let old = std::mem::replace(&mut block.words, words);
    diff.record_path(&track, &id, WORDS);
    Ok(vec![
        CaptionSetWords {
            item: id,
            words: old,
            source: block.source.clone(),
        }
        .into(),
    ])
}

/// Sets the item's `caption.words`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::CaptionSetText(s) = op {
        f.touches.push((s.item.clone(), Touch::Param(WORDS.into())));
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// No item-relative values: the times are the item's own at apply.
pub(crate) fn offsets_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// The inverse is one `caption.setWords`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "caption.setText";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = false;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::CaptionSetText(s) => vec![&mut s.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::CaptionSetText(s) => vec![&s.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
