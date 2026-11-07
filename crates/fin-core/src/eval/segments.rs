//! Piecewise curves for audio plans: a keyframed number cut into pieces at given breakpoints.
//!
//! A piece lies inside one keyframe span. It carries the span's end values `v`, its `interp`,
//! and `u`, the piece's progress range through the span, so the value at progress `x` of the
//! piece is `lerp(v[0], v[1], ease(lerp(u[0], u[1], x)))` (exact for bézier spans cut by a
//! fade or the range, which a re-fitted curve would not be). Before the first keyframe, after
//! the last, without keyframes, and in a `hold` span the value is constant: `v[0] = v[1]`,
//! `interp` hold.

use crate::error::CoreError;
use crate::eval::interp::progress;
use crate::model::{Interp, Keyframe, ParamValue};
use crate::time::Rational;

/// One piece of a curve (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Piece {
    pub v: [f64; 2],
    pub interp: Interp,
    pub u: [f64; 2],
}

/// A keyframed number on the timeline: keyframe times are `item start + at`.
pub(crate) struct NumberCurve {
    times: Vec<Rational>,
    values: Vec<f64>,
    interps: Vec<Interp>,
    base: f64,
}

impl NumberCurve {
    /// The number keyframes of `kfs` (others are ignored), sorted by time; `base` when none.
    pub fn new(kfs: &[Keyframe], base: f64, start: Rational) -> Result<NumberCurve, CoreError> {
        let mut keys: Vec<(Rational, f64, Interp)> = Vec::new();
        for k in kfs {
            if let ParamValue::Number(v) = k.value {
                keys.push((start.checked_add(k.at)?, v, k.interp));
            }
        }
        keys.sort_by_key(|k| k.0);
        Ok(NumberCurve {
            times: keys.iter().map(|k| k.0).collect(),
            values: keys.iter().map(|k| k.1).collect(),
            interps: keys.iter().map(|k| k.2).collect(),
            base,
        })
    }

    /// The keyframe times, where pieces must break.
    pub fn cuts(&self) -> &[Rational] {
        &self.times
    }

    /// The piece `[p, q)`, which must not contain a keyframe time strictly inside.
    pub fn piece(&self, p: Rational, q: Rational) -> Piece {
        let constant = |v: f64| Piece {
            v: [v, v],
            interp: Interp::Hold,
            u: [0.0, 1.0],
        };
        let n = self.times.len();
        let i = self.times.partition_point(|t| *t <= p);
        if n == 0 {
            return constant(self.base);
        }
        if i == 0 {
            return constant(self.values[0]);
        }
        if i == n {
            return constant(self.values[n - 1]);
        }
        let (a, b) = (self.times[i - 1], self.times[i]);
        let u = [progress(a, b, p), progress(a, b, q)];
        let interp = self.interps[i - 1];
        let v0 = self.values[i - 1];
        let v = match interp {
            Interp::Hold => [v0, v0],
            Interp::Linear | Interp::Bezier { .. } => [v0, self.values[i]],
        };
        Piece { v, interp, u }
    }
}

/// `lo`, `hi` and every cut strictly between them, sorted and without repeats.
pub(crate) fn breakpoints(
    lo: Rational,
    hi: Rational,
    cuts: impl IntoIterator<Item = Rational>,
) -> Vec<Rational> {
    let mut out: Vec<Rational> = cuts.into_iter().filter(|c| lo < *c && *c < hi).collect();
    out.push(lo);
    out.push(hi);
    out.sort();
    out.dedup();
    out
}
