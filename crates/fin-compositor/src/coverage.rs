//! Edge coverage (spec §5.1): WebGPU has no clamp-to-border, so both compositors weight each
//! output pixel by the product, over the quad's four edges, of `clamp(d + ½, 0, 1)`, `d` the
//! signed distance in pixels from the pixel centre to the edge (positive inside).

/// Coverage of the pixel centred at `px` by the convex quad `quad` (either winding); 0 for a
/// degenerate quad.
pub fn coverage(quad: &[[f64; 2]; 4], px: [f64; 2]) -> f64 {
    let mut area = 0.0;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        area += a[0] * b[1] - b[0] * a[1];
    }
    if area == 0.0 || !area.is_finite() {
        return 0.0;
    }
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    let mut c = 1.0;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let len = (ex * ex + ey * ey).sqrt();
        if len == 0.0 {
            continue;
        }
        // Inside is to the left of each edge for positive area (y down: counter-clockwise on
        // screen is clockwise in these coordinates); `sign` folds both windings.
        let d = sign * (ex * (px[1] - a[1]) - ey * (px[0] - a[0])) / len;
        c *= (d + 0.5).clamp(0.0, 1.0);
        if c == 0.0 {
            return 0.0;
        }
    }
    c
}
