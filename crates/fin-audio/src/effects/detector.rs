//! The dynamics effects' shared parts (plan 6C Task 9): the linked level detector, the static
//! gain curve and the gain smoother.
//!
//! **Detection** is linked across the channels it is handed (1 for mono, 2 for L/R), over a
//! window of the last [`WINDOW`] samples (10 ms), the current one included:
//! - [`Measure::Peak`] (`detector: peak`, and the gate): the linked sample peak `max |x|` of
//!   every channel over the **same 10 ms window** (a sliding maximum in a monotonic queue,
//!   exact). A window, not the instantaneous `|x|`, so a steady tone reads its peak on every
//!   sample (the gain does not chase each half cycle) and the gate does not chatter at zero
//!   crossings. The cost: a level drop is seen only once the loud samples leave the window, so
//!   the compressor's release and the gate's hold countdown start ~10 ms (479 samples) after
//!   the drop. Attack is not delayed (a louder sample enters the window at once). The tests
//!   time release from the gain's first movement.
//! - [`Measure::Rms`]: the mean over the window of the per-frame mean square
//!   `(L² + R²)/2` (or `x²`). The squares are kept in a ring and summed into a running sum that
//!   adds the new square and removes the leaving one; the sum is **recomputed from the ring in
//!   ring order every time the ring wraps**. That bounds rounding error to what one window can
//!   accumulate (it does not make the running sum mathematically exact), and the value depends
//!   only on the item's samples since its first, never on how the calls cut them.
//!
//! **Exception to "scalar `f32`" (plan 6C Determinism):** the RMS squares, ring and sum are
//! `f64`. Only `+ − ×` and one division are used, which IEEE 754 rounds correctly on every
//! target (wasm32 included), in a fixed order and with no `mul_add`, so the result is
//! bit-identical everywhere; no `f64` transcendental is involved (the mean square is cast to
//! `f32` once before its logarithm). An `f32` square of an `f32` sample is exact in `f64`, and the
//! wider sum keeps the window's rounding far below the level's resolution.
//!
//! The level is in dBFS of that measure (a sine's RMS is its peak − 3.01 dB), computed in `f32`
//! as `log2` scaled to dB ([`log2_poly`]) and floored at −200 dBFS, so silence never reaches
//! `log(0)`. Gains in dB become amplitudes through [`exp2_poly`] ([`amp`]). Both are short
//! polynomials in `+ − ×` and bit operations (audioVersion 2, 6C Task 23: `libm::log10f` and
//! `libm::exp10f` per frame kept the mixer under spec §14's throughput), accurate to 2.5e-4 dB.
//!
//! **Gain** is computed and smoothed in dB, per sample, in `f32` with a fixed operation order
//! and no `mul_add`. [`static_gain`] is the soft-knee curve of Giannoulis, Massberg and Reiss
//! (2012); [`Smoother`] is a one-pole `g ← a·g + (1 − a)·target` whose coefficient
//! `a = exp(−1/(τ·48000))` ([`coeff`]) the caller picks per sample (attack or release).

use std::collections::VecDeque;

use crate::RATE;

/// The detection window: 10 ms at 48 kHz.
pub const WINDOW: usize = 480;

/// The floor of every detected level, in dBFS.
const FLOOR_DB: f32 = -200.0;

/// What the detector measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    Peak,
    Rms,
}

impl Measure {
    /// The registry's `detector` value (`rms` when absent or unknown, the default).
    pub fn parse(v: Option<&str>) -> Measure {
        match v {
            Some("peak") => Measure::Peak,
            _ => Measure::Rms,
        }
    }
}

/// A linked level detector over the last [`WINDOW`] frames.
#[derive(Debug, Clone)]
pub struct Detector {
    measure: Measure,
    /// Frames seen since the item's first sample.
    n: u64,
    /// Peak: `(frame, |x|)` with strictly decreasing values, the window's maximum first.
    queue: VecDeque<(u64, f32)>,
    /// Rms: the window's per-frame mean squares (zeros before the first frame).
    ring: Vec<f64>,
    pos: usize,
    sum: f64,
}

impl Detector {
    pub fn new(measure: Measure) -> Detector {
        let (queue, ring) = match measure {
            Measure::Peak => (VecDeque::with_capacity(WINDOW + 1), Vec::new()),
            Measure::Rms => (VecDeque::new(), vec![0.0; WINDOW]),
        };
        Detector {
            measure,
            n: 0,
            queue,
            ring,
            pos: 0,
            sum: 0.0,
        }
    }

    /// Takes the next frame (one value per channel) and returns the window's level in dBFS.
    pub fn level_db(&mut self, frame: &[f32]) -> f32 {
        let level = match self.measure {
            Measure::Peak => {
                let mut p = 0.0f32;
                for &v in frame {
                    p = p.max(v.abs());
                }
                self.peak(p)
            }
            Measure::Rms => {
                let mut sq = 0.0f64;
                for &v in frame {
                    let v = f64::from(v);
                    sq += v * v;
                }
                self.rms(sq / frame.len() as f64)
            }
        };
        self.n += 1;
        level
    }

