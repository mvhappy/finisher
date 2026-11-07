//! `compose::caption_import`: `fin caption import` (spec §5.5). An op allocates at most one id
//! (plan 4C Decision 17), so the import expands here into one `item.insert` per block.

use serde_json::json;

use crate::captions::{blocks, find_style};
use crate::compose::Composed;
use crate::error::CoreError;
use crate::model::{CaptionBlock, ItemBody, Project, Word};
use crate::ops::util::track_index;
use crate::ops::{InsertMode, ItemInsert};
use crate::time::Rational;

/// Ops for importing `words` (times from the transcript's start) as captions of `style` (a
/// preset or a project style) on `track`, the transcript's time 0 at `at`: the words split
/// into blocks by the style's rule ([`blocks`]), each an `item.insert` in `overwrite` mode
/// named `$cap<n>` (n from 1, in time order) with its words relative to its start. `echo` is
/// `{ mode: "overwrite", style, blocks }`. `NOT_FOUND` for an unknown track or style; every
/// other check is the inserts' own (a caption track, a block at least one frame long).
pub fn caption_import(
    project: &Project,
    track: &str,
    at: Rational,
    words: &[Word],
    style: &str,
) -> Result<Composed, CoreError> {
    track_index(project, track)?;
    let drafts = blocks(words, find_style(project, style)?);
    let mut ops = Vec::with_capacity(drafts.len());
    for (n, b) in drafts.into_iter().enumerate() {
        ops.push(
            ItemInsert {
                id: format!("$cap{}", n + 1),
                track: track.to_string(),
                at: at.checked_add(b.start)?,
                duration: b.end.checked_sub(b.start)?,
                mode: InsertMode::Overwrite,
                body: ItemBody::Caption {
                    block: CaptionBlock {
                        words: b.words,
                        style: style.to_string(),
                        overrides: Default::default(),
                        source: None,
                    },
                },
            }
            .into(),
        );
    }
    let echo = json!({ "mode": "overwrite", "style": style, "blocks": ops.len() });
    Ok(Composed { ops, echo })
}
