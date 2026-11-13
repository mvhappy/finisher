//! Loudness (plan 6C Decision 18, spec §8.6): the BS.1770 meter (wrapping `ebur128`; reported,
//! never hashed), the look-ahead true-peak limiter (A0: hashed, bit-identical) and the
//! normalisation decision between the export's two passes.

pub mod limiter;
pub mod meter;
pub mod normalise;

pub use limiter::Limiter;
pub use meter::{Measured, Meter};
pub use normalise::{GainDecision, normalise};
