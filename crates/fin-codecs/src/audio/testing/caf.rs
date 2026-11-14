//! CAF with `lpcm`: 16-bit big-endian integers or 32-bit little-endian floats.

use super::int_be;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CafFormat {
    I16Be,
    F32Le,
}

/// A CAF file of `pcm` (interleaved): `desc` and a sized `data` chunk.
pub fn caf(pcm: &[f32], channels: u16, rate: u32, format: CafFormat) -> Vec<u8> {
    let (flags, bits): (u32, u32) = match format {
        CafFormat::I16Be => (0, 16),
        // kCAFLinearPCMFormatFlagIsFloat | kCAFLinearPCMFormatFlagIsLittleEndian
        CafFormat::F32Le => (1 | 2, 32),
    };
    let mut desc = Vec::new();
    desc.extend_from_slice(&(rate as f64).to_be_bytes());
    desc.extend_from_slice(b"lpcm");
    desc.extend_from_slice(&flags.to_be_bytes());
    desc.extend_from_slice(&(channels as u32 * bits / 8).to_be_bytes());
    desc.extend_from_slice(&1u32.to_be_bytes());
    desc.extend_from_slice(&(channels as u32).to_be_bytes());
    desc.extend_from_slice(&bits.to_be_bytes());
    let mut data = vec![0u8; 4]; // mEditCount
    for &x in pcm {
        match format {
            CafFormat::I16Be => int_be(x, 16, &mut data),
            CafFormat::F32Le => data.extend_from_slice(&x.to_le_bytes()),
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"caff");
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    chunk(&mut out, b"desc", &desc);
    chunk(&mut out, b"data", &data);
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as i64).to_be_bytes());
    out.extend_from_slice(body);
}
