//! Rational time to sample index.

use fin_core::{CoreError, Rational};

use crate::RATE;

/// `round_half_up(t × 48000)`, exact (never through a float): `floor((2·n·48000 + d) / 2d)`
/// for `t = n/d`. `TIME_OVERFLOW` when the index does not fit an `i64`.
pub fn sample_at(t: Rational) -> Result<i64, CoreError> {
    let n = i128::from(t.num()) * i128::from(RATE);
    let d = i128::from(t.den());
    let rounded = (2 * n + d).div_euclid(2 * d);
    i64::try_from(rounded).map_err(|_| {
        CoreError::new("TIME_OVERFLOW", "a time does not fit a 64-bit sample index")
            .with_fix("use a smaller time")
    })
}
