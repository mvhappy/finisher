//! RIFF/WAVE: 16- and 24-bit integer PCM and 32-bit float, as `WAVEFORMATEX` or, with a channel
//! mask, `WAVEFORMATEXTENSIBLE`.

use super::int_le;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavFormat {
    Pcm16,
    Pcm24,
    F32,
}

impl WavFormat {
    pub fn bits(self) -> u32 {
        match self {
            WavFormat::Pcm16 => 16,
            WavFormat::Pcm24 => 24,
            WavFormat::F32 => 32,
        }
    }
}

/// The KSDATAFORMAT subtype GUID tail shared by PCM (1) and IEEE float (3).
const GUID_TAIL: [u8; 14] = [
    0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xaa, 0x00, 0x38, 0x9b, 0x71,
];

/// A WAVE file of `pcm` (interleaved, `channels` a frame). `mask` writes
/// `WAVEFORMATEXTENSIBLE` with that `dwChannelMask` (0 included); `None` the plain header.
pub fn wav(pcm: &[f32], channels: u16, rate: u32, format: WavFormat, mask: Option<u32>) -> Vec<u8> {
    let bits = format.bits();
    let tag: u16 = if format == WavFormat::F32 { 3 } else { 1 };
    let block = channels as u32 * bits / 8;
    let mut fmt = Vec::new();
    fmt.extend_from_slice(&(if mask.is_some() { 0xFFFE } else { tag }).to_le_bytes());
    fmt.extend_from_slice(&channels.to_le_bytes());
    fmt.extend_from_slice(&rate.to_le_bytes());
    fmt.extend_from_slice(&(rate * block).to_le_bytes());
    fmt.extend_from_slice(&(block as u16).to_le_bytes());
    fmt.extend_from_slice(&(bits as u16).to_le_bytes());
    if let Some(mask) = mask {
        fmt.extend_from_slice(&22u16.to_le_bytes());
        fmt.extend_from_slice(&(bits as u16).to_le_bytes());
        fmt.extend_from_slice(&mask.to_le_bytes());
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&GUID_TAIL);
    }
    let mut data = Vec::with_capacity(pcm.len() * bits as usize / 8);
    for &x in pcm {
        match format {
            WavFormat::F32 => data.extend_from_slice(&x.to_le_bytes()),
            _ => int_le(x, bits, &mut data),
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    let riff = 4 + 8 + fmt.len() + 8 + data.len() + data.len() % 2;
    out.extend_from_slice(&(riff as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    chunk(&mut out, b"fmt ", &fmt);
    chunk(&mut out, b"data", &data);
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}
