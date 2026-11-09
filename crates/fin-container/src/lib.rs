//! Containers (sub-project 6 spec §6.1, §18): sample tables, edit lists, fragments, limits and
//! codec configurations over `mp4-atom`'s box layer, and the MP4 muxer. Pure: no I/O beyond
//! the `Read + Seek` / `Write + Seek` it is given; builds for `wasm32-unknown-unknown`.

pub mod demux;
pub mod error;
mod limits;
pub mod mux;

pub use demux::{AudioTrack, CodecParams, Demuxer, SampleInfo, VideoTrack};
pub use limits::Limits;
pub use mux::{
    AudioTrackConfig, Brands, CodecConfig, Mp4Options, Mp4Writer, TrackRef, VideoTrackConfig,
};
