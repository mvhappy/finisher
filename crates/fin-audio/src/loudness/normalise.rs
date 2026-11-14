//! The normalisation decision between an export's two passes (plan 6C Decision 18, spec §8.6):
//! one static gain, quantised to 0.01 dB so a measurement's last digits never reach the hashed
//! output, and whether the true-peak limiter must run.
//!
//! [`normalise`] is a fast prediction from the first measurement alone and can under-predict
//! what the limiter costs (dense transients far above the ceiling). The binding check is
//! [`verify`] on a second measurement of the output: **6E's export must measure after the gain
//! and the limiter and call `verify`**, raising its warning even when `normalise` gave none.
//! [`normalise_pcm`] runs both passes over an in-memory signal.

use fin_core::CoreError;
use serde_json::{Value, json};

use super::{Limiter, Measured, Meter};
use crate::Stereo;

/// Below this integrated loudness (the BS.1770 absolute gate) the input counts as silent.
const SILENT_LUFS: f64 = -70.0;
/// The most loudness the limiter may cost before the target counts as unreachable, in LU.
const MAX_LIMIT_LU: f64 = 1.0;

/// What to apply in the second pass.
#[derive(Debug, Clone, PartialEq)]
pub struct GainDecision {
    /// The static gain in dB, a whole number of hundredths.
    pub gain_db: f64,
    /// Whether the true-peak limiter must run after the gain.
    pub limit: bool,
    /// `LOUDNESS_TARGET_UNREACHABLE {measured, target}` (a warning, never a failure).
    pub warning: Option<CoreError>,
}

impl GainDecision {
    /// The gain as a linear `f32` factor (`10^(gain_db / 20)`, designed in `f64`, cast once).
    pub fn linear(&self) -> f32 {
        libm::pow(10.0, self.gain_db / 20.0) as f32
    }
}

fn unreachable(measured: Option<f64>, target: f64, why: &str) -> CoreError {
    CoreError::new(
        "LOUDNESS_TARGET_UNREACHABLE",
        format!("the loudness target {target} LUFS cannot be reached: {why}"),
    )
    .with_details(
        json!({ "measured": measured.map_or(Value::Null, Value::from), "target": target }),
    )
}

/// The gain that takes `m` to `target_lufs` under a `ceiling_dbtp` true-peak ceiling.
///
/// - Silent input (integrated `None` or below −70 LUFS): no gain, no limiter, and the warning.
/// - Otherwise the gain is `target − integrated` rounded to 0.01 dB; the limiter runs when the
///   true peak after the gain would exceed the ceiling.
/// - The limiter cannot raise sustained loudness above the ceiling: a stretch whose short-term
///   loudness after the gain is above the ceiling is held down by about the excess (a sine,
///   whose loudness equals its peak, exactly so). When that excess (the short-term maximum, or
///   the integrated loudness when there is none, plus the gain, minus the ceiling) is more than
///   1 LU, the target is unreachable: the gain and the limiter still apply, with the warning.
///   This is a prediction and can miss transient-driven loss; [`verify`] is the binding check.
pub fn normalise(m: &Measured, target_lufs: f64, ceiling_dbtp: f64) -> GainDecision {
    let Some(integrated) = m.integrated.filter(|&i| i >= SILENT_LUFS) else {
        return GainDecision {
            gain_db: 0.0,
            limit: false,
            warning: Some(unreachable(
                m.integrated,
                target_lufs,
                "the input is silent",
            )),
        };
    };
    let gain_db = libm::round((target_lufs - integrated) * 100.0) / 100.0;
    let limit = m.true_peak.is_some_and(|tp| tp + gain_db > ceiling_dbtp);
    let loudest = m.short_term_max.unwrap_or(integrated).max(integrated);
    let excess = loudest + gain_db - ceiling_dbtp;
    let warning = (excess > MAX_LIMIT_LU).then(|| {
        unreachable(
            Some(integrated),
            target_lufs,
            "the limiter would need more than 1 LU of reduction",
        )
    });
    GainDecision {
        gain_db,
        limit,
        warning,
    }
}

/// The binding check on the second pass: `after` is the measurement of the output (after the
/// gain and the limiter). `LOUDNESS_TARGET_UNREACHABLE {measured: after.integrated, target}`
/// when it is silent or misses the target by more than 1 LU, else `None`.
pub fn verify(after: &Measured, target_lufs: f64) -> Option<CoreError> {
    match after.integrated.filter(|&i| i >= SILENT_LUFS) {
        Some(i) if (i - target_lufs).abs() <= MAX_LIMIT_LU => None,
        Some(i) => Some(unreachable(
            Some(i),
            target_lufs,
            "the result misses it by more than 1 LU",
        )),
        None => Some(unreachable(
            after.integrated,
            target_lufs,
            "the result is silent",
        )),
    }
}

/// Both passes over an in-memory 48 kHz stereo signal: measure, [`normalise`], apply the gain
/// (and the limiter when the decision asks for it), measure again and [`verify`]. Returns the
/// output, the decision and the binding warning (the second pass's, or the decision's own for
/// silent input).
pub fn normalise_pcm(
    pcm: &Stereo,
    target_lufs: f64,
    ceiling_dbtp: f64,
) -> (Stereo, GainDecision, Option<CoreError>) {
    let mut meter = Meter::new(2);
    meter.push(pcm);
    let d = normalise(&meter.measured(), target_lufs, ceiling_dbtp);
    let g = d.linear();
    let mut out = Stereo {
        l: pcm.l.iter().map(|&v| v * g).collect(),
        r: pcm.r.iter().map(|&v| v * g).collect(),
    };
    if d.limit {
        let mut lim = Limiter::new(ceiling_dbtp);
        lim.process(&mut out);
        lim.finish(&mut out);
    }
    let mut after = Meter::new(2);
    after.push(&out);
    let warning = verify(&after.measured(), target_lufs);
    (out, d, warning)
}
