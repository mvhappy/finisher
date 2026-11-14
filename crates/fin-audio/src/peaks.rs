//! Waveform peaks, `wave` v1 (plan 6C Decision 19). A0: hashed, bit-identical everywhere.
//!
//! Per channel of the 48 kHz stereo signal, `(min, max)` pairs as `i16` (`round(x × 32767)`,
//! half away from zero, clamped to the `i16` range) at 256, 1,024, 4,096, 16,384 and 65,536
//! samples per peak; the last peak of a level covers the remaining frames. Rounding is
//! monotonic, so each level is derived exactly from the one before it.
//!
//! `peaks.bin`: a 32-byte header — `FINWAVE1`, the level count (`u32`, 5), the channel count
//! (`u32`, 2), the frame count (`u64`) and 8 zero bytes, all little-endian — then the levels in
//! order; a level holds `ceil(frames / samples_per_peak)` peaks, each peak every channel's `min`
//! then `max` as `i16` LE (`min ≤ max`).

use fin_core::CoreError;
use serde_json::json;

use crate::Stereo;

/// Samples per peak of each level, finest first.
pub const LEVELS: [u32; 5] = [256, 1_024, 4_096, 16_384, 65_536];

const MAGIC: &[u8; 8] = b"FINWAVE1";
const HEADER: usize = 32;
const CHANNELS: usize = 2;

/// One level: `channels[c][i]` is channel `c`'s `(min, max)` over peak `i`.
#[derive(Debug, Clone, PartialEq)]
pub struct Level {
    pub samples_per_peak: u32,
    pub channels: Vec<Vec<(i16, i16)>>,
}

/// The waveform peaks of a signal.
#[derive(Debug, Clone, PartialEq)]
pub struct Peaks {
    pub frames: u64,
    pub levels: Vec<Level>,
}

/// `round(x × 32767)` clamped to `i16` (NaN reads as 0).
fn quantise(x: f32) -> i16 {
    libm::roundf(x * 32_767.0).clamp(-32_768.0, 32_767.0) as i16
}

fn finest(x: &[f32], spp: usize) -> Vec<(i16, i16)> {
    x.chunks(spp)
        .map(|c| {
            let (mut lo, mut hi) = (i16::MAX, i16::MIN);
            for &v in c {
                let q = quantise(v);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            (lo, hi)
        })
        .collect()
}

fn coarser(pairs: &[(i16, i16)], factor: usize) -> Vec<(i16, i16)> {
    pairs
        .chunks(factor)
        .map(|c| {
            c.iter().fold((i16::MAX, i16::MIN), |(lo, hi), &(a, b)| {
                (lo.min(a), hi.max(b))
            })
        })
        .collect()
}

/// The peaks of a 48 kHz stereo signal.
pub fn peaks(pcm: &Stereo) -> Peaks {
    let mut levels: Vec<Level> = Vec::with_capacity(LEVELS.len());
    for (k, &spp) in LEVELS.iter().enumerate() {
        let channels = match levels.last() {
            None => [&pcm.l, &pcm.r].map(|x| finest(x, spp as usize)).to_vec(),
            Some(prev) => {
                let factor = (spp / LEVELS[k - 1]) as usize;
                prev.channels.iter().map(|p| coarser(p, factor)).collect()
            }
        };
        levels.push(Level {
            samples_per_peak: spp,
            channels,
        });
    }
    Peaks {
        frames: pcm.len() as u64,
        levels,
    }
}

fn corrupt(message: &str, at: usize) -> CoreError {
    CoreError::new("MEDIA_CORRUPT", format!("peaks.bin: {message}"))
        .with_details(json!({ "asset": "peaks.bin", "at": at }))
}

/// Peaks per level for `frames` frames, or `None` when the count overflows `usize`.
fn counts(frames: u64) -> Option<Vec<usize>> {
    LEVELS
        .iter()
        .map(|&s| usize::try_from(frames.div_ceil(u64::from(s))).ok())
        .collect()
}

impl Peaks {
    /// The `peaks.bin` bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let pairs: usize = self
            .levels
            .iter()
            .map(|l| l.channels.len() * l.channels.first().map_or(0, Vec::len))
            .sum();
        let mut b = Vec::with_capacity(HEADER + pairs * 4);
        b.extend_from_slice(MAGIC);
        b.extend_from_slice(&(self.levels.len() as u32).to_le_bytes());
        let channels = self.levels.first().map_or(CHANNELS, |l| l.channels.len());
        b.extend_from_slice(&(channels as u32).to_le_bytes());
        b.extend_from_slice(&self.frames.to_le_bytes());
        b.extend_from_slice(&[0; 8]);
        for level in &self.levels {
            let count = level.channels.first().map_or(0, Vec::len);
            for i in 0..count {
                for ch in &level.channels {
                    let (lo, hi) = ch[i];
                    b.extend_from_slice(&lo.to_le_bytes());
                    b.extend_from_slice(&hi.to_le_bytes());
                }
            }
        }
        b
    }

    /// Reads `peaks.bin`; anything but the exact layout above is `MEDIA_CORRUPT`.
    pub fn parse(b: &[u8]) -> Result<Peaks, CoreError> {
        if b.len() < HEADER {
            return Err(corrupt("shorter than its header", b.len()));
        }
        if &b[..8] != MAGIC {
            return Err(corrupt("not a FINWAVE1 file", 0));
        }
        let u32_at = |o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
        if u32_at(8) as usize != LEVELS.len() {
            return Err(corrupt("the level count is not 5", 8));
        }
        if u32_at(12) as usize != CHANNELS {
            return Err(corrupt("the channel count is not 2", 12));
        }
        let mut frame_bytes = [0u8; 8];
        frame_bytes.copy_from_slice(&b[16..24]);
        let frames = u64::from_le_bytes(frame_bytes);
        if b[24..HEADER].iter().any(|&v| v != 0) {
            return Err(corrupt("the reserved header bytes are not zero", 24));
        }
        let counts = counts(frames).ok_or_else(|| corrupt("the frame count is too large", 16))?;
        let body = counts.iter().try_fold(0usize, |acc, &c| {
            acc.checked_add(c.checked_mul(CHANNELS * 4)?)
        });
        if body.and_then(|n| n.checked_add(HEADER)) != Some(b.len()) {
            return Err(corrupt(
                "the length does not match the frame count",
                b.len(),
            ));
        }
        let mut at = HEADER;
        let i16_at = |o: usize| i16::from_le_bytes([b[o], b[o + 1]]);
        let mut levels = Vec::with_capacity(LEVELS.len());
        for (&spp, &count) in LEVELS.iter().zip(&counts) {
            let mut channels: Vec<Vec<(i16, i16)>> =
                (0..CHANNELS).map(|_| Vec::with_capacity(count)).collect();
            for _ in 0..count {
                for ch in &mut channels {
                    let (lo, hi) = (i16_at(at), i16_at(at + 2));
                    if lo > hi {
                        return Err(corrupt("a peak's minimum is above its maximum", at));
                    }
                    ch.push((lo, hi));
                    at += 4;
                }
            }
            levels.push(Level {
                samples_per_peak: spp,
                channels,
            });
        }
        Ok(Peaks { frames, levels })
    }
}
