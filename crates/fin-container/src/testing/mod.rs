//! Test-only helpers (feature `testing`): a WebM/Matroska writer, so the WebM demuxer's tests
//! and the fuzz seeds generate their files (no media files are committed).

pub mod webm_writer;
