//! Second-order sections: RBJ "Audio EQ Cookbook" designs in `f64`, run in `f32` (plan 6C
//! Determinism rules, Decision 16).
//!
//! - [`design`] computes the cookbook coefficients in `f64` with `libm` (`pow`, `sin`, `cos`,
//!   `sqrt`), normalises by `a0` and casts each to `f32` **once**. The low-pass, high-pass and
//!   band-pass numerators are cast once and the others derived from that `f32` by exact
//!   operations (`b1 = 2·b0`, `b1 = −2·b0`, `b2 = −b0`), so their zeros sit exactly where they
//!   should: a high-pass rejects DC exactly, a band-pass rejects DC and Nyquist.
//! - [`Biquad`] is one channel's transposed direct form II state, run in `f32` in a fixed
//!   order with no `mul_add`: `y = b0·x + s1; s1 = b1·x − a1·y + s2; s2 = b2·x − a2·y`.
//!   Coefficients are passed in on every call, so the owner may change them between calls
//!   (every 64 samples) while the state carries on.
//!
//! **The de-esser** ([`super::deesser`]) splits with `band = BandPass` (constant 0 dB peak
//! gain, `q`) and `residual = x − band`, one [`Biquad`] per channel.

/// Normalised coefficients (`a0 = 1`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl Coeffs {
    /// `y = x`.
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };
}

/// The cookbook filters used here. `gain_db` matters for the shelves and the peak only; the
/// shelves use slope `S = 1`, the others take `q`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BandKind {
    LowShelf,
    HighShelf,
    Peak,
    LowPass,
    HighPass,
    /// Constant 0 dB peak gain at `freq` (the cookbook's "BPF, peak gain = 0 dB").
    BandPass,
}

/// The cookbook coefficients for `kind` at `freq` Hz, `gain_db` and `q`, at `rate` Hz.
/// The caller keeps `0 < freq < rate/2` and `q > 0` (the EQ clamps to the registry ranges).
pub fn design(kind: BandKind, freq: f64, gain_db: f64, q: f64, rate: f64) -> Coeffs {
    let w0 = 2.0 * core::f64::consts::PI * freq / rate;
    let (sn, cs) = (libm::sin(w0), libm::cos(w0));
    let alpha = sn / (2.0 * q);
    let a = libm::pow(10.0, gain_db / 40.0);
    let (b, den) = match kind {
        BandKind::Peak => (
            [1.0 + alpha * a, -2.0 * cs, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cs, 1.0 - alpha / a],
        ),
        BandKind::LowShelf | BandKind::HighShelf => {
            // Slope 1: alpha = sin(w0)/2 · sqrt((A + 1/A)(1/S − 1) + 2) = sin(w0)/2 · sqrt(2).
            let k = 2.0 * libm::sqrt(a) * (sn / 2.0 * libm::sqrt(2.0));
            let (p, m) = (a + 1.0, a - 1.0);
            if kind == BandKind::LowShelf {
                (
                    [
                        a * (p - m * cs + k),
                        2.0 * a * (m - p * cs),
                        a * (p - m * cs - k),
                    ],
                    [p + m * cs + k, -2.0 * (m + p * cs), p + m * cs - k],
                )
            } else {
                (
                    [
                        a * (p + m * cs + k),
                        -2.0 * a * (m + p * cs),
                        a * (p + m * cs - k),
                    ],
                    [p - m * cs + k, 2.0 * (m - p * cs), p - m * cs - k],
                )
            }
        }
        BandKind::LowPass | BandKind::HighPass | BandKind::BandPass => {
            let a0 = 1.0 + alpha;
            let (a1, a2) = ((-2.0 * cs / a0) as f32, ((1.0 - alpha) / a0) as f32);
            let (b0, b1, b2) = match kind {
                BandKind::LowPass => {
                    let b0 = ((1.0 - cs) / 2.0 / a0) as f32;
                    (b0, b0 + b0, b0)
                }
                BandKind::HighPass => {
                    let b0 = ((1.0 + cs) / 2.0 / a0) as f32;
                    (b0, -(b0 + b0), b0)
                }
                _ => {
                    let b0 = (alpha / a0) as f32;
                    (b0, 0.0, -b0)
                }
            };
            return Coeffs { b0, b1, b2, a1, a2 };
        }
    };
    let a0 = den[0];
    Coeffs {
        b0: (b[0] / a0) as f32,
        b1: (b[1] / a0) as f32,
        b2: (b[2] / a0) as f32,
        a1: (den[1] / a0) as f32,
        a2: (den[2] / a0) as f32,
    }
}

/// One channel's transposed direct form II state.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Biquad {
    s1: f32,
    s2: f32,
}

impl Biquad {
    /// One sample through `c`.
    #[inline]
    pub fn tick(&mut self, c: &Coeffs, x: f32) -> f32 {
        let y = c.b0 * x + self.s1;
        self.s1 = c.b1 * x - c.a1 * y + self.s2;
        self.s2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Filters `x` in place through `c`.
    pub fn run(&mut self, c: &Coeffs, x: &mut [f32]) {
        for v in x {
            *v = self.tick(c, *v);
        }
    }
}
