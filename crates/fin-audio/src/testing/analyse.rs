//! Measurements on `f32` signals, in `f64`.

use crate::testing::signals::db_to_amp;

const TAU: f64 = 2.0 * core::f64::consts::PI;

fn to_db(power: f64) -> f64 {
    if power <= 0.0 {
        f64::NEG_INFINITY
    } else {
        10.0 * libm::log10(power)
    }
}

/// RMS level in dBFS (`-inf` for silence).
pub fn rms_dbfs(x: &[f32]) -> f64 {
    if x.is_empty() {
        return f64::NEG_INFINITY;
    }
    let s: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    to_db(s / x.len() as f64)
}

/// Sample peak in dBFS.
pub fn peak_dbfs(x: &[f32]) -> f64 {
    let p = x.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
    if p == 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * libm::log10(p)
    }
}

/// The level in dBFS of the sine component at `hz` (single-bin Goertzel; a full-scale sine
/// reads 0 dB), exact for whole numbers of periods and close otherwise.
pub fn goertzel_db(x: &[f32], hz: f64, rate: u32) -> f64 {
    let (re, im) = correlate(x, hz, rate);
    let amp = 2.0 * libm::sqrt(re * re + im * im) / x.len() as f64;
    if amp == 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * libm::log10(amp)
    }
}

/// `(Σ x·cos, Σ x·sin)` at `hz`, by the Goertzel recurrence.
fn correlate(x: &[f32], hz: f64, rate: u32) -> (f64, f64) {
    let w = TAU * hz / f64::from(rate);
    let coeff = 2.0 * libm::cos(w);
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &v in x {
        let s = f64::from(v) + coeff * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    (s1 - s2 * libm::cos(w), s2 * libm::sin(w))
}

/// THD+N in dB relative to the fundamental: the fundamental is fitted by least squares
/// (`a·cos + b·sin`, orthogonal over whole periods and solved exactly otherwise) and removed;
/// the residual's RMS over the fitted component's RMS. A pure sine reads far below −120 dB.
pub fn thd_n_db(x: &[f32], fundamental_hz: f64, rate: u32) -> f64 {
    let w = TAU * fundamental_hz / f64::from(rate);
    let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0f64, 0.0, 0.0, 0.0, 0.0);
    for (n, &v) in x.iter().enumerate() {
        let (s, c) = (libm::sin(w * n as f64), libm::cos(w * n as f64));
        let v = f64::from(v);
        cc += c * c;
        ss += s * s;
        cs += c * s;
        xc += v * c;
        xs += v * s;
    }
    let det = cc * ss - cs * cs;
    let (a, b) = ((xc * ss - xs * cs) / det, (xs * cc - xc * cs) / det);
    let (mut res, mut fit) = (0.0f64, 0.0f64);
    for (n, &v) in x.iter().enumerate() {
        let f = a * libm::cos(w * n as f64) + b * libm::sin(w * n as f64);
        let d = f64::from(v) - f;
        res += d * d;
        fit += f * f;
    }
    to_db(res / fit)
}

/// The dominant frequency from rising zero crossings, interpolated linearly between samples.
pub fn dominant_hz(x: &[f32], rate: u32) -> f64 {
    let mut first = None;
    let (mut last, mut count) = (0.0f64, 0u32);
    for i in 1..x.len() {
        let (a, b) = (f64::from(x[i - 1]), f64::from(x[i]));
        if a < 0.0 && b >= 0.0 {
            let pos = (i - 1) as f64 + (-a) / (b - a);
            if first.is_none() {
                first = Some(pos);
            }
            last = pos;
            count += 1;
        }
    }
    match first {
        Some(f) if count > 1 => f64::from(count - 1) * f64::from(rate) / (last - f),
        _ => 0.0,
    }
}

/// The level of `a − b` in dBFS RMS (over the shorter length): how well two signals null.
pub fn null_db(a: &[f32], b: &[f32]) -> f64 {
    let d: Vec<f32> = a.iter().zip(b).map(|(x, y)| x - y).collect();
    rms_dbfs(&d)
}

/// `10^(db/20)`, re-exported for tests.
pub fn amp(db: f64) -> f64 {
    db_to_amp(db)
}
