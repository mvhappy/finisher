//! Frames and packets.

use fin_media::{ColourTags, PixelFormat, Size};

/// One plane: `height` rows of `stride` bytes, the first `width` samples of each in use
/// (2 bytes a sample for 10-bit formats).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: u32,
    pub height: u32,
}

impl Plane {
    /// Row `y`'s samples in use.
    pub fn row(&self, y: u32, bytes_per_sample: usize) -> &[u8] {
        let at = y as usize * self.stride;
        &self.data[at..at + self.width as usize * bytes_per_sample]
    }
}

/// A decoded picture: `pts` as sent with its packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    pub pts: i64,
    pub format: PixelFormat,
    pub size: Size,
    pub planes: [Plane; 3],
    pub colour: ColourTags,
}

/// A picture for an encoder (8-bit 4:2:0 in 6A).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawFrame {
    pub format: PixelFormat,
    pub size: Size,
    pub planes: [Plane; 3],
}

impl RawFrame {
    /// Tight 8-bit 4:2:0 planes.
    pub fn i420(size: Size, y: Vec<u8>, u: Vec<u8>, v: Vec<u8>) -> RawFrame {
        let (cw, ch) = (size.width.div_ceil(2), size.height.div_ceil(2));
        RawFrame {
            format: PixelFormat::I420,
            size,
            planes: [
                Plane {
                    data: y,
                    stride: size.width as usize,
                    width: size.width,
                    height: size.height,
                },
                Plane {
                    data: u,
                    stride: cw as usize,
                    width: cw,
                    height: ch,
                },
                Plane {
                    data: v,
                    stride: cw as usize,
                    width: cw,
                    height: ch,
                },
            ],
        }
    }
}

/// One encoded access unit (an AV1 temporal unit).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub keyframe: bool,
}
