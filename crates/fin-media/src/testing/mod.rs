//! Frame-id test patterns (spec §13.3): pictures whose content varies with their frame index
//! and that carry the index and a CRC-8 as a block code in the top-left corner, so decoders,
//! the sampling rule, edit lists and VFR are checked by reading ids back from decoded or
//! composited frames. Behind the `testing` feature; never in a shipped build.
//!
//! **The code:** 32 blocks in 4 rows of 8, row-major, most significant bit first: the index's
//! low 24 bits, then CRC-8 (polynomial 0x07, initial 0) of those three bytes, big-endian. A 1
//! is white (Y 235), a 0 black (Y 16), in a mid-grey quiet zone one block wide. The block size
//! is `16 · height / 540` pixels (16 at 540p, 32 at 1080p; edges at integer pixels), so the
//! code scales with the picture and keeps its place relative to the picture's height.

mod pattern;
mod read;

pub use pattern::{Yuv420, pattern};
pub use read::{Rect, read_id_rgba, read_id_y};

/// Grid rows and columns.
pub const ROWS: u32 = 4;
pub const COLS: u32 = 8;
/// Block pixels at 540 lines; the block size scales with the height.
pub const BLOCK_AT_540: u32 = 16;
/// Indices are 24 bits.
pub const MAX_INDEX: u32 = (1 << 24) - 1;

/// CRC-8 (polynomial x⁸ + x² + x + 1, initial value 0, no reflection) of `bytes`.
pub fn crc8(bytes: &[u8]) -> u8 {
    let mut crc = 0u8;
    for &b in bytes {
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

/// The 32 code bits of `index`, most significant first.
pub(crate) fn code_bits(index: u32) -> u32 {
    let i = index & MAX_INDEX;
    let bytes = [(i >> 16) as u8, (i >> 8) as u8, i as u8];
    (i << 8) | crc8(&bytes) as u32
}

/// The index carried by 32 code bits, if the CRC matches.
pub(crate) fn decode_bits(bits: u32) -> Option<u32> {
    let i = bits >> 8;
    (code_bits(i) == bits).then_some(i)
}

/// The pixel edge `k` blocks from the picture's top or left in a picture `height` lines high:
/// `⌊k · 16 · height / 540⌋`.
pub(crate) fn edge(k: u32, height: u32) -> u32 {
    (k as u64 * BLOCK_AT_540 as u64 * height as u64 / 540) as u32
}

/// Block `(row, col)`'s pixel span `[x0, x1) × [y0, y1)` (the quiet zone is block 0).
pub(crate) fn block_rect(row: u32, col: u32, height: u32) -> (u32, u32, u32, u32) {
    (
        edge(col + 1, height),
        edge(col + 2, height),
        edge(row + 1, height),
        edge(row + 2, height),
    )
}
