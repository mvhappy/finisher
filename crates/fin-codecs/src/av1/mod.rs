//! Software AV1 (spec §6.4): dav1d decode, SVT-AV1 encode (both built from pinned submodules).

mod backends;
mod dav1d_ffi;
mod decode;
mod svt;
mod svt_ffi;

pub use backends::{DEFAULT_PRESET, Dav1dBackend, SvtBackend};
pub use decode::{Dav1dDecoder, DecodeThreads, MAX_PIXELS};
