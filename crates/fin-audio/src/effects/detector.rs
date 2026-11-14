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
//! `f32` once before `log10f`). An `f32` square of an `f32` sample is exact in `f64`, and the
//! wider sum keeps the window's rounding far below the level's resolution.
//!
//! The level is in dBFS of that measure (a sine's RMS is its peak − 3.01 dB), computed in `f32`
//! with `libm::log10f` and floored at −200 dBFS, so silence never reaches `log(0)`.
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
            20.0 * libm::log10f(max)
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
        let ms = (self.sum / WINDOW as f64) as f32;
        if ms > 1e-20 {
            10.0 * libm::log10f(ms)
        } else {
            FLOOR_DB
        }
    }
}

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
        self.g = a * self.g + (1.0 - a) * target;
        self.g
    }
}

/// `10^(db/20)` in `f32`.
#[inline]
pub fn amp(db: f32) -> f32 {
    libm::exp10f(db / 20.0)
}
