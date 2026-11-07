//! `caption.setWords` (internal: the inverse of `caption.setText`, `caption.split` and
//! `caption.merge`): a caption's words, exactly. They must be at least one, ordered and not
//! overlapping (structural); times outside the item are kept (an undo restores what a start
//! trim moved out, Decision 13). The word times are item-relative, so a rebase maps them
//! through the content offsets of the steps between (Decision 16). The diff records the
//! item's `caption.words`; the inverse sets the old words.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::ids::IdAlloc;
use crate::model::{Project, Word};
use crate::ops::caption_set_text::WORDS;
use crate::ops::util::{OpDiff, block_mut, caption_at, check_caption_words, wrong_op};
use crate::ops::{Bindings, Op};
use crate::rebase::{Footprint, Touch};
use crate::time::Rational;
use crate::tx::Diff;

/// `caption.setWords`: `item`'s words become `words`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSetWords {
    pub item: String,
    pub words: Vec<Word>,
}

impl From<CaptionSetWords> for Op {
    fn from(op: CaptionSetWords) -> Op {
        Op::CaptionSetWords(op)
    }
}

pub(crate) fn apply(
    project: &mut Project,
    op: &Op,
    bindings: &mut Bindings,
    _ids: &mut dyn IdAlloc,
    diff: &mut OpDiff<'_>,
) -> Result<Vec<Op>, CoreError> {
    let Op::CaptionSetWords(s) = op else {
        return Err(wrong_op(op));
    };
    let id = bindings.resolve(&s.item)?;
    let (ti, i) = caption_at(project, &id)?;
    check_caption_words(&s.words, None)?;
    let track = ti.get(project).id.clone();
    let block = block_mut(project, ti, i)?;
    let old = std::mem::replace(&mut block.words, s.words.clone());
    diff.record_path(&track, &id, WORDS);
    Ok(vec![
        CaptionSetWords {
            item: id,
            words: old,
        }
        .into(),
    ])
}

/// Sets the item's `caption.words`.
pub(crate) fn footprint(op: &Op, op_diff: &Diff) -> Footprint {
    let mut f = Footprint::on(None, op_diff);
    if let Op::CaptionSetWords(s) = op {
        f.touches.push((s.item.clone(), Touch::Param(WORDS.into())));
    }
    f
}

/// No timeline positions.
pub(crate) fn positions_mut(_op: &mut Op) -> Vec<(String, &mut Rational)> {
    Vec::new()
}

/// Every word's start and end, relative to the item (Decision 16).
pub(crate) fn offsets_mut(op: &mut Op) -> Vec<(String, &mut Rational)> {
    match op {
        Op::CaptionSetWords(s) => {
            let item = s.item.clone();
            s.words
                .iter_mut()
                .flat_map(|w| [&mut w.start, &mut w.end])
                .map(|t| (item.clone(), t))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// The inverse is one `caption.setWords`.
pub(crate) fn inverse_len(_op: &Op, _own: &Diff) -> usize {
    1
}

/// The wire name.
pub(crate) const NAME: &str = "caption.setWords";
/// Whether the op is produced only as an inverse and never accepted from clients.
pub(crate) const INTERNAL: bool = true;

/// Every id reference the op carries, mutably.
pub(crate) fn refs_mut(op: &mut Op) -> Vec<&mut String> {
    match op {
        Op::CaptionSetWords(s) => vec![&mut s.item],
        _ => Vec::new(),
    }
}

/// Every id the op names (the fields [`refs_mut`] lists).
pub(crate) fn ids(op: &Op) -> Vec<&str> {
    match op {
        Op::CaptionSetWords(s) => vec![&s.item],
        _ => Vec::new(),
    }
}

/// The placeholder the op binds to a newly allocated id, if any.
pub(crate) fn allocates(_: &Op) -> Option<&str> {
    None
}
