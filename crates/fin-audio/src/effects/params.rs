//! An effect's parameters on the sample grid (plan 6C Decision 15).
//!
//! [`ParamsAt`] holds one [`AudioEffectPlan`]'s numbers as [`ParamCurve`]s and its other values
//! as they are. A number's segments are read exactly like gain (the mixer's
//! [`crate::mix::curves`]): segment `[from, to)` covers samples `[sample_at(from),
//! sample_at(to))` and its value at sample `n` is `lerp(v0, v1, ease(lerp(u0, u1, x)))` with
//! `x = (n − start)/(end − start)`. Effects read their numbers once per block, at the block's
//! first sample (Decision 16), never per sample.
//!
//! **For effect authors (Task 9 on).** Build the curves once in the effect's constructor with
//! [`ParamsAt::curve`] (a missing number becomes a constant default, so a plan from an older
//! model still plays), then call [`ParamCurve::at`] in [`super::Effect::update`]. Enums come
//! from [`ParamsAt::text`]. All of it is `f64`, `+ − × ÷` plus `fin-core`'s `bezier_y`.

use std::collections::BTreeMap;

use fin_core::CoreError;
use fin_core::eval::{AudioEffectPlan, ParamSegment, PlanValue};

use crate::mix::curves::{Piece, Piecewise};

/// One number of an effect over the item's samples.
#[derive(Debug, Clone)]
pub struct ParamCurve {
    pieces: Piecewise<()>,
    /// The value when there are no pieces.
    default: f64,
    /// `Some(v)` when the curve is `v` at every sample (no pieces, or every piece from `v` to
    /// `v`): [`ParamCurve::at`] then answers without a lookup, the value it would compute.
    constant: Option<f64>,
}

/// `Some(v)` when every piece holds `v` from end to end (a piece with equal ends is that value
/// at every progress, see [`Piece::value`]).
fn constant_of(pieces: &Piecewise<()>, default: f64) -> Option<f64> {
    let mut v = None;
    for p in pieces.pieces() {
        let [a, b] = p.ends();
        if a.to_bits() != b.to_bits() || v.is_some_and(|v: f64| v.to_bits() != a.to_bits()) {
            return None;
        }
        v = Some(a);
    }
    Some(v.unwrap_or(default))
}

impl ParamCurve {
    /// `segs` over the item's samples `[start, end)`. Errors: `TIME_OVERFLOW` when an edge does
    /// not fit a sample index.
    pub fn new(
        segs: &[ParamSegment],
        start: i64,
        end: i64,
        default: f64,
    ) -> Result<Self, CoreError> {
        let pieces = segs
            .iter()
            .map(|s| Piece::new(s.from, s.to, s.v, s.interp, s.u, ()))
            .collect::<Result<Vec<_>, _>>()?;
        let pieces = Piecewise::new(pieces, start, end);
        Ok(ParamCurve {
            constant: constant_of(&pieces, default),
            pieces,
            default,
        })
    }

    /// A number that never moves.
    pub fn constant(v: f64) -> Self {
        ParamCurve {
            pieces: Piecewise::new(Vec::new(), 0, 0),
            default: v,
            constant: Some(v),
        }
    }

    /// The value at sample `n`. Plans cover the item without gaps; outside every piece (before
    /// the first, after the last, or in a segment that rounded to no samples) the nearest
    /// piece's end value holds.
    pub fn at(&self, n: i64) -> f64 {
        if let Some(v) = self.constant {
            return v;
        }
        self.pieces.nearest(n).map_or(self.default, |p| p.value(n))
    }
}

/// An [`AudioEffectPlan`] ready to be read at block starts.
#[derive(Debug, Clone)]
pub struct ParamsAt {
    numbers: BTreeMap<String, ParamCurve>,
    values: BTreeMap<String, PlanValue>,
}

impl ParamsAt {
    /// `plan`'s numbers over the item's samples `[start, end)` (`start` = `sample_at(item.from)`).
    pub fn new(plan: &AudioEffectPlan, start: i64, end: i64) -> Result<Self, CoreError> {
        let numbers = plan
            .numbers
            .iter()
            .map(|(k, segs)| Ok((k.clone(), ParamCurve::new(segs, start, end, 0.0)?)))
            .collect::<Result<_, CoreError>>()?;
        Ok(ParamsAt {
            numbers,
            values: plan.values.clone(),
        })
    }

    /// The curve of number `name`, or the constant `default` when the plan lacks it.
    pub fn curve(&self, name: &str, default: f64) -> ParamCurve {
        self.numbers
            .get(name)
            .cloned()
            .unwrap_or_else(|| ParamCurve::constant(default))
    }

    /// Number `name` at sample `n` (`default` when the plan lacks it).
    pub fn number(&self, name: &str, n: i64, default: f64) -> f64 {
        self.numbers.get(name).map_or(default, |c| c.at(n))
    }

    /// Value `name` when it is text (an enum), else `None`.
    pub fn text(&self, name: &str) -> Option<&str> {
        match self.values.get(name) {
            Some(PlanValue::Text(t)) => Some(t),
            _ => None,
        }
    }
}
