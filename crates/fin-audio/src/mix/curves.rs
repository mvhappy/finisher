//! Gain and pan curves on the sample grid (plan 6C Decisions 4 and 16).
//!
//! A plan segment `[from, to)` becomes a piece over samples `[sample_at(from), sample_at(to))`
//! (each edge from its own rational, so edges never drift). Its value at sample `c` takes the
//! progress `x = (c − start)/(end − start)` (`0` at the first sample, `1` at the end edge) and
//! reads it like the plan says: `lerp(v0, v1, ease(lerp(u0, u1, x)))`. A piece that rounds to
//! no samples is dropped; samples of the item no piece covers are a gap (silence for gain,
//! centre for pan).
//!
//! **Control points.** Values are evaluated only at control points, the multiples of
//! [`CONTROL`] on the timeline's sample grid and every piece (or gap) edge, and ramped linearly
//! in amplitude between them ([`Control`]); the interval holding a sample, and so the sample's
//! amplitude, is a function of the sample index alone, so the output cannot depend on the
//! block length. All of it is scalar `f32`/`f64` in a fixed order with `libm` for `powf` and
//! `sqrtf`; `bezier_y` is `fin-core`'s (`+ − × ÷` only).

use fin_core::eval::{GainSegment, PanSegment, Shape, ShapeKind, bezier_y};
use fin_core::{CoreError, Curve, Interp, Rational};

use crate::sample_at;

/// Samples between grid control points.
pub const CONTROL: i64 = 32;

/// `(1 − e)·a + e·b`, and exactly `a` when both ends are equal (a constant stays constant).
fn lerp(a: f64, b: f64, e: f64) -> f64 {
    if a == b { a } else { (1.0 - e) * a + e * b }
}

fn ease(interp: &Interp, u: f64) -> f64 {
    match *interp {
        Interp::Hold => 0.0,
        Interp::Linear => u,
        Interp::Bezier { x1, y1, x2, y2 } => bezier_y([x1, y1, x2, y2], u),
    }
}

/// One plan segment on the sample grid.
#[derive(Debug, Clone)]
pub(crate) struct Piece<T> {
    pub start: i64,
    pub end: i64,
    v: [f64; 2],
    interp: Interp,
    u: [f64; 2],
    pub extra: T,
}

impl<T> Piece<T> {
    fn new(
        from: Rational,
        to: Rational,
        v: [f64; 2],
        interp: Interp,
        u: [f64; 2],
        extra: T,
    ) -> Result<Piece<T>, CoreError> {
        Ok(Piece {
            start: sample_at(from)?,
            end: sample_at(to)?,
            v,
            interp,
            u,
            extra,
        })
    }

    /// The progress of control point `c` through the piece, in `[0, 1]`.
    pub fn progress(&self, c: i64) -> f64 {
        ((c - self.start) as f64 / (self.end - self.start) as f64).clamp(0.0, 1.0)
    }

    /// The plan value at control point `c`.
    pub fn value(&self, c: i64) -> f64 {
        let x = self.progress(c);
        lerp(
            self.v[0],
            self.v[1],
            ease(&self.interp, lerp(self.u[0], self.u[1], x)),
        )
    }
}

/// Where a sample lies: the piece holding it (`None` in a gap) and that piece's (or gap's)
/// sample bounds.
pub(crate) struct Located<'a, T> {
    pub lo: i64,
    pub hi: i64,
    pub piece: Option<&'a Piece<T>>,
}

/// An item's segments in order over its samples `[start, end)`.
#[derive(Debug, Clone)]
pub(crate) struct Piecewise<T> {
    pieces: Vec<Piece<T>>,
    start: i64,
    end: i64,
}

impl<T> Piecewise<T> {
    fn new(mut pieces: Vec<Piece<T>>, start: i64, end: i64) -> Piecewise<T> {
        pieces.retain(|p| p.start < p.end);
        pieces.sort_by_key(|p| p.start);
        Piecewise { pieces, start, end }
    }

