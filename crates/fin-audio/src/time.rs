//! Rational time to sample index.

use fin_core::{CoreError, Rational};

use crate::RATE;

/// `round_half_up(t × 48000)`, exact (never through a float): `floor((2·n·48000 + d) / 2d)`
/// for `t = n/d`. `TIME_OVERFLOW` when the index does not fit an `i64`.
pub fn sample_at(t: Rational) -> Result<i64, CoreError> {
    let n = i128::from(t.num()) * i128::from(RATE);
    let d = i128::from(t.den());
    let rounded = (2 * n + d).div_euclid(2 * d);
    i64::try_from(rounded).map_err(|_| time_overflow())
}

/// `TIME_OVERFLOW`: a sample index (or an exclusive end of a sample range) past `i64`.
pub(crate) fn time_overflow() -> CoreError {
    CoreError::new("TIME_OVERFLOW", "a time does not fit a 64-bit sample index")
        .with_fix("use a smaller time")
}

/// `at + frames`, the exclusive end of a sample range, or `TIME_OVERFLOW` when it does not
/// fit an `i64`.
pub(crate) fn range_end(at: i64, frames: u64) -> Result<i64, CoreError> {
    i64::try_from(frames)
        .ok()
        .and_then(|f| at.checked_add(f))
        .ok_or_else(time_overflow)
}
