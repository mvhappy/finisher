//! The mip chain (spec §5.1): exact 2×2 box averages, an odd last column or row averaged with
//! itself, and bilinear sampling clamped to the edge.

/// An R′G′B′ `f32` image.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Image {
    pub width: usize,
    pub height: usize,
    pub px: Vec<[f32; 3]>,
}

impl Image {
    fn get(&self, x: isize, y: isize) -> [f32; 3] {
        let x = x.clamp(0, self.width as isize - 1) as usize;
        let y = y.clamp(0, self.height as isize - 1) as usize;
        self.px[y * self.width + x]
    }

    /// Bilinear at normalised `(u, v)`: texel centres at `(i + ½) / width`, clamped to edge.
    pub fn sample(&self, u: f64, v: f64) -> [f32; 3] {
        let x = (u * self.width as f64 - 0.5) as f32;
        let y = (v * self.height as f64 - 0.5) as f32;
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let (xi, yi) = (x0 as isize, y0 as isize);
        let (a, b, c, d) = (
            self.get(xi, yi),
            self.get(xi + 1, yi),
            self.get(xi, yi + 1),
            self.get(xi + 1, yi + 1),
        );
        let mut out = [0.0; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * tx;
            let bottom = c[k] + (d[k] - c[k]) * tx;
            out[k] = top + (bottom - top) * ty;
        }
        out
    }

    /// The next level: `⌈w/2⌉ × ⌈h/2⌉`, each texel the mean of its 2×2 block.
    fn half(&self) -> Image {
        let (w, h) = (self.width.div_ceil(2), self.height.div_ceil(2));
        let mut px = Vec::with_capacity(w * h);
        for y in 0..h {
            let (y0, y1) = (2 * y, (2 * y + 1).min(self.height - 1));
            for x in 0..w {
                let (x0, x1) = (2 * x, (2 * x + 1).min(self.width - 1));
                let (a, b) = (self.px[y0 * self.width + x0], self.px[y0 * self.width + x1]);
                let (c, d) = (self.px[y1 * self.width + x0], self.px[y1 * self.width + x1]);
                px.push([0, 1, 2].map(|k| ((a[k] + b[k]) + (c[k] + d[k])) * 0.25));
            }
        }
        Image {
            width: w,
            height: h,
            px,
        }
    }
}

/// Level 0 and as many halvings as `level` needs (`⌈level⌉`), stopping at 1×1.
pub(crate) fn chain(base: Image, level: f64) -> Vec<Image> {
    let need = level.ceil() as usize;
    let mut out = vec![base];
    while out.len() <= need {
        let last = out.last().expect("level 0");
        if last.width == 1 && last.height == 1 {
            break;
        }
        let next = last.half();
        out.push(next);
    }
    out
}

/// Sampling at explicit level `level`: bilinear in `⌊L⌋` and `⌈L⌉`, mixed by the fraction
/// (levels past the chain's end read its last level).
pub(crate) fn sample(levels: &[Image], level: f64, u: f64, v: f64) -> [f32; 3] {
    let lo = (level.floor() as usize).min(levels.len() - 1);
    let hi = (level.ceil() as usize).min(levels.len() - 1);
    let a = levels[lo].sample(u, v);
    if hi == lo {
        return a;
    }
    let b = levels[hi].sample(u, v);
    let t = (level - level.floor()) as f32;
    [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t)
}
