//! The windowed-sinc prototype of plan 6C Decision 5 and its polyphase tables.
//!
//! Every coefficient is designed in `f64` with `libm` (the Bessel I₀ by its power series),
//! normalised to unit DC gain per phase, then cast once to `f32`. The filter is specified in
//! units of `fs_min = min(from, to)`: passband edge 0.4535·fs_min, stopband edge 0.5·fs_min,
//! −6 dB cutoff 0.4768·fs_min, Kaiser β = 10.06, half-length 70 samples at `fs_min`.

use core::f64::consts::PI;

/// The −6 dB cutoff in units of `fs_min` (the transition band's midpoint).
const CUTOFF: f64 = 0.4768;
/// The Kaiser window's β (the Kaiser formula for 100 dB stopband attenuation).
const BETA: f64 = 10.06;
/// The half-length in samples at `fs_min`.
const HALF_AT_MIN: u64 = 70;
/// Phase resolution for ratios whose `L` exceeds it: tables at `q / PHASES`, `q = 0..=PHASES`.
pub(super) const PHASES: u64 = 1024;

/// The zeroth-order modified Bessel function of the first kind: Σ ((x/2)^k / k!)².
fn bessel_i0(x: f64) -> f64 {
    let half = x / 2.0;
    let (mut sum, mut term) = (1.0f64, 1.0f64);
    for k in 1..200u32 {
        let t = half / f64::from(k);
        term *= t * t;
        sum += term;
        if term < sum * 1e-21 {
            break;
        }
    }
    sum
}

/// The polyphase tables for the reduced ratio `l / m` (output rate / input rate).
pub(super) struct Kernel {
    /// Taps either side of the output instant, in input samples: tap `j` of a table reads input
    /// `i − half + 1 + j`, where `i = floor(t)` for the output's source time `t`.
    pub half: usize,
    /// `tables[p]` holds `2·half` taps for phase `p / phases` of an input sample.
    pub tables: Vec<Vec<f32>>,
    /// `l` when every phase has its own table, else [`PHASES`] (interpolated between tables).
    pub phases: u64,
}

impl Kernel {
    /// The tables for `l / m` (reduced, both ≥ 1, not 1 / 1).
    pub fn design(l: u64, m: u64) -> Kernel {
        // In input samples: `fs_min / from` is 1 when upsampling and `l / m` when downsampling.
        let scale = if l >= m { 1.0 } else { l as f64 / m as f64 };
        let half_len = HALF_AT_MIN as f64 / scale;
        let half = if l >= m {
            HALF_AT_MIN
        } else {
            (HALF_AT_MIN * m).div_ceil(l)
        } as usize;
        let fc = CUTOFF * scale; // cycles per input sample
        let i0_beta = bessel_i0(BETA);
        let (phases, count) = if l <= PHASES {
            (l, l)
        } else {
            (PHASES, PHASES + 1)
        };
        let tables = (0..count)
            .map(|p| {
                let phi = p as f64 / phases as f64;
                let taps: Vec<f64> = (0..2 * half)
                    .map(|j| {
                        let d = phi + (half - 1) as f64 - j as f64;
                        tap(d, fc, half_len, i0_beta)
                    })
                    .collect();
                let mut dc = 0.0f64;
                for &c in &taps {
                    dc += c;
                }
                taps.iter().map(|&c| (c / dc) as f32).collect()
            })
            .collect();
        Kernel {
            half,
            tables,
            phases,
        }
    }
}

/// The prototype at `d` input samples from the output instant: `2fc·sinc(2fc·d)` under the
/// Kaiser window of half-length `half_len`.
fn tap(d: f64, fc: f64, half_len: f64, i0_beta: f64) -> f64 {
    let x = d / half_len;
    // The open interval: the window's endpoints are dropped on both sides alike, so a phase-0
    // table stays exactly symmetric.
    if x.abs() >= 1.0 {
        return 0.0;
    }
    let window = bessel_i0(BETA * libm::sqrt(1.0 - x * x)) / i0_beta;
    let arg = 2.0 * fc * d;
    let sinc = if arg == 0.0 {
        1.0
    } else {
        libm::sin(PI * arg) / (PI * arg)
    };
    2.0 * fc * sinc * window
}

/// `Σ c[j]·x[j]` in `f32`, in index order (no fused multiply-add, no reordering).
pub(super) fn dot(c: &[f32], x: &[f32]) -> f32 {
    let mut acc = 0.0f32;
    for (a, b) in c.iter().zip(x) {
        acc += a * b;
    }
    acc
}
