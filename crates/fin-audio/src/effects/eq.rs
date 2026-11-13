//! The `eq` effect (spec §8.4, plan 6C Decision 13): eight bands, each `band<n>Type` (`off`,
//! `lowShelf`, `highShelf`, `peak`, `lowPass`, `highPass`; not animatable) with `band<n>Freq`,
//! `band<n>Gain` and `band<n>Q` (animatable).
//!
//! A band that is `off` (or of a type this engine does not know) is not run at all, so an EQ
//! with every band off is a bit-exact passthrough. The others run in band order, each over the
//! whole block, one [`Biquad`] state per channel. On every block start
//! ([`super::Effect::update`]) each band reads its numbers there, clamps them to the registry
//! ranges and redesigns its coefficients when they moved.

use fin_core::CoreError;
use fin_core::eval::AudioEffectPlan;

use super::Effect;
use super::biquad::{BandKind, Biquad, Coeffs, design};
use super::params::{ParamCurve, ParamsAt};
use crate::RATE;

/// The registry's default band frequencies (Decision 13), for a plan that lacks them.
const DEFAULT_FREQ: [f64; 8] = [80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10_000.0];

fn kind_of(ty: &str) -> Option<BandKind> {
    Some(match ty {
        "lowShelf" => BandKind::LowShelf,
        "highShelf" => BandKind::HighShelf,
        "peak" => BandKind::Peak,
        "lowPass" => BandKind::LowPass,
        "highPass" => BandKind::HighPass,
        _ => return None,
    })
}

struct Band {
    kind: BandKind,
    freq: ParamCurve,
    gain: ParamCurve,
    q: ParamCurve,
    /// The clamped `(freq, gain, q)` the coefficients were designed from.
    designed: Option<[f64; 3]>,
    c: Coeffs,
    state: [Biquad; 2],
}

impl Band {
    fn update(&mut self, n: i64) {
        let p = [
            self.freq.at(n).clamp(20.0, 20_000.0),
            self.gain.at(n).clamp(-24.0, 24.0),
            self.q.at(n).clamp(0.1, 18.0),
        ];
        if self.designed != Some(p) {
            self.c = design(self.kind, p[0], p[1], p[2], f64::from(RATE));
            self.designed = Some(p);
        }
    }
}

/// The EQ's running state.
pub struct Eq {
    bands: Vec<Band>,
}

impl Eq {
    /// The EQ of `plan` over the item's samples `[start, end)`.
    pub fn new(plan: &AudioEffectPlan, start: i64, end: i64) -> Result<Eq, CoreError> {
        let params = ParamsAt::new(plan, start, end)?;
        let bands = (1..=8)
            .filter_map(|b| {
                let kind = kind_of(params.text(&format!("band{b}Type"))?)?;
                Some(Band {
                    kind,
                    freq: params.curve(&format!("band{b}Freq"), DEFAULT_FREQ[b - 1]),
                    gain: params.curve(&format!("band{b}Gain"), 0.0),
                    q: params.curve(&format!("band{b}Q"), 0.707),
                    designed: None,
                    c: Coeffs::IDENTITY,
                    state: [Biquad::default(); 2],
                })
            })
            .collect();
        Ok(Eq { bands })
    }
}

impl Effect for Eq {
    fn update(&mut self, n: i64) {
        for b in &mut self.bands {
            b.update(n);
        }
    }

    fn run(&mut self, ch: &mut [&mut [f32]]) {
        for b in &mut self.bands {
            for (x, st) in ch.iter_mut().zip(&mut b.state) {
                st.run(&b.c, x);
            }
        }
    }
}
