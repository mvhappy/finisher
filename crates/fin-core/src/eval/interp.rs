//! Keyframe interpolation (sub-project 4 spec §5.4; plan 4C Decisions 12 and 42). Only `+ − × ÷`,
//! comparisons, `abs`, `min`/`max`/`clamp` and `floor` on `f64`, so every platform computes the
//! same bits.

use crate::eval::plan::PlanValue;
use crate::model::{Interp, Keyframe, ParamValue};
use crate::params::ParamType;
use crate::time::Rational;

/// How a parameter's values move between keyframes. Numbers, `vec2`s and colours interpolate;
/// `Step` values (bools, enums, text, asset refs) hold until the next keyframe. A value whose
/// shape does not match the type (or its neighbour's) holds too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueType {
    Number,
    Vec2,
    /// A `#rrggbb` or `#rrggbbaa` string, interpolated per sRGB component.
    Color,
    Step,
}

impl From<&ParamType> for ValueType {
    /// A registry type's interpolation: numbers, `vec2`s and colours move; everything else
    /// (bools, enums, text, times, rationals, sizes, asset refs) steps.
    fn from(ty: &ParamType) -> ValueType {
        match ty {
            ParamType::Number { .. } => ValueType::Number,
            ParamType::Vec2 { .. } => ValueType::Vec2,
            ParamType::Color => ValueType::Color,
            _ => ValueType::Step,
        }
    }
}

/// The CSS easing keywords as `cubic-bezier` control points `[x1, y1, x2, y2]`.
pub const PRESETS: &[(&str, [f64; 4])] = &[
    ("ease", [0.25, 0.1, 0.25, 1.0]),
    ("easeIn", [0.42, 0.0, 1.0, 1.0]),
    ("easeOut", [0.0, 0.0, 0.58, 1.0]),
    ("easeInOut", [0.42, 0.0, 0.58, 1.0]),
];

/// The value of `kfs` at the item-relative time `at`, or `base` when there are none. Before
/// the first keyframe the first value holds, after the last the last (keyframes outside the
/// item still shape the curve, so a trim never changes what stays visible). Between two
/// keyframes the earlier one's `interp` applies. `kfs` is sorted by `at` (it is sorted here
/// if not); among equal times the later keyframe wins.
pub fn interpolate(kfs: &[Keyframe], at: Rational, base: &ParamValue, ty: &ValueType) -> PlanValue {
    if kfs.is_empty() {
        return resolve(base, *ty);
    }
    if !kfs.is_sorted_by(|a, b| a.at <= b.at) {
        let mut sorted = kfs.to_vec();
        sorted.sort_by_key(|k| k.at);
        return interpolate(&sorted, at, base, ty);
    }
    let i = kfs.partition_point(|k| k.at <= at);
    if i == 0 {
        return resolve(&kfs[0].value, *ty);
    }
    if i == kfs.len() {
        return resolve(&kfs[i - 1].value, *ty);
    }
    let (a, b) = (&kfs[i - 1], &kfs[i]);
    let e = match a.interp {
        Interp::Hold => return resolve(&a.value, *ty),
        Interp::Linear => progress(a.at, b.at, at),
        Interp::Bezier { x1, y1, x2, y2 } => bezier_y([x1, y1, x2, y2], progress(a.at, b.at, at)),
    };
    mix(&a.value, &b.value, e, *ty).unwrap_or_else(|| resolve(&a.value, *ty))
}

/// `(at − a) / (b − a)` in `[0, 1]`: exact in rationals, then rounded once (an overflow falls
/// back to `f64` division). `a < b`.
pub(crate) fn progress(a: Rational, b: Rational, at: Rational) -> f64 {
    let exact = at
        .checked_sub(a)
        .and_then(|n| b.checked_sub(a).and_then(|d| n.checked_div(d)));
    let u = match exact {
        Ok(u) => u.to_f64(),
        Err(_) => (at.to_f64() - a.to_f64()) / (b.to_f64() - a.to_f64()),
    };
    u.clamp(0.0, 1.0)
}

/// `(1 − e)·a + e·b`, exact at both ends.
pub(crate) fn lerp(a: f64, b: f64, e: f64) -> f64 {
    (1.0 - e) * a + e * b
}

fn mix(a: &ParamValue, b: &ParamValue, e: f64, ty: ValueType) -> Option<PlanValue> {
    match (ty, a, b) {
        (ValueType::Number, ParamValue::Number(a), ParamValue::Number(b)) => {
            Some(PlanValue::Number(lerp(*a, *b, e)))
        }
        (ValueType::Vec2, ParamValue::Vec2(a), ParamValue::Vec2(b)) => {
            Some(PlanValue::Vec2([lerp(a[0], b[0], e), lerp(a[1], b[1], e)]))
        }
        (ValueType::Color, ParamValue::Text(a), ParamValue::Text(b)) => {
            let (a, b) = (parse_colour(a)?, parse_colour(b)?);
            let mut c = [0.0; 4];
            for k in 0..4 {
                c[k] = lerp(a[k], b[k], e).clamp(0.0, 1.0);
            }
            Some(PlanValue::Color(c))
        }
        _ => None,
    }
}

