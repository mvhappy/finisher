//! The `gate` effect (spec §8.4, plan 6C Decision 13): `threshold` (−90…0 dB), `range` (−90…0
//! dB, the gain when closed), `attack` (0.1–50 ms), `hold` (0–500 ms) and `release` (5–2,000
//! ms).
//!
//! Per sample the linked peak [`Detector`] (10 ms window) reads the level. Above `threshold`
//! the gate is open (target 0 dB) and its hold restarts; at or below it the gate stays open
//! for `hold` more samples, then targets `range`. The [`Smoother`] follows the target in dB with
//! the attack coefficient while opening (`target > g`) and the release coefficient otherwise,
//! and every channel is multiplied by `10^(g/20)`. At the item's first sample the gate is open
//! with its hold full (as if a signal had just stopped), so the item's first transient is never
//! cut. The numbers are read, clamped and turned into coefficients on each block start.

use fin_core::CoreError;
use fin_core::eval::AudioEffectPlan;

use super::Effect;
use super::detector::{Detector, Measure, Smoother, amp, coeff};
use super::params::{ParamCurve, ParamsAt};

/// The block's coefficients.
#[derive(Debug, Clone, Copy, Default)]
struct Coeffs {
    threshold: f32,
    range: f32,
    attack: f32,
    release: f32,
    /// `hold` in samples.
    hold: u32,
}

/// The gate's running state.
pub struct Gate {
    /// threshold, range, attack, hold, release.
    curves: [ParamCurve; 5],
    designed: Option<[f64; 5]>,
    c: Coeffs,
    det: Detector,
    gain: Smoother,
    /// Samples the gate stays open for; `None` until the first block (then the full hold).
    hold_left: Option<u32>,
}

impl Gate {
    /// The gate of `plan` over the item's samples `[start, end)`.
    pub fn new(plan: &AudioEffectPlan, start: i64, end: i64) -> Result<Gate, CoreError> {
        let p = ParamsAt::new(plan, start, end)?;
        Ok(Gate {
            curves: [
                p.curve("threshold", -50.0),
                p.curve("range", -80.0),
                p.curve("attack", 1.0),
                p.curve("hold", 50.0),
                p.curve("release", 100.0),
            ],
            designed: None,
            c: Coeffs::default(),
            det: Detector::new(Measure::Peak),
            gain: Smoother { g: 0.0 },
            hold_left: None,
        })
    }
}

impl Effect for Gate {
    fn update(&mut self, n: i64) {
        let [t, rg, a, h, rl] = &self.curves;
        let p = [
            t.at(n).clamp(-90.0, 0.0),
            rg.at(n).clamp(-90.0, 0.0),
            a.at(n).clamp(0.1, 50.0),
            h.at(n).clamp(0.0, 500.0),
            rl.at(n).clamp(5.0, 2_000.0),
        ];
        if self.designed != Some(p) {
            self.c = Coeffs {
                threshold: p[0] as f32,
                range: p[1] as f32,
                attack: coeff(p[2]),
                // At most 24,000 samples: exact in f64, rounded half up.
                hold: (p[3] * 48.0 + 0.5) as u32,
                release: coeff(p[4]),
            };
            self.designed = Some(p);
        }
        self.hold_left.get_or_insert(self.c.hold);
    }

    fn run(&mut self, ch: &mut [&mut [f32]]) {
        let c = self.c;
        let mut left = self.hold_left.unwrap_or(c.hold);
        let len = ch.first().map_or(0, |x| x.len());
        for i in 0..len {
            let (f, k) = Detector::frame(ch, i);
            let target = if self.det.level_db(&f[..k]) > c.threshold {
                left = c.hold;
                0.0
            } else if left > 0 {
                left -= 1;
                0.0
            } else {
                c.range
            };
            let a = if target > self.gain.g {
                c.attack
            } else {
                c.release
            };
            let g = amp(self.gain.step(target, a));
            for x in ch.iter_mut() {
                x[i] *= g;
            }
        }
        self.hold_left = Some(left);
    }
}
