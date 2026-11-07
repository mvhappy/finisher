//! Frame-id patterns (spec §13.3): uncompressed round trips, limited-range conversion and
//! noise, CRC failures, determinism, and reading from a composited RGBA frame. The lossy
//! check (AV1 at CRF 40 and 50) is `fin-codecs/tests/pattern_lossy.rs`.

use fin_media::testing::{Rect, crc8, pattern, read_id_rgba, read_id_y};

const SIZES: [(u32, u32); 3] = [(320, 180), (960, 540), (1920, 1080)];

fn indices() -> impl Iterator<Item = u32> {
    (0..=10_000)
        .step_by(73)
        .chain([1, 2, 255, 256, 9_999, 10_000, (1 << 24) - 1])
}

#[test]
fn uncompressed_round_trip() {
    for (w, h) in SIZES {
        for i in indices() {
            let p = pattern(i, w, h, 7);
            assert_eq!(p.y.len(), (w * h) as usize);
            assert_eq!(p.u.len(), (w.div_ceil(2) * h.div_ceil(2)) as usize);
            assert_eq!(read_id_y(&p.y, w, h), Some(i), "index {i} at {w}×{h}");
        }
    }
}

/// A deterministic ±`amp` noise source.
struct Noise(u64);

impl Noise {
    fn next(&mut self, amp: i32) -> i32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % (2 * amp as u64 + 1)) as i32 - amp
    }
}

#[test]
fn survives_range_expansion_and_noise() {
    let mut noise = Noise(0x1234_5678_9abc_def1);
    for (w, h) in SIZES {
        for i in indices().step_by(5) {
            let p = pattern(i, w, h, 3);
            // Limited → full: (y − 16) · 255 / 219, then ±3 code values of noise.
            let full: Vec<u8> =
                p.y.iter()
                    .map(|&y| {
                        let e = ((y as i32 - 16) * 255 + 109) / 219;
                        (e + noise.next(3)).clamp(0, 255) as u8
                    })
                    .collect();
            assert_eq!(
                read_id_y(&full, w, h),
                Some(i),
                "full range, index {i} at {w}×{h}"
            );
            // Full → limited compression of the full-range plane.
            let limited: Vec<u8> = full
                .iter()
                .map(|&y| ((y as i32 * 219 + 127) / 255 + 16 + noise.next(3)).clamp(0, 255) as u8)
                .collect();
            assert_eq!(
                read_id_y(&limited, w, h),
                Some(i),
                "limited, index {i} at {w}×{h}"
            );
        }
    }
}

#[test]
fn a_corrupted_block_fails_the_crc() {
    let (w, h) = (960, 540);
    let mut p = pattern(4_242, w, h, 1);
    assert_eq!(read_id_y(&p.y, w, h), Some(4_242));
    // Invert block (row 1, column 3): pixels 16·(3+1)…16·(3+2) × 16·(1+1)…16·(1+2).
    for row in 32..48 {
        for col in 64..80 {
            let y = &mut p.y[row * w as usize + col];
            *y = if *y > 128 { 16 } else { 235 };
        }
    }
    assert_eq!(read_id_y(&p.y, w, h), None);
    // A plane too small for its size is unreadable.
    assert_eq!(read_id_y(&p.y[..100], w, h), None);
    // CRC-8/SMBUS check value.
    assert_eq!(crc8(b"123456789"), 0xF4);
}

#[test]
fn deterministic_for_a_seed_and_varied_by_index() {
    let a = pattern(17, 960, 540, 99);
    assert_eq!(a, pattern(17, 960, 540, 99));
    let other_seed = pattern(17, 960, 540, 100);
    assert_ne!(a, other_seed);
    assert_eq!(read_id_y(&other_seed.y, 960, 540), Some(17));
    let next = pattern(18, 960, 540, 99);
    let changed = a.y.iter().zip(&next.y).filter(|(x, y)| x != y).count();
    assert!(
        changed > (960 * 540) / 10,
        "content varies with the index ({changed} pixels)"
    );
}

/// The Y plane as grey RGBA placed at (`ox`, `oy`) in a `cw × ch` black canvas, downscaled
/// by an integer `k` (box filter).
#[allow(clippy::too_many_arguments)]
fn composite(y: &[u8], w: u32, h: u32, k: u32, ox: u32, oy: u32, cw: u32, ch: u32) -> Vec<u8> {
    let mut out = vec![0u8; (cw * ch * 4) as usize];
    for py in 0..h / k {
        for px in 0..w / k {
            let mut sum = 0u32;
            for dy in 0..k {
                for dx in 0..k {
                    sum += y[((py * k + dy) * w + px * k + dx) as usize] as u32;
                }
            }
            let g = ((sum / (k * k)) as i32 - 16).max(0) * 255 / 219;
            let i = (((oy + py) * cw + ox + px) * 4) as usize;
            out[i..i + 4].copy_from_slice(&[
                g.min(255) as u8,
                g.min(255) as u8,
                g.min(255) as u8,
                255,
            ]);
        }
    }
    out
}

#[test]
fn reads_from_a_composited_rgba_frame() {
    let (w, h) = (960, 540);
    let p = pattern(777, w, h, 5);
    let canvas = composite(&p.y, w, h, 1, 100, 300, 1920, 1080);
    let at = Rect {
        x: 100.0,
        y: 300.0,
        width: 960.0,
        height: 540.0,
    };
    assert_eq!(read_id_rgba(&canvas, 1920, 1080, at), Some(777));
    // At half scale.
    let half = composite(&p.y, w, h, 2, 40, 20, 640, 360);
    let at = Rect {
        x: 40.0,
        y: 20.0,
        width: 480.0,
        height: 270.0,
    };
    assert_eq!(read_id_rgba(&half, 640, 360, at), Some(777));
    // A rectangle that puts the code outside the image.
    let off = Rect {
        x: -500.0,
        y: 0.0,
        width: 960.0,
        height: 540.0,
    };
    assert_eq!(read_id_rgba(&canvas, 1920, 1080, off), None);
}
