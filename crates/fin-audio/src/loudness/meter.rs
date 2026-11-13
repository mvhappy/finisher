//! The BS.1770 / EBU R 128 meter (plan 6C Decision 18): `ebur128::EbuR128` in modes
//! `I | LRA | TRUE_PEAK | SAMPLE_PEAK`. Its numbers are reported, never hashed (spec §8.6), so
//! this file is outside the float gate; the normalisation gain derived from them is quantised
//! to 0.01 dB before it is applied.

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

/// A streaming loudness meter over 48 kHz audio.
pub struct Meter {
    ebu: EbuR128,
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
        let mode = Mode::I | Mode::LRA | Mode::TRUE_PEAK | Mode::SAMPLE_PEAK;
        let ebu = EbuR128::new(channels, RATE, mode)
            .expect("one or two channels at 48 kHz are valid ebur128 parameters");
        Meter {
            ebu,
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
            self.ebu
                .add_frames_planar_f32(planes)
                .expect("the planes have the meter's channel count and equal lengths");
            at += n;
            self.since_hop += n;
            if self.since_hop == HOP {
                self.since_hop = 0;
                if let Ok(st) = self.ebu.loudness_shortterm() {
                    self.short_term_max = self.short_term_max.max(st);
                }
            }
        }
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
            true_peak: db(peak(&|c| self.ebu.true_peak(c))),
            sample_peak: db(peak(&|c| self.ebu.sample_peak(c))),
            short_term_max: finite(self.short_term_max),
        }
    }
}
