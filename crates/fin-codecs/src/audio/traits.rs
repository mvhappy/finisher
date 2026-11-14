//! The audio codec traits (6C plan Frozen interfaces, Decisions 3 and 22). A decoder hands out
//! interleaved `f32` at the source's own rate with the channel layout in its own order; nothing
//! here resamples, downmixes or reorders (`fin-codecs` never links `fin-audio`).

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::AudioCodec;
use fin_media::audio_layout::ChannelLayout;
use symphonia::core::codecs::audio::AudioCodecParameters;

use crate::frame::EncodedPacket;
use crate::traits::BackendId;

/// Decoded frames: interleaved `f32`, `layout.channels()` a frame, at `rate`.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    pub interleaved: Vec<f32>,
    pub layout: ChannelLayout,
    pub rate: u32,
    /// The first frame's position: the packet's pts from a decoder; from `AudioReader`, the
    /// index of the first frame counted from the first audible sample.
    pub pts: i64,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        match self.layout.channels() {
            0 => 0,
            n => self.interleaved.len() / n,
        }
    }
}

/// What a decoder is opened from: a container record (`fin-container`'s MP4 sample entry), or
/// the codec parameters one of symphonia's format readers found.
#[derive(Debug, Clone)]
pub enum AudioConfig {
    Record(CodecConfig),
    Symphonia(SymphoniaParams),
}

/// symphonia's codec parameters for a track, opaque outside this crate.
#[derive(Debug, Clone)]
pub struct SymphoniaParams(pub(crate) AudioCodecParameters);

pub trait AudioDecoder: Send {
    /// One packet with the pts to give its frames.
    fn send(&mut self, packet: &[u8], pts: i64) -> Result<(), CoreError>;
    /// The next decoded frames, if any are ready; `None` asks for the next packet.
    fn receive(&mut self) -> Result<Option<DecodedAudio>, CoreError>;
    /// Ends the stream: `receive` then gives whatever is still buffered.
    fn finish(&mut self) -> Result<(), CoreError>;
    /// Drops every buffered packet and frame (a seek).
    fn flush(&mut self) -> Result<(), CoreError>;
    fn backend(&self) -> BackendId;
}

/// Decision 22: the decoded total is `priming + valid + padding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedLength {
    pub priming: u32,
    /// The samples sent.
    pub valid: u64,
    pub padding: u32,
}

/// An audio encode: the codec, the input's rate and channels (interleaved `f32`), and the
/// bitrate in bits a second.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioEncoderConfig {
    pub codec: AudioCodec,
    pub rate: u32,
    pub channels: u16,
    pub bitrate: u32,
}

pub trait AudioEncoder: Send {
    fn send(&mut self, pcm: &[f32], pts: i64) -> Result<(), CoreError>;
    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError>;
    /// Ends the stream: every remaining packet becomes available from `receive`.
    fn finish(&mut self) -> Result<EncodedLength, CoreError>;
    /// The configuration record for the container (`esds`, `dOps`).
    fn config(&self) -> CodecConfig;
    /// Samples of encoder delay at the start of the decoded stream.
    fn priming(&self) -> u32;
    /// Samples a packet carries.
    fn frame_size(&self) -> u32;
}