/// A stored value as a plan value: a colour string under [`ValueType::Color`] becomes its
/// components; everything else is carried as is.
pub(crate) fn resolve(v: &ParamValue, ty: ValueType) -> PlanValue {
    match v {
        ParamValue::Bool(b) => PlanValue::Bool(*b),
        ParamValue::Number(n) => PlanValue::Number(*n),
        ParamValue::Vec2(v) => PlanValue::Vec2(*v),
        ParamValue::Text(s) => match (ty, parse_colour(s)) {
            (ValueType::Color, Some(c)) => PlanValue::Color(c),
            _ => PlanValue::Text(s.clone()),
        },
        ParamValue::Asset { asset } => PlanValue::Asset {
            asset: asset.clone(),
        },
    }
}

/// `#rrggbb` or `#rrggbbaa` (either case) as `[r, g, b, a]` in 0…1 (`c / 255`); alpha 1
/// when absent.
pub fn parse_colour(s: &str) -> Option<[f64; 4]> {
    let hex = s.strip_prefix('#')?;
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut c = [1.0; 4];
    for (k, slot) in c.iter_mut().enumerate().take(hex.len() / 2) {
        let byte = u8::from_str_radix(&hex[2 * k..2 * k + 2], 16).ok()?;
        *slot = f64::from(byte) / 255.0;
    }
    Some(c)
}

/// `#rrggbbaa` (lower case) of a plan colour, each component `floor(c × 255 + 0.5)` after
/// clamping to 0…1 (plan 4C Decision 12).
pub fn colour_hex(c: [f64; 4]) -> String {
    let mut out = String::from("#");
    for v in c {
        let byte = (v.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

const NEWTON_STEPS: usize = 8;
const BISECTION_STEPS: usize = 32;
const X_EPSILON: f64 = 1e-7;
const SLOPE_EPSILON: f64 = 1e-6;

/// CSS `cubic-bezier(x1, y1, x2, y2)` at input progress `u` (plan 4C Decision 42): the curve
/// parameter `s` with `x(s) = u` is found by 8 Newton steps from `s = u` (accepted once
/// `|x(s) − u| < 1e-7` with `s` in `[0, 1]`; a step stops when `|x'(s)| ≤ 1e-6`), else by 32
/// bisection steps over `[0, 1]`; then `y(s)`. `x1` and `x2` are clamped to `[0, 1]` (CSS
/// requires it) and `u` too; `y` may leave `[0, 1]` (overshoot).
pub fn bezier_y(c: [f64; 4], u: f64) -> f64 {
    let u = u.clamp(0.0, 1.0);
    if u == 0.0 || u == 1.0 {
        return u;
    }
    let (x1, x2) = (c[0].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0));
    let x = Poly::new(x1, x2);
    let y = Poly::new(c[1], c[3]);
    y.at(solve(&x, u))
}

/// `((a·s + b)·s + c)·s`, the cubic Bézier coordinate with end points 0 and 1.
struct Poly {
    a: f64,
    b: f64,
    c: f64,
}

impl Poly {
    fn new(p1: f64, p2: f64) -> Poly {
        let c = 3.0 * p1;
        let b = 3.0 * (p2 - p1) - c;
        Poly {
            a: 1.0 - c - b,
            b,
            c,
        }
    }

    fn at(&self, s: f64) -> f64 {
        ((self.a * s + self.b) * s + self.c) * s
    }

    fn slope(&self, s: f64) -> f64 {
        (3.0 * self.a * s + 2.0 * self.b) * s + self.c
    }
}

fn solve(x: &Poly, u: f64) -> f64 {
    let mut s = u;
    for _ in 0..NEWTON_STEPS {
        let err = x.at(s) - u;
        if err.abs() < X_EPSILON && (0.0..=1.0).contains(&s) {
            return s;
        }
        let d = x.slope(s);
        if d.abs() <= SLOPE_EPSILON {
            break;
        }
        s -= err / d;
    }
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    s = u;
    for _ in 0..BISECTION_STEPS {
        let v = x.at(s);
        if (v - u).abs() < X_EPSILON {
            return s;
        }
        if v < u {
            lo = s;
        } else {
            hi = s;
        }
        s = (lo + hi) * 0.5;
    }
    s
}
