//! Codecs (sub-project 6 spec §6, §18): `VideoDecoder`/`VideoEncoder` traits, the software AV1
//! backend (dav1d decode, SVT-AV1 encode) in 6A, platform backends from 6B, backend choice
//! under the encoder policy (`registry`), encoder sessions with `ENCODER_HUNG` (`hung`), the
//! capability probe and `FrameReader` (random access by presentation index). Linked only by
//! `fin-workers`. `unsafe` lives only in `av1/dav1d_ffi.rs` and `av1/svt_ffi.rs`.

pub mod audio;
pub mod av1;
mod caps;
mod config;
#[cfg(feature = "testing")]
pub mod fixture;
mod frame;
mod hung;
mod reader;
mod registry;
#[cfg(feature = "testing")]
pub mod testing;
mod traits;

pub use audio::{
    AudioConfig, AudioDecoder, AudioEncoder, AudioEncoderConfig, AudioLimits, AudioReader,
    DecodedAudio, EncodedLength, open_audio_decoder, open_audio_encoder,
};
pub use av1::{DEFAULT_PRESET, DecodeThreads};
pub use caps::{Capabilities, CodecSupport, Support, capabilities};
pub use config::{AdapterBinding, DecoderConfig, EncoderConfig, Gop, RateControl, StreamDesc};
pub use frame::{DecodedFrame, EncodedPacket, Plane, RawFrame};
pub use hung::{EncoderSession, FallbackNote, FallbackReason, Fed, HUNG_FRAMES, Output};
pub use reader::FrameReader;
pub use registry::{
    DecoderBackend, EncoderBackend, OpenToken, ProbeResult, open_decoder, open_encoder,
};
pub use traits::{BackendId, VideoDecoder, VideoEncoder};
