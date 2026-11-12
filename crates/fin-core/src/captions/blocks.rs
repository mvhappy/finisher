//! Words → caption blocks by a style's splitting rule (spec §5.5: the rule is part of the
//! style).

use crate::error::CoreError;
use crate::model::{CaptionStyle, Word};
use crate::time::Rational;

/// One block to insert: `[start, end)` in the words' own time base, and its words with times
/// relative to `start` (as a caption item holds them, Decision 13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDraft {
    pub start: Rational,
    pub end: Rational,
    pub words: Vec<Word>,
}

/// The characters of a word, as a line counts them (Unicode scalar values).
fn chars(w: &Word) -> u32 {
    u32::try_from(w.text.chars().count()).unwrap_or(u32::MAX)
}

/// Splits `words` (ordered, not overlapping) into blocks, greedily and in order. A word starts a
/// new block when:
/// - the pause before it is at least `style.gap` (a gap of 0 never splits);
/// - the block would span more than `style.maxDuration` (a single word may);
/// - it fits on no line: lines hold at most `maxChars` characters, words joined by one space,
///   and a block at most `maxLines` lines. A word longer than a line gets a line of its own
///   (measurement reports the overflow).
///
/// A block ends at its last word's end. Exact rational arithmetic; overflow while deciding a
/// split ends the block where it stands, but a block whose times cannot be made relative to its
/// start is `TIME_OVERFLOW` (never silently dropped: the caller's import is atomic).
pub fn blocks(words: &[Word], style: &CaptionStyle) -> Result<Vec<BlockDraft>, CoreError> {
    let mut out: Vec<Vec<&Word>> = Vec::new();
    let (mut lines, mut line) = (0u32, 0u32);
    let mut prev: Option<&Word> = None;
    for w in words {
        let n = chars(w);
        let fresh = match (prev, out.last()) {
            (Some(p), Some(block)) => {
                let pause = w.start.checked_sub(p.end).unwrap_or_default();
                let by_gap = style.gap.is_positive() && pause >= style.gap;
                let by_duration = style.max_duration.is_some_and(|max| {
                    w.end
                        .checked_sub(block[0].start)
                        .map_or(true, |span| span > max)
                });
                let fits_line = line.saturating_add(1).saturating_add(n) <= style.max_chars;
                let by_lines = !fits_line && lines >= style.max_lines;
                by_gap || by_duration || by_lines
            }
            _ => true,
        };
        if fresh {
            out.push(vec![w]);
            (lines, line) = (1, n);
        } else {
            let fits_line = line.saturating_add(1).saturating_add(n) <= style.max_chars;
            if fits_line {
                line = line.saturating_add(1).saturating_add(n);
            } else {
                lines += 1;
                line = n;
            }
            if let Some(block) = out.last_mut() {
                block.push(w);
            }
        }
        prev = Some(w);
    }
    out.into_iter()
        .map(|block| {
            let (Some(first), Some(last)) = (block.first(), block.last()) else {
                return Ok(None);
            };
            let (start, end) = (first.start, last.end);
            let words = block
                .into_iter()
                .map(|w| {
                    Ok(Word {
                        text: w.text.clone(),
                        start: w.start.checked_sub(start)?,
                        end: w.end.checked_sub(start)?,
                    })
                })
                .collect::<Result<Vec<Word>, CoreError>>()?;
            Ok(Some(BlockDraft { start, end, words }))
        })
        .filter_map(Result::transpose)
        .collect()
}
