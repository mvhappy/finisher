//! AIFF: big-endian integer PCM (`COMM` + `SSND`).

use super::int_be;

/// An AIFF file of `pcm` (interleaved) at 16 or 24 bits.
pub fn aiff(pcm: &[f32], channels: u16, rate: u32, bits: u16) -> Vec<u8> {
    let frames = pcm.len() / channels as usize;
    let mut comm = Vec::new();
    comm.extend_from_slice(&channels.to_be_bytes());
    comm.extend_from_slice(&(frames as u32).to_be_bytes());
    comm.extend_from_slice(&bits.to_be_bytes());
    comm.extend_from_slice(&extended(rate));
    let mut ssnd = vec![0u8; 8];
    for &x in pcm {
        int_be(x, bits as u32, &mut ssnd);
    }
    let mut body = Vec::new();
    body.extend_from_slice(b"AIFF");
    chunk(&mut body, b"COMM", &comm);
    chunk(&mut body, b"SSND", &ssnd);
    let mut out = Vec::new();
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// `rate` as an IEEE 754 80-bit extended float (big-endian), as `COMM` stores it.
fn extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let e = 31 - rate.leading_zeros();
    let exp = (16383 + e) as u16;
    let mantissa = (rate as u64) << (63 - e);
    out[..2].copy_from_slice(&exp.to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    if body.len() % 2 == 1 {
        out.push(0);
    }
}
