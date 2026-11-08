//! Tolerance T1 (spec §13.3): a GPU render against the CPU reference render of the same plan
//! and source pixels, on any adapter. Per channel |Δ| ≤ 2/255 on ≥ 99.9 % of pixels, max |Δ| ≤
//! 8/255, PSNR ≥ 50 dB. Images are 8-bit RGBA: the per-channel limits cover alpha too, the
//! PSNR is over the colour channels (an always-equal alpha would dilute it by 1.25 dB).

/// The measures T1 gates on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct T1 {
    /// Over the R, G and B channels, `10·log10(255² / MSE)`; infinite when they are equal.
    pub psnr: f64,
    /// The largest per-channel difference.
    pub max_abs: u8,
    /// The fraction of pixels with any channel differing by more than 2.
    pub over_2: f64,
}

/// The fraction of pixels allowed past 2/255.
pub const MAX_OVER_2: f64 = 0.001;
/// The largest difference allowed anywhere.
pub const MAX_ABS: u8 = 8;
/// The PSNR floor in dB.
pub const MIN_PSNR: f64 = 50.0;

/// Compares two RGBA8 images of the same size. Buffers of different lengths (or not whole
/// pixels) are as far apart as images can be: PSNR 0, `max_abs` 255, every pixel over.
pub fn t1(a: &[u8], b: &[u8]) -> T1 {
    if a.len() != b.len() || !a.len().is_multiple_of(4) {
        return T1 {
            psnr: 0.0,
            max_abs: 255,
            over_2: 1.0,
        };
    }
    let pixels = a.len() / 4;
    let (mut sq, mut max_abs, mut over) = (0u64, 0u8, 0usize);
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let mut worst = 0u8;
        for k in 0..4 {
            let d = pa[k].abs_diff(pb[k]);
            if k < 3 {
                sq += u64::from(d) * u64::from(d);
            }
            worst = worst.max(d);
        }
        max_abs = max_abs.max(worst);
        if worst > 2 {
            over += 1;
        }
    }
    let psnr = if sq == 0 {
        f64::INFINITY
    } else {
        let mse = sq as f64 / (pixels * 3) as f64;
        10.0 * libm::log10(255.0 * 255.0 / mse)
    };
    T1 {
        psnr,
        max_abs,
        over_2: if pixels == 0 {
            0.0
        } else {
            over as f64 / pixels as f64
        },
    }
}

/// Whether `t` is within T1.
pub fn passes(t: &T1) -> bool {
    t.over_2 <= MAX_OVER_2 && t.max_abs <= MAX_ABS && t.psnr >= MIN_PSNR
}

/// A visual diff for a failing case: each pixel's largest channel difference, scaled ×32 and
/// shown in red over a dimmed grey copy of `a`. Same length as `a` (empty for mismatched
/// inputs).
pub fn diff_image(a: &[u8], b: &[u8]) -> Vec<u8> {
    if a.len() != b.len() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(a.len());
    for (pa, pb) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
        let d = (0..4).map(|k| pa[k].abs_diff(pb[k])).max().unwrap_or(0);
        let grey = ((u16::from(pa[0]) + u16::from(pa[1]) + u16::from(pa[2])) / 12) as u8;
        out.extend_from_slice(&[grey.saturating_add(d.saturating_mul(32)), grey, grey, 255]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_images_pass_with_infinite_psnr() {
        let a = vec![10u8; 64];
        let t = t1(&a, &a);
        assert_eq!((t.max_abs, t.over_2), (0, 0.0));
        assert!(t.psnr.is_infinite() && passes(&t));
    }

    #[test]
    fn the_three_limits_each_fail_alone() {
        // 2000 pixels, all off by 1: PSNR 48.1 dB fails, the other two pass.
        let a = vec![100u8; 8000];
        let b: Vec<u8> = a.iter().map(|v| v + 1).collect();
        let t = t1(&a, &b);
        assert!(t.psnr < MIN_PSNR && t.max_abs == 1 && t.over_2 == 0.0 && !passes(&t));
        // One pixel off by 9 in 10,000: max fails.
        let a = vec![100u8; 40_000];
        let mut b = a.clone();
        b[0] = 109;
        let t = t1(&a, &b);
        assert!(t.psnr > MIN_PSNR && t.max_abs == 9 && !passes(&t));
        // 0.2 % of pixels off by 3: the 99.9 % rule fails.
        let mut b = a.clone();
        for p in 0..20 {
            b[p * 4] = 103;
        }
        let t = t1(&a, &b);
        assert!((t.over_2 - 0.002).abs() < 1e-12 && t.max_abs == 3 && !passes(&t));
        // 0.1 % off by 3 passes.
        let mut b = a.clone();
        for p in 0..10 {
            b[p * 4] = 103;
        }
        assert!(passes(&t1(&a, &b)));
    }

    #[test]
    fn mismatched_buffers_fail() {
        let t = t1(&[0; 8], &[0; 4]);
        assert!(!passes(&t) && t.max_abs == 255);
        assert!(diff_image(&[0; 8], &[0; 4]).is_empty());
    }
}
