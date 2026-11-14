//! R′G′B′ to NV12 (plan 6B-1 Task 14): the picture an encoder takes, BT.709 limited range with
//! left-sited 4:2:0 chroma (what `ColourTags::BT709` tags, and what ingest reads back). Both the
//! CPU reference (`reference::scale`) and `scale.wgsl` use these definitions; they agree within
//! T1.
//!
//! - **Luma:** `Y′ = Kr·R′ + Kg·G′ + Kb·B′` (Kr 0.2126, Kb 0.0722), code `16 + 219·Y′`.
//! - **Chroma:** each sample averages the R′G′B′ it covers (linear in R′G′B′, so the same as
//!   averaging Cb and Cr): horizontally co-sited with luma column `2k` (columns `2k − 1`, `2k`,
//!   `2k + 1` weighted ¼, ½, ¼, clamped to the edge), vertically between rows `2j` and `2j + 1`
//!   (½ each, the last row of an odd height with itself); `Pb = (B′ − Y′)/1.8556`,
//!   `Pr = (R′ − Y′)/1.5748`, code `128 + 224·P`.
//! - Values are clamped to 0…1 (R′G′B′) before the matrix; codes are rounded half to even.

use fin_media::Size;

/// Rec.709's luma weights.
pub const KR: f32 = 0.2126;
pub const KB: f32 = 0.0722;
pub const KG: f32 = 1.0 - KR - KB;

/// An 8-bit NV12 picture with tight planes: `y` is `width × height`, `uv` holds
/// `⌈width/2⌉ × ⌈height/2⌉` (U, V) pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nv12Frame {
    pub size: Size,
    pub y: Vec<u8>,
    pub uv: Vec<u8>,
}

impl Nv12Frame {
    /// The chroma plane's size in pairs.
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.size.width.div_ceil(2), self.size.height.div_ceil(2))
    }
}

fn code(v: f32) -> u8 {
    v.clamp(0.0, 255.0).round_ties_even() as u8
}

fn clamp01(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| c.clamp(0.0, 1.0))
}

/// Y′ of clamped R′G′B′.
pub fn luma(rgb: [f32; 3]) -> f32 {
    let [r, g, b] = clamp01(rgb);
    KR * r + KG * g + KB * b
}

/// The luma code of one pixel.
pub fn y_code(rgb: [f32; 3]) -> u8 {
    code(16.0 + 219.0 * luma(rgb))
}

/// The (Cb, Cr) codes of an averaged R′G′B′.
pub fn uv_codes(rgb: [f32; 3]) -> (u8, u8) {
    let y = luma(rgb);
    let [r, _, b] = clamp01(rgb);
    let pb = (b - y) / (2.0 * (1.0 - KB));
    let pr = (r - y) / (2.0 * (1.0 - KR));
    (code(128.0 + 224.0 * pb), code(128.0 + 224.0 * pr))
}

/// The R′G′B′ chroma sample `(k, j)` covers in a `width × height` picture (`get(x, y)`, each
/// value clamped to 0…1 first), in a fixed order: per row `¼·left + ½·centre + ¼·right`, then
/// the two rows' mean.
pub fn chroma_rgb(
    get: impl Fn(usize, usize) -> [f32; 3],
    width: usize,
    height: usize,
    k: usize,
    j: usize,
) -> [f32; 3] {
    let x = 2 * k;
    let (xl, xr) = (x.saturating_sub(1), (x + 1).min(width - 1));
    let (y0, y1) = (2 * j, (2 * j + 1).min(height - 1));
    let row = |y: usize| {
        let (l, c, r) = (clamp01(get(xl, y)), clamp01(get(x, y)), clamp01(get(xr, y)));
        [0, 1, 2].map(|i| 0.25 * l[i] + 0.5 * c[i] + 0.25 * r[i])
    };
    let (a, b) = (row(y0), row(y1));
    [0, 1, 2].map(|i| (a[i] + b[i]) * 0.5)
}

/// `rgb` (`size`, row-major R′G′B′) as NV12 (module docs).
pub fn from_rgb(rgb: &[[f32; 3]], size: Size) -> Nv12Frame {
    let (w, h) = (size.width as usize, size.height as usize);
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    let y = rgb.iter().map(|p| y_code(*p)).collect();
    let mut uv = Vec::with_capacity(2 * cw * ch);
    for j in 0..ch {
        for k in 0..cw {
            let (u, v) = uv_codes(chroma_rgb(|x, y| rgb[y * w + x], w, h, k, j));
            uv.push(u);
            uv.push(v);
        }
    }
    Nv12Frame { size, y, uv }
}
