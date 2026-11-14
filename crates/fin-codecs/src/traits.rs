//! The codec traits every backend implements (spec §6.1, §13.5).

use fin_core::CoreError;

use crate::frame::{DecodedFrame, EncodedPacket, RawFrame};
use fin_container::CodecConfig;

/// Which backend did the work (`dav1d`, `svt-av1`, later `videotoolbox`, `mf`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BackendId(pub &'static str);

pub trait VideoDecoder: Send {
    /// One packet (an AV1 temporal unit) with the pts to give its frame.
    fn send(&mut self, data: &[u8], pts: i64) -> Result<(), CoreError>;
    /// The next decoded frame in presentation order, if one is ready. A decoder may hold
    /// several frames in flight: `None` asks for the next packet, until `finish`.
    fn receive(&mut self) -> Result<Option<DecodedFrame>, CoreError>;
    /// Ends the stream (until the next `flush` or `send`): `receive` then gives every frame
    /// still buffered, waiting for those in flight, and `None` once none is left.
    fn finish(&mut self) -> Result<(), CoreError>;
    /// Drops every buffered packet and frame (a seek).
    fn flush(&mut self) -> Result<(), CoreError>;
    fn backend(&self) -> BackendId;
}

pub trait VideoEncoder: Send {
    fn send(&mut self, frame: &RawFrame, pts: i64, force_key: bool) -> Result<(), CoreError>;
    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError>;
    /// Ends the stream: every remaining packet becomes available from `receive`.
    fn finish(&mut self) -> Result<(), CoreError>;
    /// The configuration record for the container (`av1C`).
    fn config(&self) -> CodecConfig;
}
