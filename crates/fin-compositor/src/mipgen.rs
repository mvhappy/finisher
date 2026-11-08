//! The mip chain on the GPU (spec §5.1): the reference's chain (exact 2×2 box averages, sizes
//! `⌈w/2⌉ × ⌈h/2⌉`, as many levels as `⌈L⌉` needs, stopping at 1×1), one texture per level so
//! the sizes match the reference's, built by `mip.wgsl`; and which two levels a layer reads.

/// Level sizes from the coded size down, as `reference::mipchain::chain` builds them.
pub(crate) fn chain_sizes(width: u32, height: u32, level: f64) -> Vec<(u32, u32)> {
    let need = level.ceil() as usize;
    let mut out = vec![(width, height)];
    while out.len() <= need {
        let (w, h) = *out.last().expect("level 0");
        if w == 1 && h == 1 {
            break;
        }
        out.push((w.div_ceil(2), h.div_ceil(2)));
    }
    out
}

/// The levels `⌊L⌋` and `⌈L⌉` (past the chain's end its last level) and the fraction that
/// mixes them (0 when they are the same level).
pub(crate) fn pick(levels: usize, level: f64) -> (usize, usize, f32) {
    let lo = (level.floor() as usize).min(levels - 1);
    let hi = (level.ceil() as usize).min(levels - 1);
    let t = if hi == lo {
        0.0
    } else {
        (level - level.floor()) as f32
    };
    (lo, hi, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_round_up_and_stop_at_one_pixel() {
        assert_eq!(chain_sizes(5, 3, 0.0), [(5, 3)]);
        assert_eq!(chain_sizes(5, 3, 1.2), [(5, 3), (3, 2), (2, 1)]);
        assert_eq!(chain_sizes(3840, 2160, 1.0), [(3840, 2160), (1920, 1080)]);
        assert_eq!(chain_sizes(2, 1, 9.0), [(2, 1), (1, 1)]);
    }

    #[test]
    fn levels_and_fraction() {
        assert_eq!(pick(3, 0.0), (0, 0, 0.0));
        assert_eq!(pick(3, 1.0), (1, 1, 0.0));
        assert_eq!(pick(3, 1.25), (1, 2, 0.25));
        assert_eq!(pick(2, 4.5), (1, 1, 0.0));
    }
}
