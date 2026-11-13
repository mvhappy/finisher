//! The `deEsser` effect (spec §8.4, plan 6C Decision 13): `frequency` (2,000–12,000 Hz), `q`
//! (0.5–8), `threshold` (−60…0 dB), `ratio` (1–20) and `range` (0–24 dB, the most it reduces).
//!
//! A compressor on one band. Each channel is split by an RBJ band-pass ([`BandKind::BandPass`],
//! 0 dB peak gain at `frequency`, `q`; one [`Biquad`] per channel) into `band` and `residual =
//! x − band`. The linked RMS [`Detector`] (10 ms window) reads the band only; the hard-knee
//! [`static_gain`], capped at `−range`, is smoothed in dB ([`ATTACK_MS`], [`RELEASE_MS`]) and
//! applied to the band only: `y = residual + G·band`, computed as `x − (1 − G)·band` (the same
//! sum; with no reduction `G = 1` and the output is the input bit for bit). The numbers are
//! read, clamped and turned into coefficients on each block start; the filter states, the
//! detector and the gain start at rest at the item's first sample.

use fin_core::CoreError;
use fin_core::eval::AudioEffectPlan;

use super::Effect;
use super::biquad::{BandKind, Biquad, Coeffs as Bq, design};
use super::detector::{Detector, Measure, Smoother, amp, coeff, static_gain};
use super::params::{ParamCurve, ParamsAt};
use crate::RATE;

/// The de-esser's attack, fixed (the registry has no number for it).
pub const ATTACK_MS: f64 = 1.0;
/// The de-esser's release, fixed.
pub const RELEASE_MS: f64 = 50.0;

/// The de-esser's running state.
pub struct DeEsser {
    /// frequency, q, threshold, ratio, range.
    curves: [ParamCurve; 5],
    designed: Option<[f64; 5]>,
    band: Bq,
    threshold: f32,
    /// `1/ratio − 1`.
    slope: f32,
    /// `−range` dB.
    floor: f32,
    attack: f32,
    release: f32,
    split: [Biquad; 2],
    det: Detector,
    gain: Smoother,
}

impl DeEsser {
    /// The de-esser of `plan` over the item's samples `[start, end)`.
    pub fn new(plan: &AudioEffectPlan, start: i64, end: i64) -> Result<DeEsser, CoreError> {
        let p = ParamsAt::new(plan, start, end)?;
        Ok(DeEsser {
            curves: [
                p.curve("frequency", 6_500.0),
                p.curve("q", 2.0),
                p.curve("threshold", -30.0),
                p.curve("ratio", 6.0),
                p.curve("range", 12.0),
            ],
            designed: None,
            band: Bq::IDENTITY,
            threshold: 0.0,
            slope: 0.0,
            floor: 0.0,
            attack: coeff(ATTACK_MS),
            release: coeff(RELEASE_MS),
            split: [Biquad::default(); 2],
            det: Detector::new(Measure::Rms),
            gain: Smoother { g: 0.0 },
        })
    }
}

impl Effect for DeEsser {
    fn update(&mut self, n: i64) {
        let [f, q, t, r, rg] = &self.curves;
        let p = [
            f.at(n).clamp(2_000.0, 12_000.0),
            q.at(n).clamp(0.5, 8.0),
            t.at(n).clamp(-60.0, 0.0),
            r.at(n).clamp(1.0, 20.0),
            rg.at(n).clamp(0.0, 24.0),
        ];
        let d = self.designed;
        if d.is_none_or(|d| d[..2] != p[..2]) {
            self.band = design(BandKind::BandPass, p[0], 0.0, p[1], f64::from(RATE));
        }
        if d != Some(p) {
            self.threshold = p[2] as f32;
            self.slope = (1.0 / p[3] - 1.0) as f32;
            self.floor = -p[4] as f32;
            self.designed = Some(p);
        }
    }

    fn run(&mut self, ch: &mut [&mut [f32]]) {
        let len = ch.first().map_or(0, |x| x.len());
        let k = ch.len().min(2);
        for i in 0..len {
            let mut band = [0.0f32; 2];
            for ((b, st), x) in band.iter_mut().zip(&mut self.split).zip(ch.iter()) {
                *b = st.tick(&self.band, x[i]);
            }
            let level = self.det.level_db(&band[..k]);
            let target = static_gain(level, self.threshold, self.slope, 0.0).max(self.floor);
            let a = if target < self.gain.g {
                self.attack
            } else {
                self.release
            };
            let cut = 1.0 - amp(self.gain.step(target, a));
            for (x, b) in ch.iter_mut().zip(band) {
                x[i] -= cut * b;
            }
        }
    }
}
