//! Audio codecs (6C plan Decisions 3, 8–10, 22, 24): the decoder and encoder traits, backend
//! choice under the audio encoder policy (`registry`, the only place that opens an audio
//! encoder), the symphonia backend, MP4 audio through `fin-container`, and [`AudioReader`],
//! which hands out a file's audio at its own rate and channel layout. Nothing here resamples or
//! downmixes: `fin-codecs` never links `fin-audio`; the worker converts (`audio_src.rs`).

mod mp4;
mod reader;
mod registry;
mod sniff;
mod symphonia;
#[cfg(feature = "testing")]
pub mod testing;
mod traits;

pub use reader::{AudioLimits, AudioReader};
pub use registry::{AudioOpenToken, open_audio_decoder, open_audio_encoder};
pub use traits::{
    AudioConfig, AudioDecoder, AudioEncoder, AudioEncoderConfig, DecodedAudio, EncodedLength,
    SymphoniaParams,
};
