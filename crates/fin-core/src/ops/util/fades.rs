//! An item's audio fades restricted to one part of a split (spec §5.13, review #55), so both
//! parts keep the original gain envelope.
//!
//! A fade is its length and its offset ([`AudioParams`]): its window lies at a fixed place on
//! the timeline relative to the item's original edge. A part whose edge lies `shift` inward of
//! that edge (the left part's end, the right part's start) keeps the fade with `offset + shift`
//! when the window still reaches into the part (`fade > offset + shift`), else drops it (length
//! and offset 0). The part on the original edge's side has `shift` 0 and keeps a fade that
//! reaches the item as it is. Each fade is restricted on its own, so overlapping fades still
//! multiply as before.

use crate::error::CoreError;
use crate::model::AudioParams;
use crate::time::Rational;

/// A fade's registry paths: `[length, offset]`.
pub(crate) const FADE_IN: [&str; 2] = ["audio.fadeIn", "audio.fadeInOffset"];
pub(crate) const FADE_OUT: [&str; 2] = ["audio.fadeOut", "audio.fadeOutOffset"];

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

/// `audio` restricted to a part whose start lies `from_start` after the original start and
/// whose end lies `from_end` before the original end. Returns the new params and which fades
/// changed (`[in, out]`); params left all default become `None`.
pub(crate) fn restrict_part(
    audio: &AudioParams,
    from_start: Rational,
    from_end: Rational,
) -> Result<(Option<AudioParams>, [bool; 2]), CoreError> {
    let mut a = audio.clone();
    (a.fade_in, a.fade_in_offset) = restrict(a.fade_in, a.fade_in_offset, from_start)?;
    (a.fade_out, a.fade_out_offset) = restrict(a.fade_out, a.fade_out_offset, from_end)?;
    let changed = [
        (a.fade_in, a.fade_in_offset) != (audio.fade_in, audio.fade_in_offset),
        (a.fade_out, a.fade_out_offset) != (audio.fade_out, audio.fade_out_offset),
    ];
    // Bit for bit, as `params::set` drops them (`-0.0` is kept).
    let default = a.gain.to_bits() == 0 && a.pan.to_bits() == 0 && a == AudioParams::default();
    Ok((if default { None } else { Some(a) }, changed))
}
