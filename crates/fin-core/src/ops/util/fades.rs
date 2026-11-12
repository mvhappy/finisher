//! An item's audio fades shared out between the two parts of a split (spec §5.13, review #55),
//! so both parts keep the original gain envelope.
//!
//! A fade is its length and its offset ([`AudioParams`]): its window lies at a fixed place on
//! the timeline relative to the item's original edge. A part whose edge lies `shift` inward of
//! that edge (the left part's end, the right part's start) keeps the fade with `offset + shift`
//! when the window still reaches into the part (`fade > offset + shift`), else drops it (length
//! and offset 0). The part on the original edge's side has `shift` 0 and keeps a fade that
//! reaches the item as it is. Each fade is restricted on its own, so overlapping fades still
//! multiply as before.
//!
//! A fade a transition suppresses (eval drops a fade on a side with a transition) is inaudible
//! in the original item, so the part that loses the transition's edge drops its share of it:
//! the right part's fade-in under an incoming transition, the left part's fade-out under an
//! outgoing one (whose transition the prune pass then removes; the right part keeps the
//! fade-out, as before this rule).

use crate::error::CoreError;
use crate::model::AudioParams;
use crate::time::Rational;

/// A fade's registry paths: `[length, offset]`.
pub(crate) const FADE_IN: [&str; 2] = ["audio.fadeIn", "audio.fadeInOffset"];
pub(crate) const FADE_OUT: [&str; 2] = ["audio.fadeOut", "audio.fadeOutOffset"];

/// Whether `path` is a fade's length or offset.
pub(crate) fn is_fade_path(path: &str) -> bool {
    FADE_IN.contains(&path) || FADE_OUT.contains(&path)
}

/// The audio params of both parts of a split, and per fade (`[in, out]`) whether the left
/// part's changed (its inverse puts them back) and whether the split divided it (its window
/// reaches into both parts).
pub(crate) struct Split {
    pub(crate) left: Option<AudioParams>,
    pub(crate) right: Option<AudioParams>,
    pub(crate) left_changed: [bool; 2],
    pub(crate) right_changed: bool,
    pub(crate) divided: [bool; 2],
}

/// `(fade, offset)` restricted to a part whose edge lies `shift` inward of the original one.
fn restrict(
    fade: Rational,
    offset: Rational,
    shift: Rational,
) -> Result<(Rational, Rational), CoreError> {
    let offset = offset.checked_add(shift)?;
    Ok(if fade > offset {
        (fade, offset)
    } else {
        (Rational::zero(), Rational::zero())
    })
}

/// Params left all default become `None` (bit for bit, as `params::set` drops them).
fn tidy(a: AudioParams) -> Option<AudioParams> {
    let default = a.gain.to_bits() == 0 && a.pan.to_bits() == 0 && a == AudioParams::default();
    (!default).then_some(a)
}

/// `audio` of an item of `duration` split at `offset`; `suppressed` says which of its fades
/// (`[in, out]`) a transition suppresses.
pub(crate) fn split(
    audio: &AudioParams,
    offset: Rational,
    duration: Rational,
    suppressed: [bool; 2],
) -> Result<Split, CoreError> {
    let zero = Rational::zero();
    let tail = duration.checked_sub(offset)?;
    let mut l = audio.clone();
    (l.fade_out, l.fade_out_offset) = if suppressed[1] {
        (zero, zero)
    } else {
        restrict(l.fade_out, l.fade_out_offset, tail)?
    };
    (l.fade_in, l.fade_in_offset) = restrict(l.fade_in, l.fade_in_offset, zero)?;
    let mut r = audio.clone();
    (r.fade_in, r.fade_in_offset) = if suppressed[0] {
        (zero, zero)
    } else {
        restrict(r.fade_in, r.fade_in_offset, offset)?
    };
    (r.fade_out, r.fade_out_offset) = restrict(r.fade_out, r.fade_out_offset, zero)?;
    let fade_in = |a: &AudioParams| (a.fade_in, a.fade_in_offset);
    let fade_out = |a: &AudioParams| (a.fade_out, a.fade_out_offset);
    let left_changed = [
        fade_in(&l) != fade_in(audio),
        fade_out(&l) != fade_out(audio),
    ];
    let right_changed = r != *audio;
    // The window reaches the part farther from its edge, so it reaches both.
    let divided = [
        audio.fade_in > audio.fade_in_offset.checked_add(offset)?,
        audio.fade_out > audio.fade_out_offset.checked_add(tail)?,
    ];
    Ok(Split {
        left: tidy(l),
        right: tidy(r),
        left_changed,
        right_changed,
        divided,
    })
}
