//! The `compressor` effect (spec §8.4, plan 6C Decision 13): `detector` (`rms|peak`, not
//! animatable), `threshold` (−60…0 dB), `ratio` (1–20), `knee` (0–24 dB), `attack` (0.1–200
//! ms), `release` (5–2,000 ms) and `makeup` (0–24 dB).
//!
//! Per sample: the linked [`Detector`] reads the level, [`static_gain`] gives the target gain
//! in dB, the [`Smoother`] follows it with the attack coefficient while the reduction grows
//! (`target < g`) and the release coefficient otherwise, and every channel is multiplied by
//! `10^((g + makeup)/20)`. The numbers are read, clamped to the registry ranges and turned into
//! coefficients on each block start ([`super::Effect::update`]); the detector and the gain
//! carry across blocks and start at rest (empty window, 0 dB) at the item's first sample.

use fin_core::CoreError;
use fin_core::eval::AudioEffectPlan;

use super::Effect;
use super::detector::{Detector, Measure, Smoother, amp, coeff, static_gain};
use super::params::{ParamCurve, ParamsAt};

/// The block's coefficients.
#[derive(Debug, Clone, Copy, Default)]
struct Coeffs {
    threshold: f32,
    /// `1/ratio − 1`.
    slope: f32,
    knee: f32,
    attack: f32,
    release: f32,
    makeup: f32,
}

/// The compressor's running state.
pub struct Compressor {
    /// threshold, ratio, knee, attack, release, makeup.
    curves: [ParamCurve; 6],
    /// The clamped numbers `c` was computed from.
    designed: Option<[f64; 6]>,
    c: Coeffs,
    det: Detector,
    gain: Smoother,
}

impl Compressor {
    /// The compressor of `plan` over the item's samples `[start, end)`.
    pub fn new(plan: &AudioEffectPlan, start: i64, end: i64) -> Result<Compressor, CoreError> {
        let p = ParamsAt::new(plan, start, end)?;
        Ok(Compressor {
            curves: [
                p.curve("threshold", -18.0),
                p.curve("ratio", 4.0),
                p.curve("knee", 6.0),
                p.curve("attack", 10.0),
                p.curve("release", 100.0),
                p.curve("makeup", 0.0),
            ],
            designed: None,
            c: Coeffs::default(),
            det: Detector::new(Measure::parse(p.text("detector"))),
            gain: Smoother { g: 0.0 },
        })
    }
}

impl Effect for Compressor {
    fn update(&mut self, n: i64) {
        let [t, r, k, a, rl, m] = &self.curves;
        let p = [
            t.at(n).clamp(-60.0, 0.0),
            r.at(n).clamp(1.0, 20.0),
            k.at(n).clamp(0.0, 24.0),
            a.at(n).clamp(0.1, 200.0),
            rl.at(n).clamp(5.0, 2_000.0),
            m.at(n).clamp(0.0, 24.0),
        ];
        if self.designed == Some(p) {
            return;
        }
        self.c = Coeffs {
            threshold: p[0] as f32,
            slope: (1.0 / p[1] - 1.0) as f32,
            knee: p[2] as f32,
            attack: coeff(p[3]),
            release: coeff(p[4]),
            makeup: p[5] as f32,
        };
        self.designed = Some(p);
    }

    fn run(&mut self, ch: &mut [&mut [f32]]) {
        let c = self.c;
        let len = ch.first().map_or(0, |x| x.len());
        for i in 0..len {
            let (f, k) = Detector::frame(ch, i);
            let level = self.det.level_db(&f[..k]);
            let target = static_gain(level, c.threshold, c.slope, c.knee);
            let a = if target < self.gain.g {
                c.attack
            } else {
                c.release
            };
            let g = amp(self.gain.step(target, a) + c.makeup);
            for x in ch.iter_mut() {
                x[i] *= g;
            }
        }
    }
}
