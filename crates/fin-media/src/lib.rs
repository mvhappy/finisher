//! Pure media types (sub-project 6 spec §18): pixel formats, colour tags and matrices,
//! `MediaInfo`, the sample table and the nearest-frame rule `frame_at` (§10.2),
//! `RENDER_VERSION`, codec backends and the encoder policy, the media cache's keys and entries, media job params and results,
//! capability and template types; frame-id test patterns behind the `testing` feature. No I/O, no clocks, no randomness; builds for
//! `wasm32-unknown-unknown`.

pub mod audio_layout;
pub mod backend;
pub mod backend_audio;
pub mod cache;
pub mod colour;
pub mod format;
pub mod info;
pub mod jobs;
pub mod sample;
pub mod table_bin;
#[cfg(feature = "testing")]
pub mod testing;
mod version;

pub use colour::{ChromaSiting, ColourTags, Matrix, Primaries, Range, Transfer};
pub use format::{AudioCodec, Chroma, ContainerKind, PixelFormat, VideoCodec};
pub use info::{
    AudioTrackInfo, ContentLight, DolbyVision, HdrInfo, IgnoredTrack, MasteringDisplay, MediaInfo,
    Rotation, Size, VideoTrackInfo, display_size,
};
pub use sample::{FrameAt, FrameEntry, SampleTable, SyncPoint, decode_start, frame_at};
pub use version::AUDIO_VERSION;
pub use version::RENDER_VERSION;

use fin_core::CoreError;
use serde_json::json;

/// `MEDIA_CORRUPT` (spec §17): a structurally broken file or sample. `at` is a byte offset, a
/// frame index or a box path, whichever locates the fault.
pub fn media_corrupt(message: impl Into<String>, at: serde_json::Value) -> CoreError {
    CoreError::new("MEDIA_CORRUPT", message).with_details(json!({ "at": at }))
}
