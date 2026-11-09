//! Fake decoders and encoders (plan 6B-1 Task 3): correct, hanging, stalling, failing and
//! corrupt, under the names and classes of `fin_media::backend::BACKENDS`.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use fin_container::CodecConfig;
use fin_core::CoreError;
use fin_media::backend::BackendEntry;
use fin_media::jobs::Profile;
use fin_media::{ColourTags, PixelFormat, Size, VideoCodec};
use serde_json::json;

use crate::config::{DecoderConfig, EncoderConfig, StreamDesc};
use crate::frame::{DecodedFrame, EncodedPacket, Plane, RawFrame};
use crate::registry::{DecoderBackend, EncoderBackend, OpenToken, entry};
use crate::traits::{BackendId, VideoDecoder, VideoEncoder};

/// Blocks the calling thread for good (a stalled hardware session).
fn block_forever() -> ! {
    loop {
        std::thread::sleep(Duration::from_secs(3600));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeBehaviour {
    Correct,
    /// `open` fails.
    FailOpen,
    /// Every packet is `DECODE_FAILED`.
    Corrupt,
}

/// A decoder backend named after a `BACKENDS` entry.
pub struct FakeDecoderBackend {
    pub name: &'static str,
    pub hw: bool,
    pub codecs: Vec<VideoCodec>,
    /// The deepest bit depth it decodes.
    pub max_depth: u8,
    pub behaviour: DecodeBehaviour,
    /// Sessions opened.
    pub opened: AtomicU32,
}

impl FakeDecoderBackend {
    pub fn new(name: &'static str, hw: bool, codecs: &[VideoCodec]) -> FakeDecoderBackend {
        FakeDecoderBackend {
            name,
            hw,
            codecs: codecs.to_vec(),
            max_depth: 10,
            behaviour: DecodeBehaviour::Correct,
            opened: AtomicU32::new(0),
        }
    }

    pub fn behaving(mut self, b: DecodeBehaviour) -> Self {
        self.behaviour = b;
        self
    }

    pub fn max_depth(mut self, d: u8) -> Self {
        self.max_depth = d;
        self
    }

    pub fn arc(self) -> Arc<FakeDecoderBackend> {
        Arc::new(self)
    }
}

impl DecoderBackend for FakeDecoderBackend {
    fn entry(&self) -> &'static BackendEntry {
        entry(self.name)
    }

    fn hw(&self) -> bool {
        self.hw
    }

    fn decodes(&self, s: &StreamDesc) -> bool {
        self.codecs.contains(&s.codec) && s.bit_depth <= self.max_depth
    }

    fn open(&self, _cfg: &DecoderConfig) -> Result<Box<dyn VideoDecoder>, CoreError> {
        if self.behaviour == DecodeBehaviour::FailOpen {
            return Err(
                CoreError::new("DECODE_FAILED", "the fake decoder did not open")
                    .with_details(json!({ "backend": self.name })),
            );
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeDecoder {
            name: self.name,
            hw: self.hw,
            corrupt: self.behaviour == DecodeBehaviour::Corrupt,
            queue: VecDeque::new(),
        }))
    }
}

struct FakeDecoder {
    name: &'static str,
    hw: bool,
    corrupt: bool,
    queue: VecDeque<i64>,
}