    /// Frame `i` of `ch`'s channels.
    pub fn frame(ch: &[&mut [f32]], i: usize) -> ([f32; 2], usize) {
        let mut f = [0.0f32; 2];
        for (d, c) in f.iter_mut().zip(ch) {
            *d = c[i];
        }
        (f, ch.len().min(2))
    }

    /// What the detector measures.
    pub fn measure(&self) -> Measure {
        self.measure
    }

    /// [`Detector::level_db`] of every frame of `ch` (1 or 2 channels of equal length) into
    /// `out[..len]`: the same values in the same order, with the RMS window's state held in
    /// locals across the run (the ring's writes cannot then force the running sum back through
    /// memory on every frame).
    pub fn levels(&mut self, ch: &[&mut [f32]], out: &mut [f32]) {
        let len = ch.first().map_or(0, |x| x.len());
        let out = &mut out[..len];
        match (self.measure, ch) {
            (Measure::Rms, [l, r]) => self.rms_levels(out, |i| {
                let (l, r) = (f64::from(l[i]), f64::from(r[i]));
                (l * l + r * r) / 2.0
            }),
            (Measure::Rms, [m]) => self.rms_levels(out, |i| {
                let m = f64::from(m[i]);
                m * m
            }),
            _ => {
                for (i, o) in out.iter_mut().enumerate() {
                    let (f, k) = Detector::frame(ch, i);
                    *o = self.level_db(&f[..k]);
                }
            }
        }
    }

    /// The RMS levels of `out.len()` frames whose mean squares `sq(i)` gives, as
    /// [`Detector::rms`] one by one.
    #[inline]
    fn rms_levels(&mut self, out: &mut [f32], sq: impl Fn(usize) -> f64) {
        let (mut sum, mut pos) = (self.sum, self.pos);
        let ring = &mut self.ring[..WINDOW];
        for (i, o) in out.iter_mut().enumerate() {
            let sq = sq(i);
            sum += sq - ring[pos];
            ring[pos] = sq;
            pos += 1;
            if pos == WINDOW {
                pos = 0;
                sum = ring.iter().fold(0.0, |s, &v| s + v);
            }
            *o = rms_db(sum);
        }
        (self.sum, self.pos) = (sum, pos);
        self.n += out.len() as u64;
    }

    /// Takes one frame's linked `|x|`; the window's peak in dBFS.
    fn peak(&mut self, p: f32) -> f32 {
        while self.queue.back().is_some_and(|&(_, v)| v <= p) {
            self.queue.pop_back();
        }
        self.queue.push_back((self.n, p));
        while self
            .queue
            .front()
            .is_some_and(|&(k, _)| k + WINDOW as u64 <= self.n)
        {
            self.queue.pop_front();
        }
        let max = self.queue.front().map_or(0.0, |&(_, v)| v);
        if max > 1e-10 {
            DB_PER_OCTAVE_AMP * log2_poly(max)
        } else {
            FLOOR_DB
        }
    }

    /// Takes one frame's mean square; the window's mean square in dBFS.
    fn rms(&mut self, sq: f64) -> f32 {
        self.sum += sq - self.ring[self.pos];
        self.ring[self.pos] = sq;
        self.pos += 1;
        if self.pos == WINDOW {
            self.pos = 0;
            // The periodic recompute (see the module docs): a fixed order, once per window.
            self.sum = self.ring.iter().fold(0.0, |s, &v| s + v);
        }
        rms_db(self.sum)
    }
}

/// The window's mean square in dBFS from its running sum: `10·log10(sum/WINDOW)` as
/// `10·log10(2)·log2(sum) − 10·log10(WINDOW)`, so no division per frame. The floor is a mean
/// square of 1e-20.
#[inline]
fn rms_db(sum: f64) -> f32 {
    let s = sum as f32;
    if s > 1e-20 * WINDOW as f32 {
        DB_PER_OCTAVE_POWER * log2_poly(s) - DB_WINDOW
    } else {
        FLOOR_DB
    }
}

/// `10·log10(WINDOW)`: the dB of the RMS window's length.
const DB_WINDOW: f32 = 26.812_412;

/// The one-pole coefficient `exp(−1/(τ·48000))` for a time constant of `ms` milliseconds (the
/// argument in `f64`, cast once, then `libm::expf`).
pub fn coeff(ms: f64) -> f32 {
    libm::expf((-1.0 / (ms / 1000.0 * f64::from(RATE))) as f32)
}

