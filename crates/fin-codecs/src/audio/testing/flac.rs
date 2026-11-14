//! FLAC with verbatim subframes only: a `STREAMINFO` block, then fixed-size frames whose
//! channels are stored independently and uncompressed (bit-exact, and simple enough to write
//! here without an encoder).

use super::to_int;

/// An MSB-first bit writer.
#[derive(Default)]
pub(crate) struct Bits {
    pub(crate) bytes: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    pub(crate) fn put(&mut self, value: u64, bits: u32) {
        for i in (0..bits).rev() {
            self.acc = (self.acc << 1) | ((value >> i) & 1);
            self.n += 1;
            if self.n == 8 {
                self.bytes.push(self.acc as u8);
                self.acc = 0;
                self.n = 0;
            }
        }
    }

    /// Pads with zero bits to the next byte.
    pub(crate) fn align(&mut self) {
        while self.n != 0 {
            self.put(0, 1);
        }
    }
}

/// CRC-8, polynomial 0x07 (frame headers).
pub(crate) fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// CRC-16, polynomial 0x8005 (whole frames).
pub(crate) fn crc16(data: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
        }
    }
    crc
}

/// The frame number in FLAC's UTF-8-like coding (up to 2^21 frames).
fn utf8(v: u32, out: &mut Vec<u8>) {
    match v {
        0..0x80 => out.push(v as u8),
        0x80..0x800 => out.extend_from_slice(&[0xC0 | (v >> 6) as u8, 0x80 | (v & 0x3F) as u8]),
        0x800..0x10000 => out.extend_from_slice(&[
            0xE0 | (v >> 12) as u8,
            0x80 | ((v >> 6) & 0x3F) as u8,
            0x80 | (v & 0x3F) as u8,
        ]),
        _ => out.extend_from_slice(&[
            0xF0 | (v >> 18) as u8,
            0x80 | ((v >> 12) & 0x3F) as u8,
            0x80 | ((v >> 6) & 0x3F) as u8,
            0x80 | (v & 0x3F) as u8,
        ]),
    }
}

/// The `STREAMINFO` block body (34 bytes).
pub fn streaminfo(channels: u16, rate: u32, bits: u32, block: usize, frames: u64) -> Vec<u8> {
    let mut b = Bits::default();
    b.put(block as u64, 16);
    b.put(block as u64, 16);
    b.put(0, 24);
    b.put(0, 24);
    b.put(rate as u64, 20);
    b.put(channels as u64 - 1, 3);
    b.put(bits as u64 - 1, 5);
    b.put(frames, 36);
    b.put(0, 64);
    b.put(0, 64);
    b.bytes
}

/// The frames of `pcm` (interleaved), `block` frames each (the last one short).
pub fn frames(pcm: &[f32], channels: u16, bits: u32, block: usize) -> Vec<Vec<u8>> {
    let ch = channels as usize;
    let size_code = match bits {
        16 => 0b100,
        24 => 0b110,
        _ => panic!("16 or 24 bits"),
    };
    pcm.chunks(block * ch)
        .enumerate()
        .map(|(n, chunk)| {
            let len = chunk.len() / ch;
            let mut h = vec![0xFF, 0xF8];
            // Block size from the 16-bit field at the header's end; rate from STREAMINFO.
            h.push(0b0111_0000);
            h.push((((ch - 1) as u8) << 4) | (size_code << 1));
            utf8(n as u32, &mut h);
            h.extend_from_slice(&((len - 1) as u16).to_be_bytes());
            h.push(crc8(&h));
            let mut b = Bits::default();
            for c in 0..ch {
                b.put(0b0000_0010, 8); // verbatim, no wasted bits
                for f in 0..len {
                    let v = to_int(chunk[f * ch + c], bits) as i64 as u64;
                    b.put(v & ((1u64 << bits) - 1), bits);
                }
            }
            b.align();
            let mut frame = h;
            frame.extend_from_slice(&b.bytes);
            let crc = crc16(&frame);
            frame.extend_from_slice(&crc.to_be_bytes());
            frame
        })
        .collect()
}

/// A native FLAC file of `pcm` (interleaved) at 16 or 24 bits, `block` frames a FLAC frame.
pub fn flac(pcm: &[f32], channels: u16, rate: u32, bits: u32, block: usize) -> Vec<u8> {
    let total = (pcm.len() / channels as usize) as u64;
    let mut out = b"fLaC".to_vec();
    out.push(0x80); // last metadata block, STREAMINFO
    out.extend_from_slice(&34u32.to_be_bytes()[1..]);
    out.extend_from_slice(&streaminfo(channels, rate, bits, block, total));
    for f in frames(pcm, channels, bits, block) {
        out.extend_from_slice(&f);
    }
    out
}
