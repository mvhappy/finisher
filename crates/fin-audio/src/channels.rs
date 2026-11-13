//! Canonical mixer sources (6C plan Decision 3): a decoded interleaved frame becomes mono or
//! planar stereo, mapped by channel position and downmixed with BS.775 coefficients.

use fin_core::CoreError;
use fin_media::audio_layout::{ChannelLayout, ChannelPos};
use serde_json::json;

use crate::Stereo;

/// What the mixer reads: one channel, or two.
#[derive(Debug, Clone, PartialEq)]
pub enum Canonical {
    Mono(Vec<f32>),
    Stereo(Stereo),
}

/// −3 dB, the BS.775 coefficient for the centre and the surrounds.
const M3DB: f32 = 0.70710677;

/// `CODEC_UNAVAILABLE {reason: channels}`.
fn unavailable(message: String) -> CoreError {
    CoreError::new("CODEC_UNAVAILABLE", message)
        .with_details(json!({ "reason": "channels" }))
        .with_fix("convert the audio to mono, stereo or a standard 5.1 or 7.1 layout")
}

/// The `(to L, to R)` gains of a position; `None` for LFE (dropped).
fn gains(pos: ChannelPos) -> Option<(f32, f32)> {
    use ChannelPos::*;
    Some(match pos {
        L => (1.0, 0.0),
        R => (0.0, 1.0),
        C => (M3DB, M3DB),
        Ls | Lb => (M3DB, 0.0),
        Rs | Rb => (0.0, M3DB),
        Cb => (0.5, 0.5),
        Lw => (M3DB, 0.0),
        Rw => (0.0, M3DB),
        Lfe | Unknown => return None,
    })
}

/// Converts interleaved frames in `layout` to the canonical form: 1 channel stays mono (bit
/// exact), 2 channels are stereo (bit exact, also when L = R), 3 to 8 are mapped by position
/// and folded to stereo (C, Ls, Rs, Lb, Rb at −3 dB, Cb at −6 dB to both, LFE dropped; in
/// channel order). An `Unknown` position among 3 or more channels, more than 8 channels, no
/// channels or a ragged buffer is `CODEC_UNAVAILABLE {reason: channels}`.
pub fn canonical(interleaved: &[f32], layout: &ChannelLayout) -> Result<Canonical, CoreError> {
    let n = layout.channels();
    if n == 0 || n > 8 {
        return Err(unavailable(format!(
            "{n} channels are not supported (1 to 8)"
        )));
    }
    if !interleaved.len().is_multiple_of(n) {
        return Err(unavailable(format!(
            "{} samples are not a whole number of {n}-channel frames",
            interleaved.len()
        )));
    }
    match n {
        1 => return Ok(Canonical::Mono(interleaved.to_vec())),
        2 => {
            let mut s = Stereo::default();
            for f in interleaved.as_chunks::<2>().0 {
                s.push(f[0], f[1]);
            }
            return Ok(Canonical::Stereo(s));
        }
        _ => {}
    }
    if layout.0.contains(&ChannelPos::Unknown) {
        return Err(unavailable(
            "a channel has no known position in the room".to_string(),
        ));
    }
    let table: Vec<Option<(f32, f32)>> = layout.0.iter().map(|p| gains(*p)).collect();
    let mut s = Stereo::default();
    for f in interleaved.chunks_exact(n) {
        let (mut l, mut r) = (0.0f32, 0.0f32);
        for (x, g) in f.iter().zip(&table) {
            if let Some((gl, gr)) = g {
                if *gl != 0.0 {
                    l += x * gl;
                }
                if *gr != 0.0 {
                    r += x * gr;
                }
            }
        }
        s.push(l, r);
    }
    Ok(Canonical::Stereo(s))
}

/// The analysis feed's fold: mono as is, stereo as `0.5·(L + R)`. `out` is cleared first.
pub fn fold_mono(c: &Canonical, out: &mut Vec<f32>) {
    out.clear();
    match c {
        Canonical::Mono(m) => out.extend_from_slice(m),
        Canonical::Stereo(s) => out.extend(s.l.iter().zip(&s.r).map(|(l, r)| 0.5 * (l + r))),
    }
}
