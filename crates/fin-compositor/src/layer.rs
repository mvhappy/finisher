//! The layer pass's uniforms (spec §5.1): the inverse matrix, the level fraction, alpha and the
//! coverage edges, all from the shared [`Placement`] and converted to `f32` here.

use crate::placement::Placement;

/// The quad's edges as `(nx, ny, ax, ay)`: the same signed distance `coverage` computes
/// (positive inside, either winding); `None` for a degenerate quad, which covers nothing. A
/// zero-length edge is one every pixel is far inside of.
pub(crate) fn edges(quad: &[[f64; 2]; 4]) -> Option<[[f32; 4]; 4]> {
    let mut area = 0.0;
    for i in 0..4 {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        area += a[0] * b[1] - b[0] * a[1];
    }
    if area == 0.0 || !area.is_finite() {
        return None;
    }
    let sign = if area > 0.0 { 1.0 } else { -1.0 };
    Some([0, 1, 2, 3].map(|i| {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        let (ex, ey) = (b[0] - a[0], b[1] - a[1]);
        let len = (ex * ex + ey * ey).sqrt();
        if len == 0.0 {
            return [0.0, 1.0, 0.0, -1.0e6];
        }
        // d = sign·(ex·(py − ay) − ey·(px − ax)) / len.
        [
            (-sign * ey / len) as f32,
            (sign * ex / len) as f32,
            a[0] as f32,
            a[1] as f32,
        ]
    }))
}

/// The `Layer` uniform block of `layer.wgsl`.
pub(crate) fn uniforms(p: &Placement, edges: &[[f32; 4]; 4], frac: f32, alpha: f32) -> [f32; 24] {
    let m = &p.inverse;
    let mut out = [0.0f32; 24];
    out[..8].copy_from_slice(&[
        m[0] as f32,
        m[1] as f32,
        m[2] as f32,
        frac,
        m[3] as f32,
        m[4] as f32,
        m[5] as f32,
        alpha,
    ]);
    for (i, e) in edges.iter().enumerate() {
        out[8 + i * 4..12 + i * 4].copy_from_slice(e);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::coverage;

    /// The plane equations give the coverage `coverage` computes, both windings, rotated.
    #[test]
    fn edges_reproduce_coverage() {
        let (s, c) = (0.5f64, 0.866_025_403_784_438_6f64);
        let quad = [
            [10.0, 5.0],
            [10.0 + 40.0 * c, 5.0 + 40.0 * s],
            [10.0 + 40.0 * c - 20.0 * s, 5.0 + 40.0 * s + 20.0 * c],
            [10.0 - 20.0 * s, 5.0 + 20.0 * c],
        ];
        for q in [quad, [quad[3], quad[2], quad[1], quad[0]]] {
            let e = edges(&q).unwrap();
            for y in 0..40 {
                for x in 0..60 {
                    let px = [x as f64 + 0.5, y as f64 + 0.5];
                    let mut cov = 1.0f64;
                    for k in &e {
                        let d = f64::from(k[0]) * (px[0] - f64::from(k[2]))
                            + f64::from(k[1]) * (px[1] - f64::from(k[3]));
                        cov *= (d + 0.5).clamp(0.0, 1.0);
                    }
                    assert!((cov - coverage(&q, px)).abs() < 1e-5, "{x},{y}");
                }
            }
        }
        assert!(edges(&[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]]).is_none());
    }
}
