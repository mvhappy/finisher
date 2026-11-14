//! Test-only container writers (feature `testing`, never shipped): fixtures are generated at
//! test time from `fin-audio::testing` signals, so no media file is committed (6C plan Global
//! Constraints). Each writer takes interleaved `f32` frames in `[-1, 1]`.

pub mod aiff;
pub mod caf;
pub mod flac;
pub mod mp3;
pub mod ogg;
pub mod wav;

/// `x` as a `bits`-bit integer sample: `round(x × 2^(bits−1))`, clamped to the range.
pub fn to_int(x: f32, bits: u32) -> i32 {
    let scale = (1i64 << (bits - 1)) as f64;
    let q = (x as f64 * scale).round();
    q.clamp(-scale, scale - 1.0) as i32
}

/// What an integer decoder gives back for `pcm` written at `bits`: `to_int(x) / 2^(bits−1)`,
/// which is exact in `f32` up to 24 bits.
pub fn quantised(pcm: &[f32], bits: u32) -> Vec<f32> {
    let scale = (1i64 << (bits - 1)) as f32;
    pcm.iter()
        .map(|&x| to_int(x, bits) as f32 / scale)
        .collect()
}

/// Little-endian bytes of `x` as a `bits`-bit integer (16, 24 or 32).
pub(crate) fn int_le(x: f32, bits: u32, out: &mut Vec<u8>) {
    let v = to_int(x, bits).to_le_bytes();
    out.extend_from_slice(&v[..bits as usize / 8]);
}

/// Big-endian bytes of `x` as a `bits`-bit integer (16, 24 or 32).
pub(crate) fn int_be(x: f32, bits: u32, out: &mut Vec<u8>) {
    let v = to_int(x, bits).to_be_bytes();
    out.extend_from_slice(&v[4 - bits as usize / 8..]);
}
