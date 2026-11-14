//! The BS.1770 / EBU R 128 meter (plan 6C Decision 18): `ebur128::EbuR128` in modes
//! `I | LRA | TRUE_PEAK | SAMPLE_PEAK`. Its numbers are reported, never hashed (spec §8.6), so
//! this file is outside the float gate; the normalisation gain derived from them is quantised
//! to 0.01 dB before it is applied.
//!
//! **True peak at the end of the stream.** `ebur128`'s 4× interpolator delays its output by
//! a few frames, so a crest in the last frames would never be read. The meter keeps the true
//! peak in its own `TRUE_PEAK` instance (the loudness instance runs `I | LRA | SAMPLE_PEAK`,
//! untouched) and the last `TAIL` frames; `measured()` feeds those frames, the first half
//! faded in with a raised cosine when they start mid-stream (so the fresh interpolator's onset
//! cannot ring above the signal; a stream no longer than `TAIL` is fed unfaded, since its true
//! onset is the zero history a fresh instance assumes), then `TAIL` zeros, to a throwaway `TRUE_PEAK` instance, and reports the larger of
//! the two. The streaming state is never flushed, so readings can be taken at any time and the
//! stream continues undisturbed.

use ebur128::{EbuR128, Mode};

use crate::{RATE, Stereo};

/// What the meter read. Levels that are minus infinity (silence, or nothing above the
/// −70 LUFS absolute gate) are `None`, never NaN.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Measured {
    /// Integrated loudness, LUFS.
    pub integrated: Option<f64>,
    /// Loudness range, LU (0 when there is nothing to measure).
    pub range: f64,
    /// The highest true peak over the channels, dBTP.
    pub true_peak: Option<f64>,
    /// The highest sample peak over the channels, dBFS.
    pub sample_peak: Option<f64>,
    /// The highest short-term (3 s) loudness, LUFS, read every 100 ms.
    pub short_term_max: Option<f64>,
}

/// The short-term loudness is read at this hop (100 ms at 48 kHz).
const HOP: usize = RATE as usize / 10;

/// Frames kept for the end-of-stream true peak (the interpolator's reach is 12 frames).
const TAIL: usize = 64;

/// A streaming loudness meter over 48 kHz audio.
pub struct Meter {
    ebu: EbuR128,
    tp: EbuR128,
    /// The last `TAIL` (or fewer) frames, per channel.
    tail: [Vec<f32>; 2],
    /// Frames pushed in all.
    frames: u64,
    channels: u32,
    /// Frames since the last short-term reading.
    since_hop: usize,
    short_term_max: f64,
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

fn db(linear: f64) -> Option<f64> {
    (linear > 0.0).then(|| 20.0 * linear.log10())
}

impl Meter {
    /// A meter for mono (`1`: the left channel of each block) or stereo (`2`, any other value).
    pub fn new(channels: u16) -> Meter {
        let channels = if channels == 1 { 1 } else { 2 };
        let ebu = EbuR128::new(channels, RATE, Mode::I | Mode::LRA | Mode::SAMPLE_PEAK)
            .expect("one or two channels at 48 kHz are valid ebur128 parameters");
        Meter {
            ebu,
            tp: true_peak_meter(channels),
            tail: [Vec::new(), Vec::new()],
            frames: 0,
            channels,
            since_hop: 0,
            short_term_max: f64::NEG_INFINITY,
        }
    }

    /// Feeds a block of 48 kHz frames.
    pub fn push(&mut self, block: &Stereo) {
        let mut at = 0;
        while at < block.len() {
            let n = (HOP - self.since_hop).min(block.len() - at);
            let (l, r) = (&block.l[at..at + n], &block.r[at..at + n]);
            let planes: &[&[f32]] = if self.channels == 1 { &[l] } else { &[l, r] };
            for e in [&mut self.ebu, &mut self.tp] {
                e.add_frames_planar_f32(planes)
                    .expect("the planes have the meter's channel count and equal lengths");
            }
            for (t, x) in self.tail.iter_mut().zip([l, r]) {
                t.extend_from_slice(x);
                let drop = t.len().saturating_sub(TAIL);
                t.drain(..drop);
            }
            at += n;
            self.frames += n as u64;
            self.since_hop += n;
            if self.since_hop == HOP {
                self.since_hop = 0;
                if let Ok(st) = self.ebu.loudness_shortterm() {
                    self.short_term_max = self.short_term_max.max(st);
                }
            }
        }
    }

    /// The true peak of the last frames with the interpolator flushed (linear).
    fn flushed_true_peak(&self) -> f64 {
        if self.tail[0].is_empty() {
            return 0.0;
        }
        // The retained frames are the whole stream: no fade.
        let half = if self.frames > self.tail[0].len() as u64 {
            TAIL / 2
        } else {
            0
        };
        let feed = |x: &[f32]| -> Vec<f32> {
            let mut v: Vec<f32> = x
                .iter()
                .enumerate()
                .map(|(k, &s)| {
                    let w = if k < half {
                        0.5 * (1.0 - (core::f64::consts::PI * k as f64 / half as f64).cos())
                    } else {
                        1.0
                    };
                    (f64::from(s) * w) as f32
                })
                .collect();
            v.resize(v.len() + TAIL, 0.0);
            v
        };
        let (l, r) = (feed(&self.tail[0]), feed(&self.tail[1]));
        let planes: &[&[f32]] = if self.channels == 1 { &[&l] } else { &[&l, &r] };
        let mut e = true_peak_meter(self.channels);
        e.add_frames_planar_f32(planes)
            .expect("the planes have the meter's channel count and equal lengths");
        (0..self.channels)
            .filter_map(|c| e.true_peak(c).ok())
            .fold(0.0f64, f64::max)
    }

    /// The short-term (3 s) loudness now, LUFS (`None` for silence): `fin listen` reads it after
    /// each 100 ms it pushes (plan 6C Task 21).
    pub fn short_term(&self) -> Option<f64> {
        self.ebu.loudness_shortterm().ok().and_then(finite)
    }

    /// The measurements so far.
    pub fn measured(&self) -> Measured {
        let peak = |f: &dyn Fn(u32) -> Result<f64, ebur128::Error>| {
            (0..self.channels)
                .filter_map(|c| f(c).ok())
                .fold(0.0f64, f64::max)
        };
        Measured {
            integrated: self.ebu.loudness_global().ok().and_then(finite),
            range: self
                .ebu
                .loudness_range()
                .ok()
                .filter(|v| v.is_finite())
                .unwrap_or(0.0),
            true_peak: db(peak(&|c| self.tp.true_peak(c)).max(self.flushed_true_peak())),
            sample_peak: db(peak(&|c| self.ebu.sample_peak(c))),
            short_term_max: finite(self.short_term_max),
        }
    }
}

fn true_peak_meter(channels: u32) -> EbuR128 {
    EbuR128::new(channels, RATE, Mode::TRUE_PEAK)
        .expect("one or two channels at 48 kHz are valid ebur128 parameters")
}
