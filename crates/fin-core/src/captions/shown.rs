//! Which of a caption's words are shown (plan 4C Tasks 19–20): the one rule `caption.merge`,
//! `caption.setText`, measurement (the overflow check and eval's layout) and eval's word list
//! share, so what is merged, what is measured and what is drawn agree.
//!
//! Word times are item-relative and may run outside the item after a trim. A word with a
//! length is shown when it overlaps the item's span `(0, duration)`; a zero-length word when
//! it lies inside `[0, duration]` (so a word at the very end, as `caption.setText` may time
//! one, is shown). Shown words are clamped to `[0, duration]`. [`hidden_before_start`] and
//! [`hidden_after_end`] are the two halves of the rule, for `caption.merge`, which drops at
//! its seam what either caption hides there and keeps the hidden words at its outer edges as
//! they are (hidden still, and where a trim of the merged caption expects them).

use crate::model::Word;
use crate::time::Rational;

/// Whether `w` is hidden before its item's start: it ends at or before 0, unless it is a
/// zero-length word at 0.
pub fn hidden_before_start(w: &Word) -> bool {
    let zero = Rational::zero();
    w.end <= zero && !(w.start == zero && w.end == zero)
}

/// Whether `w` is hidden after the end of an item `duration` long: it starts at or after the
/// end, unless it is a zero-length word at the end.
pub fn hidden_after_end(w: &Word, duration: Rational) -> bool {
    w.start >= duration && !(w.start == duration && w.end == duration)
}

/// Whether `w` is shown on an item `duration` long (module docs): hidden at neither edge.
pub fn is_shown(w: &Word, duration: Rational) -> bool {
    !hidden_before_start(w) && !hidden_after_end(w, duration)
}

/// The shown words of `words` on an item `duration` long, in order, clamped to
/// `[0, duration]`.
pub fn shown_words(words: &[Word], duration: Rational) -> Vec<Word> {
    let zero = Rational::zero();
    words
        .iter()
        .filter(|w| is_shown(w, duration))
        .map(|w| Word {
            text: w.text.clone(),
            start: w.start.max(zero),
            end: w.end.min(duration),
        })
        .collect()
}
