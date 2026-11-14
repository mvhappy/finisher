//! Small RGBA pictures for thumbnails and filmstrips (plan 6B-1 Task 15): standing a coded
//! picture upright and laying tiles side by side. Exact integer moves, no resampling.

use fin_media::{Rotation, Size};

/// `rgba` (`size`, 4 bytes a pixel, row-major) turned clockwise by `rotation`; the result is
/// `size` swapped for 90° and 270°.
pub fn rotate(rgba: &[u8], size: Size, rotation: Rotation) -> Vec<u8> {
    let (w, h) = (size.width as usize, size.height as usize);
    debug_assert_eq!(rgba.len(), w * h * 4);
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            // Where source pixel (x, y) lands, in a picture `ow` wide.
            let (dx, dy, ow) = match rotation {
                Rotation::R0 => (x, y, w),
                Rotation::R90 => (h - 1 - y, x, h),
                Rotation::R180 => (w - 1 - x, h - 1 - y, w),
                Rotation::R270 => (y, w - 1 - x, h),
            };
            let (s, d) = ((y * w + x) * 4, (dy * ow + dx) * 4);
            out[d..d + 4].copy_from_slice(&rgba[s..s + 4]);
        }
    }
    out
}

/// Copies the `tile` (`tile_size`) into `sprite` (`sprite_width` wide) at column `x`.
pub fn paste(sprite: &mut [u8], sprite_width: u32, x: u32, tile: &[u8], tile_size: Size) {
    let (sw, tw, th) = (
        sprite_width as usize * 4,
        tile_size.width as usize * 4,
        tile_size.height as usize,
    );
    for row in 0..th {
        let d = row * sw + x as usize * 4;
        sprite[d..d + tw].copy_from_slice(&tile[row * tw..(row + 1) * tw]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3 × 2 picture whose pixels are 0..6 (first byte).
    fn pic() -> Vec<u8> {
        (0u8..6).flat_map(|i| [i, 0, 0, 255]).collect()
    }

    fn firsts(p: &[u8]) -> Vec<u8> {
        p.chunks(4).map(|c| c[0]).collect()
    }

    #[test]
    fn rotation_is_clockwise() {
        // 0 1 2
        // 3 4 5
        let s = Size::new(3, 2);
        assert_eq!(firsts(&rotate(&pic(), s, Rotation::R0)), [0, 1, 2, 3, 4, 5]);
        // 3 0
        // 4 1
        // 5 2
        assert_eq!(
            firsts(&rotate(&pic(), s, Rotation::R90)),
            [3, 0, 4, 1, 5, 2]
        );
        assert_eq!(
            firsts(&rotate(&pic(), s, Rotation::R180)),
            [5, 4, 3, 2, 1, 0]
        );
        // 2 5
        // 1 4
        // 0 3
        assert_eq!(
            firsts(&rotate(&pic(), s, Rotation::R270)),
            [2, 5, 1, 4, 0, 3]
        );
    }

    #[test]
    fn four_quarter_turns_are_the_identity() {
        let mut p = pic();
        let mut s = Size::new(3, 2);
        for _ in 0..4 {
            p = rotate(&p, s, Rotation::R90);
            s = Size::new(s.height, s.width);
        }
        assert_eq!(p, pic());
    }

    #[test]
    fn tiles_land_side_by_side() {
        let tile = pic();
        let mut sprite = vec![0u8; 6 * 2 * 4];
        paste(&mut sprite, 6, 0, &tile, Size::new(3, 2));
        paste(&mut sprite, 6, 3, &tile, Size::new(3, 2));
        assert_eq!(firsts(&sprite), [0, 1, 2, 0, 1, 2, 3, 4, 5, 3, 4, 5]);
    }
}
