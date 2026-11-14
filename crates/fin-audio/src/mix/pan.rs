//! The two pan laws (spec §8.1–8.2; plan 6C Decisions 3 and 16).
//!
//! - **Mono, constant power:** `L = x·sqrt((1 − p)/2)`, `R = x·sqrt((1 + p)/2)`. This *is* the
//!   mono source's −3 dB upmix (§8.1): at the centre each side is `x × 0.70710677`, and nothing
//!   attenuates it again.
//! - **Stereo, balance:** `p < 0` scales R by `1 + p`, `p > 0` scales L by `1 − p`, the other
//!   side stays at unity; dual mono (L = R) is stereo.

use crate::mix::SourceLayout;

/// The `(L, R)` amplitudes of gain `g` at pan `p` (already clamped to `[−1, 1]`): the pan
/// factor first, then times `g`.
pub(crate) fn sides(layout: SourceLayout, g: f32, p: f32) -> (f32, f32) {
    let (l, r) = match layout {
        SourceLayout::Mono => (libm::sqrtf((1.0 - p) * 0.5), libm::sqrtf((1.0 + p) * 0.5)),
        SourceLayout::Stereo if p < 0.0 => (1.0, 1.0 + p),
        SourceLayout::Stereo if p > 0.0 => (1.0 - p, 1.0),
        SourceLayout::Stereo => (1.0, 1.0),
    };
    (g * l, g * r)
}
