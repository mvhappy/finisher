//! The pattern generator.

use super::{COLS, MAX_INDEX, ROWS, block_rect, code_bits, edge};

/// An 8-bit 4:2:0 picture, limited range, planes without padding (`u` and `v` are
/// `⌈width/2⌉ × ⌈height/2⌉`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420 {
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

const BLACK: u8 = 16;
const WHITE: u8 = 235;
const GREY: u8 = 126;

/// A picture whose content varies with `index` (a diagonal gradient that drifts, eight
/// colour bars whose order depends on `seed`, a bright vertical edge that moves 7 pixels a
/// frame) and carries `index` and a CRC-8 as a 4×8 grid of 2-level blocks in the top-left
/// corner (module docs). Integer arithmetic only: the same arguments give the same bytes on
/// every platform. Panics if `index` exceeds 24 bits or the picture is smaller than the code
/// (10 × 6 blocks).
pub fn pattern(index: u32, width: u32, height: u32, seed: u64) -> Yuv420 {
    assert!(index <= MAX_INDEX, "frame-id patterns carry 24-bit indices");
    let code_w = edge(COLS + 2, height);
    let code_h = edge(ROWS + 2, height);
    assert!(
        width >= code_w && height >= code_h && edge(1, height) >= 2,
        "a {width}×{height} picture is too small for the frame-id code"
    );
    let (w, h) = (width as usize, height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let mut y = vec![0u8; w * h];
    let mut u = vec![128u8; cw * ch];
    let mut v = vec![128u8; cw * ch];

    let r = splitmix(seed);
    let shift = (index as u64 * 3 + (r & 0xff)) as usize;
    let edge_x = ((index as u64 * 7 + (r >> 8) % width as u64) % width as u64) as usize;
    for row in 0..h {
        for col in 0..w {
            // Diagonal gradient over 16…235, drifting with the index.
            let g = ((col + row + shift) % (w + h)) * 219 / (w + h);
            let mut luma = 16 + g as u32;
            if col.abs_diff(edge_x) < 2 {
                luma = 235;
            }
            y[row * w + col] = luma as u8;
        }
    }
    // Eight colour bars in the lower third of the chroma planes, ordered by the seed.
    let bars: [(u8, u8); 8] = [
        (90, 240),
        (54, 34),
        (240, 110),
        (128, 128),
        (166, 16),
        (202, 222),
        (16, 146),
        (110, 200),
    ];
    let offset = (r >> 16) as usize % 8;
    for row in (ch * 2 / 3)..ch {
        for col in 0..cw {
            let (bu, bv) = bars[(col * 8 / cw + offset + index as usize / 30) % 8];
            u[row * cw + col] = bu;
            v[row * cw + col] = bv;
        }
    }

    // The quiet zone and the code, with neutral chroma.
    for row in 0..code_h as usize {
        for col in 0..code_w as usize {
            y[row * w + col] = GREY;
        }
    }
    for row in 0..(code_h as usize).div_ceil(2) {
        for col in 0..(code_w as usize).div_ceil(2) {
            u[row * cw + col] = 128;
            v[row * cw + col] = 128;
        }
    }
    let bits = code_bits(index);
    for b in 0..ROWS * COLS {
        let (r0, c0) = (b / COLS, b % COLS);
        let on = bits & (1 << (31 - b)) != 0;
        let (x0, x1, y0, y1) = block_rect(r0, c0, height);
        for row in y0..y1 {
            for col in x0..x1 {
                y[row as usize * w + col as usize] = if on { WHITE } else { BLACK };
            }
        }
    }
    Yuv420 {
        width,
        height,
        y,
        u,
        v,
    }
}
