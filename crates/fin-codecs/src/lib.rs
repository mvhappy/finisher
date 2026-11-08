//! Codecs (sub-project 6 spec §6, §18): `VideoDecoder`/`VideoEncoder` traits, the software AV1
//! backend (dav1d decode, SVT-AV1 encode) in 6A, platform backends from 6B, the capability
//! probe and `FrameReader` (random access by presentation index). Linked only by `fin-workers`.
//! `unsafe` lives only in `av1/dav1d_ffi.rs` and `av1/svt_ffi.rs`.

pub mod av1;
mod caps;
#[cfg(feature = "testing")]
pub mod fixture;
mod frame;
mod reader;
mod traits;

pub use av1::{Av1EncoderConfig, DecodeThreads, open_av1_encoder};
pub use caps::{
    Capabilities, CodecSupport, Support, capabilities, open_decoder, open_decoder_with,
};
pub use frame::{DecodedFrame, EncodedPacket, Plane, RawFrame};
pub use reader::FrameReader;
pub use traits::{BackendId, VideoDecoder, VideoEncoder};