/// The gain in dB (≤ 0) a compressor applies to a level of `x` dB: threshold `t`, `slope =
/// 1/ratio − 1` and knee width `w` dB (Giannoulis et al., the quadratic within `|x − t| ≤ w/2`).
#[inline]
pub fn static_gain(x: f32, t: f32, slope: f32, w: f32) -> f32 {
    let over = x - t;
    if w > 0.0 && over + over >= -w && over + over <= w {
        let k = over + w * 0.5;
        slope * (k * k) / (w + w)
    } else if over > 0.0 {
        slope * over
    } else {
        0.0
    }
}

/// A gain in dB smoothed by a one-pole filter.
#[derive(Debug, Clone, Copy)]
pub struct Smoother {
    /// The current gain in dB.
    pub g: f32,
}

impl Smoother {
    /// Moves towards `target` with coefficient `a`; the new gain.
    #[inline]
    pub fn step(&mut self, target: f32, a: f32) -> f32 {
        self.g = Smoother::next(self.g, target, a);
        self.g
    }

    /// The gain after `g` moves towards `target` with coefficient `a`.
    #[inline]
    pub fn next(g: f32, target: f32, a: f32) -> f32 {
        a * g + (1.0 - a) * target
    }
}

/// `10^(db/20)` in `f32`: `2^(db · log2(10)/20)` through [`exp2_poly`].
#[inline]
pub fn amp(db: f32) -> f32 {
    exp2_poly(db * OCTAVES_PER_DB_AMP)
}

/// `10·log10(2)`: dB per octave of a power (a mean square).
const DB_PER_OCTAVE_POWER: f32 = 3.010_3;
/// `20·log10(2)`: dB per octave of an amplitude.
const DB_PER_OCTAVE_AMP: f32 = 6.020_6;
/// `log2(10)/20`: octaves of amplitude per dB.
const OCTAVES_PER_DB_AMP: f32 = 0.166_096_4;

/// `log2(1 + t)/t` on `t ∈ [√½ − 1, √2 − 1]`: a degree-4 minimax fit, `c[0]` first (the
/// error of [`log2_poly`] is below 4e-5, i.e. below 2.5e-4 dB of an amplitude).
const LOG2_C: [f32; 5] = [
    1.442_263_6,
    -0.721_159_5,
    0.496_625_57,
    -0.381_153_76,
    0.182_369_6,
];

/// `(2^f − 1)/f` on `f ∈ [0, 1)`: a degree-3 minimax fit, `c[0]` first (the relative error of
/// [`exp2_poly`] is below 5e-6, i.e. below 5e-5 dB).
const EXP2_C: [f32; 4] = [0.693_018_56, 0.241_445_51, 0.051_950_548, 0.013_581_248];

/// `log2(x)` for a normal, finite `x > 0`; the caller guarantees it (the detectors' floors keep
/// subnormals, zero, negatives and NaN out). Outside that domain the answer is finite garbage, not
/// libm's `±inf`/NaN (`log2_poly(0)` is −127, `log2_poly(inf)` is 128). Exponent and mantissa from the bits, the mantissa folded into `[√½, √2)` (exact),
/// then `e + t·P(t)` with `t = m − 1` (exact) and [`LOG2_C`] by Horner. Only `+ − ×` and bit
/// operations in a fixed order, no `mul_add`: the same bits on every target (plan 6C
/// Determinism), and cheaper per frame than `libm::log10f` (6C Task 23 throughput).
#[inline]
pub fn log2_poly(x: f32) -> f32 {
    debug_assert!(
        x.is_normal() && x > 0.0,
        "log2_poly({x}) outside its domain"
    );
    let bits = x.to_bits();
    let mut e = ((bits >> 23) & 0xff) as i32 - 127;
    let mut m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if m > core::f32::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    let t = m - 1.0;
    let mut p = LOG2_C[4];
    for &c in LOG2_C[..4].iter().rev() {
        p = p * t + c;
    }
    e as f32 + t * p
}

/// `2^y` (`y` clamped to `[−126, 127]`, so the result is a normal `f32`): `2^i` from the bits
/// times `1 + f·P(f)` for `y = i + f`, `0 ≤ f ≤ 1` ([`EXP2_C`] by Horner; `f` rounds to 1 for a
/// tiny negative `y`, still within the bound). `amp(-inf)` is therefore 2^-126, not 0.
/// `+ − ×` and bit operations only, in a fixed order (see [`log2_poly`]).
#[inline]
pub fn exp2_poly(y: f32) -> f32 {
    let y = y.clamp(-126.0, 127.0);
    let mut i = y as i32;
    if i as f32 > y {
        i -= 1;
    }
    let f = y - i as f32;
    let mut p = EXP2_C[3];
    for &c in EXP2_C[..3].iter().rev() {
        p = p * f + c;
    }
    f32::from_bits(((i + 127) as u32) << 23) * (1.0 + f * p)
}