impl VideoDecoder for FakeDecoder {
    fn send(&mut self, _data: &[u8], pts: i64) -> Result<(), CoreError> {
        if self.corrupt {
            return Err(CoreError::new("DECODE_FAILED", "corrupt sample")
                .with_details(json!({ "backend": self.name })));
        }
        self.queue.push_back(pts);
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<DecodedFrame>, CoreError> {
        Ok(self.queue.pop_front().map(|pts| {
            let plane = |w: u32, h: u32| Plane {
                data: vec![128; (w * h) as usize],
                stride: w as usize,
                width: w,
                height: h,
            };
            DecodedFrame {
                pts,
                format: PixelFormat::I420,
                size: Size::new(16, 16),
                planes: [plane(16, 16), plane(8, 8), plane(8, 8)],
                colour: ColourTags::BT709,
                hw: self.hw,
            }
        }))
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        Ok(())
    }

    fn flush(&mut self) -> Result<(), CoreError> {
        self.queue.clear();
        Ok(())
    }

    fn backend(&self) -> BackendId {
        BackendId(self.name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeBehaviour {
    /// One packet per frame, at once.
    Correct,
    /// `send` of frame `n` (0-based) and later blocks for good.
    HangAfter(u32),
    /// Frames from `n` on are accepted but never produce a packet; `finish` blocks for good.
    StallAfter(u32),
    /// `open` fails.
    FailOpen,
    /// `open` blocks for good.
    HangOpen,
}

/// An encoder backend named after a `BACKENDS` entry. Its packets carry the frame's pts as
/// their bytes, so a test can check that every frame came out.
pub struct FakeEncoderBackend {
    pub name: &'static str,
    pub hw: bool,
    pub codecs: Vec<VideoCodec>,
    pub behaviour: EncodeBehaviour,
    /// Sessions opened.
    pub opened: AtomicU32,
}

impl FakeEncoderBackend {
    pub fn new(name: &'static str, hw: bool, codecs: &[VideoCodec]) -> FakeEncoderBackend {
        FakeEncoderBackend {
            name,
            hw,
            codecs: codecs.to_vec(),
            behaviour: EncodeBehaviour::Correct,
            opened: AtomicU32::new(0),
        }
    }

    pub fn behaving(mut self, b: EncodeBehaviour) -> Self {
        self.behaviour = b;
        self
    }

    pub fn arc(self) -> Arc<FakeEncoderBackend> {
        Arc::new(self)
    }
}

impl EncoderBackend for FakeEncoderBackend {
    fn entry(&self) -> &'static BackendEntry {
        entry(self.name)
    }

    fn hw(&self) -> bool {
        self.hw
    }

    fn encodes(&self, codec: VideoCodec, _profile: Option<Profile>) -> bool {
        self.codecs.contains(&codec)
    }

    fn open(&self, _cfg: &EncoderConfig, _: OpenToken) -> Result<Box<dyn VideoEncoder>, CoreError> {
        if self.behaviour == EncodeBehaviour::HangOpen {
            block_forever();
        }
        if self.behaviour == EncodeBehaviour::FailOpen {
            return Err(
                CoreError::new("ENCODE_FAILED", "the fake encoder did not open")
                    .with_details(json!({ "backend": self.name })),
            );
        }
        self.opened.fetch_add(1, Ordering::SeqCst);
        Ok(Box::new(FakeEncoder {
            behaviour: self.behaviour,
            sent: 0,
            out: VecDeque::new(),
        }))
    }
}

struct FakeEncoder {
    behaviour: EncodeBehaviour,
    sent: u32,
    out: VecDeque<EncodedPacket>,
}

impl VideoEncoder for FakeEncoder {
    fn send(&mut self, _frame: &RawFrame, pts: i64, _force_key: bool) -> Result<(), CoreError> {
        let n = self.sent;
        self.sent += 1;
        match self.behaviour {
            EncodeBehaviour::HangAfter(k) if n >= k => block_forever(),
            EncodeBehaviour::StallAfter(k) if n >= k => {}
            _ => self.out.push_back(EncodedPacket {
                data: pts.to_le_bytes().to_vec(),
                pts,
                dts: pts,
                keyframe: n == 0,
            }),
        }
        Ok(())
    }

    fn receive(&mut self) -> Result<Option<EncodedPacket>, CoreError> {
        Ok(self.out.pop_front())
    }

    fn finish(&mut self) -> Result<(), CoreError> {
        if matches!(self.behaviour, EncodeBehaviour::StallAfter(k) if self.sent > k) {
            block_forever();
        }
        Ok(())
    }

    fn config(&self) -> CodecConfig {
        CodecConfig::Other { fourcc: *b"fake" }
    }
}
