//! Software AV1 (spec §6.4): dav1d decode, SVT-AV1 encode (both built from pinned submodules).

mod dav1d_ffi;
mod decode;
mod svt;
mod svt_ffi;

pub use decode::{Dav1dDecoder, DecodeThreads, MAX_PIXELS};
pub use svt::{Av1EncoderConfig, open_av1_encoder};
