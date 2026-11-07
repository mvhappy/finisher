//! The `tkhd` matrix as a display rotation (spec §6.3): a rotation by a multiple of 90° (the
//! translation is ignored), anything else `UNSUPPORTED_TRANSFORM`.

use fin_core::CoreError;
use fin_media::Rotation;

use crate::error::unsupported_transform;

const ONE: i32 = 0x0001_0000;

/// `m` is `(a, b, u, c, d, v, x, y, w)` as stored.
pub(crate) fn rotation(m: [i32; 9]) -> Result<Rotation, CoreError> {
    match (m[0], m[1], m[3], m[4]) {
        (ONE, 0, 0, ONE) => Ok(Rotation::R0),
        (0, ONE, n, 0) if n == -ONE => Ok(Rotation::R90),
        (a, 0, 0, d) if a == -ONE && d == -ONE => Ok(Rotation::R180),
        (0, b, ONE, 0) if b == -ONE => Ok(Rotation::R270),
        // An all-zero matrix (some writers) reads as identity.
        (0, 0, 0, 0) => Ok(Rotation::R0),
        _ => Err(unsupported_transform(&m)),
    }
}
