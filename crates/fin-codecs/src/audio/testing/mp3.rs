//! Silent MPEG-1 Layer III frames (48 kHz, 128 kb/s, 1,152 samples each): a header, zeroed side
//! information (every granule's `part2_3_length` 0) and zeroed main data, which decode to
//! silence. Nothing in the tree encodes MP3, so tests that need real MPEG audio packets (MP3 in
//! MP4, the Xing frame count) build them here; real MP3 audio comes from the fixture cache.

/// Bytes in one frame: 144 × 128,000 / 48,000.
pub const FRAME_BYTES: usize = 384;
pub const FRAME_SAMPLES: u64 = 1152;

fn side_info(channels: u16) -> usize {
    if channels == 1 { 17 } else { 32 }
}

/// One silent frame, mono or stereo.
pub fn frame(channels: u16) -> Vec<u8> {
    let mode = if channels == 1 { 0xC0 } else { 0x00 };
    let mut f = vec![0xFF, 0xFB, 0x94, mode];
    f.resize(FRAME_BYTES, 0);
    f
}

/// A Xing tag frame declaring `frames` audio frames and `bytes` bytes (no LAME extension).
pub fn xing(channels: u16, frames: u32, bytes: u32) -> Vec<u8> {
    let mut f = frame(channels);
    let at = 4 + side_info(channels);
    f[at..at + 4].copy_from_slice(b"Xing");
    f[at + 4..at + 8].copy_from_slice(&3u32.to_be_bytes());
    f[at + 8..at + 12].copy_from_slice(&frames.to_be_bytes());
    f[at + 12..at + 16].copy_from_slice(&bytes.to_be_bytes());
    f
}

/// An MP3 file of `frames` silent frames, led by a Xing frame that states the count when `xing`.
pub fn mp3(channels: u16, frames: u32, xing_tag: bool) -> Vec<u8> {
    let mut out = Vec::new();
    if xing_tag {
        let bytes = (frames as usize + 1) * FRAME_BYTES;
        out.extend_from_slice(&xing(channels, frames, bytes as u32));
    }
    for _ in 0..frames {
        out.extend_from_slice(&frame(channels));
    }
    out
}
