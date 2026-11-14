//! Reading the code back.

use super::{BLOCK_AT_540, COLS, ROWS, block_rect, decode_bits};

/// A rectangle in output pixels (origin top-left), where a source landed in a composited
/// frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// The mean of `get(x, y)` over the central half of a block, thresholded at mid-grey.
fn bit(x0: u32, x1: u32, y0: u32, y1: u32, get: impl Fn(usize, usize) -> u32) -> bool {
    let (qx, qy) = ((x1 - x0) / 4, (y1 - y0) / 4);
    let (mut sum, mut n) = (0u64, 0u64);
    for row in (y0 + qy)..(y1 - qy).max(y0 + qy + 1) {
        for col in (x0 + qx)..(x1 - qx).max(x0 + qx + 1) {
            sum += get(col as usize, row as usize) as u64;
            n += 1;
        }
    }
    sum >= 128 * n
}

/// Reads the code from a Y plane (`width` bytes a row, at least `width × height`); `None` if
/// the plane is too small or the CRC does not match. The block centres are thresholded at
/// mid-grey (128), so limited and full range both read.
pub fn read_id_y(y: &[u8], width: u32, height: u32) -> Option<u32> {
    if y.len() < width as usize * height as usize {
        return None;
    }
    let (_, x1, _, y1) = block_rect(ROWS - 1, COLS - 1, height);
    if x1 > width || y1 > height {
        return None;
    }
    let mut bits = 0u32;
    for b in 0..ROWS * COLS {
        let (x0, x1, y0, y1) = block_rect(b / COLS, b % COLS, height);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        if bit(x0, x1, y0, y1, |c, r| y[r * width as usize + c] as u32) {
            bits |= 1 << (31 - b);
        }
    }
    decode_bits(bits)
}

/// Reads the code from an RGBA image (`width × height`, 4 bytes a pixel) whose source landed
/// at `at` (unrotated, square pixels): block centres are mapped by `at.height` (the code's
/// geometry scales with the source height) from `at`'s corner and read as BT.709 luma over a
/// window a quarter of a block wide. `None` if a block falls outside the image or the CRC
/// does not match.
pub fn read_id_rgba(rgba: &[u8], width: u32, height: u32, at: Rect) -> Option<u32> {
    if rgba.len() < width as usize * height as usize * 4 || at.height.is_nan() || at.height <= 0.0 {
        return None;
    }
    let block = at.height * BLOCK_AT_540 as f64 / 540.0;
    let radius = (block / 4.0).floor() as i64;
    let luma = |x: i64, y: i64| -> Option<u64> {
        if x < 0 || y < 0 || x >= width as i64 || y >= height as i64 {
            return None;
        }
        let i = (y as usize * width as usize + x as usize) * 4;
        let (r, g, b) = (rgba[i] as u64, rgba[i + 1] as u64, rgba[i + 2] as u64);
        Some((2126 * r + 7152 * g + 722 * b + 5000) / 10_000)
    };
    let mut bits = 0u32;
    for b in 0..ROWS * COLS {
        let (row, col) = ((b / COLS) as f64, (b % COLS) as f64);
        let cx = (at.x + block * (col + 1.5)).floor() as i64;
        let cy = (at.y + block * (row + 1.5)).floor() as i64;
        let (mut sum, mut n) = (0u64, 0u64);
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                sum += luma(cx + dx, cy + dy)?;
                n += 1;
            }
        }
        if sum >= 128 * n {
            bits |= 1 << (31 - b);
        }
    }
    decode_bits(bits)
}
