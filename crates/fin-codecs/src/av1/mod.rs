//! Software AV1 (spec §6.4): `rav1d` decode, SVT-AV1 encode (built from the pinned submodule).

mod decode;
mod rav1d_ffi;
mod svt;
mod svt_ffi;

pub use decode::Rav1dDecoder;
pub use svt::{Av1EncoderConfig, open_av1_encoder};
