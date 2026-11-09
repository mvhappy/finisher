//! Containers (sub-project 6 spec §6.1, §18): sample tables, edit lists, fragments, limits and
//! codec configurations over `mp4-atom`'s box layer, the MP4 muxer, and the WebM/Matroska
//! demuxer (EBML, clusters, cues; 6B-1 Task 12). Pure: no I/O beyond
//! the `Read + Seek` / `Write + Seek` it is given; builds for `wasm32-unknown-unknown`.

pub mod demux;
pub mod error;
mod limits;
pub mod mux;
#[cfg(feature = "testing")]
pub mod testing;

pub use demux::{AudioTrack, CodecParams, Demuxer, SampleInfo, VideoTrack};
pub use limits::Limits;
pub use mux::{
    AudioTrackConfig, Brands, CodecConfig, Mp4Options, Mp4Writer, TrackRef, VideoTrackConfig,
};