    pub fn locate(&self, n: i64) -> Located<'_, T> {
        let i = self.pieces.partition_point(|p| p.end <= n);
        let lo_gap = if i == 0 {
            self.start
        } else {
            self.pieces[i - 1].end
        };
        match self.pieces.get(i) {
            Some(p) if p.start <= n => Located {
                lo: p.start,
                hi: p.end,
                piece: Some(p),
            },
            Some(p) => Located {
                lo: lo_gap.min(n),
                hi: p.start,
                piece: None,
            },
            None => Located {
                lo: lo_gap.min(n),
                hi: self.end.max(n + 1),
                piece: None,
            },
        }
    }
}

/// The gain curve: dB plus the fade and crossfade shapes, as an amplitude.
pub(crate) type Gain = Piecewise<Vec<Shape>>;
/// The pan curve, clamped to `[−1, 1]`.
pub(crate) type Pan = Piecewise<()>;

pub(crate) fn gain_curve(segs: &[GainSegment], start: i64, end: i64) -> Result<Gain, CoreError> {
    let pieces = segs
        .iter()
        .map(|s| Piece::new(s.from, s.to, s.db, s.interp, s.u, s.shapes.clone()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Piecewise::new(pieces, start, end))
}

pub(crate) fn pan_curve(segs: &[PanSegment], start: i64, end: i64) -> Result<Pan, CoreError> {
    let pieces = segs
        .iter()
        .map(|s| Piece::new(s.from, s.to, s.pan, s.interp, s.u, ()))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Piecewise::new(pieces, start, end))
}

/// A shape's amplitude factor at progress `p` through the whole fade: `curve(p)` in,
/// `curve(1 − p)` out; `linear` is `x`, `equalPower` `sqrt(x)`.
fn shape_factor(s: &Shape, x: f64) -> f32 {
    let p = lerp(s.p[0], s.p[1], x).clamp(0.0, 1.0);
    let arg = match s.kind {
        ShapeKind::FadeIn | ShapeKind::CrossIn => p,
        ShapeKind::FadeOut | ShapeKind::CrossOut => 1.0 - p,
    } as f32;
    match s.curve {
        Curve::Linear => arg,
        Curve::EqualPower => libm::sqrtf(arg),
    }
}

/// The gain amplitude at control point `c`: `10^(dB/20)` (`libm::powf`) times each shape's
/// factor in plan order; 0 in a gap.
pub(crate) fn gain_at(at: &Located<'_, Vec<Shape>>, c: i64) -> f32 {
    let Some(p) = at.piece else { return 0.0 };
    let db = p.value(c);
    let mut a = libm::powf(10.0, (db / 20.0) as f32);
    if !p.extra.is_empty() {
        let x = p.progress(c);
        for s in &p.extra {
            a *= shape_factor(s, x);
        }
    }
    a
}

/// The pan at control point `c`, clamped to `[−1, 1]`; centre in a gap.
pub(crate) fn pan_at(at: &Located<'_, ()>, c: i64) -> f32 {
    at.piece.map_or(0.0, |p| p.value(c).clamp(-1.0, 1.0) as f32)
}

/// One control interval `[c0, c1)` and the per-side amplitudes at its two ends.
pub(crate) struct Control {
    pub c0: i64,
    pub c1: i64,
    pub l: [f32; 2],
    pub r: [f32; 2],
}

impl Control {
    /// The amplitude ramps at sample `n` (`c0 ≤ n < c1`): `a0 + (a1 − a0)·t`, which is exactly
    /// `a0` when the ends agree.
    pub fn at(&self, n: i64) -> (f32, f32) {
        let t = (n - self.c0) as f32 / (self.c1 - self.c0) as f32;
        (
            self.l[0] + (self.l[1] - self.l[0]) * t,
            self.r[0] + (self.r[1] - self.r[0]) * t,
        )
    }
}

/// The control interval holding sample `n`: the grid cell, cut at every gain and pan piece
/// (or gap) edge.
pub(crate) fn interval(n: i64, g: &Located<'_, Vec<Shape>>, p: &Located<'_, ()>) -> (i64, i64) {
    let cell = n.div_euclid(CONTROL) * CONTROL;
    let c0 = cell.max(g.lo).max(p.lo);
    let c1 = (cell + CONTROL).min(g.hi).min(p.hi);
    (c0, c1)
}
