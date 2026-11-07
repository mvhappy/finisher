//! Colour conversion against hand-computed values (spec §13.1 unit layer).

use fin_media::colour::convert::{
    convert_gamut, expand, from_linear, primaries_to_709, to_linear, to_rgb,
};
use fin_media::{Matrix, Primaries, Range};

fn near(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
    (0..3).all(|k| (a[k] - b[k]).abs() <= eps)
}

#[test]
fn range_expansion_and_matrices() {
    let rgb = |y, cb, cr, range, m| {
        let (l, pb, pr) = expand(y, cb, cr, range, 8);
        to_rgb(l, pb, pr, m)
    };
    assert_eq!(
        rgb(16.0, 128.0, 128.0, Range::Limited, Matrix::Bt709),
        [0.0, 0.0, 0.0]
    );
    assert!(near(
        rgb(235.0, 128.0, 128.0, Range::Limited, Matrix::Bt709),
        [1.0, 1.0, 1.0],
        1e-6
    ));
    assert!(near(
        rgb(255.0, 128.0, 128.0, Range::Full, Matrix::Bt601),
        [1.0, 1.0, 1.0],
        1e-6
    ));
    // BT.709 limited-range red (63, 102, 240) is (1, 0, 0) to within a code value.
    assert!(near(
        rgb(63.0, 102.0, 240.0, Range::Limited, Matrix::Bt709),
        [1.0, 0.0, 0.0],
        0.01
    ));
    // BT.601 limited-range red (81, 90, 240).
    assert!(near(
        rgb(81.0, 90.0, 240.0, Range::Limited, Matrix::Bt601),
        [1.0, 0.0, 0.0],
        0.01
    ));
    // BT.2020 NCL limited-range red (74, 97, 240).
    assert!(near(
        rgb(74.0, 97.0, 240.0, Range::Limited, Matrix::Bt2020Ncl),
        [1.0, 0.0, 0.0],
        0.01
    ));
    // Identity is GBR.
    assert_eq!(to_rgb(0.25, 0.0, 0.25, Matrix::Identity), [0.75, 0.25, 0.5]);
    // 10-bit limited white.
    assert!(near(
        {
            let (l, pb, pr) = expand(940.0, 512.0, 512.0, Range::Limited, 10);
            to_rgb(l, pb, pr, Matrix::Bt709)
        },
        [1.0, 1.0, 1.0],
        1e-6
    ));
}

#[test]
fn transfer_and_gamut() {
    assert_eq!(to_linear(1.0), 1.0);
    assert_eq!(to_linear(-0.1), 0.0);
    assert!((to_linear(0.5) - 0.189_464_57).abs() < 1e-6, "0.5^2.4");
    assert!((from_linear(to_linear(0.7)) - 0.7).abs() < 1e-6);
    assert!(
        primaries_to_709(Primaries::Bt709).is_none()
            && primaries_to_709(Primaries::Bt601_625).is_none()
    );
    // White stays white; BT.2020 pure green is out of Rec.709's gamut and clips.
    let m = primaries_to_709(Primaries::Bt2020).unwrap();
    assert!(near(
        convert_gamut([1.0, 1.0, 1.0], &m),
        [1.0, 1.0, 1.0],
        1e-3
    ));
    let g = convert_gamut([0.0, 1.0, 0.0], &m);
    assert_eq!((g[0], g[2]), (0.0, 0.0));
    assert!((g[1] - 1.0).abs() < 1e-6);
    let p3 = primaries_to_709(Primaries::DisplayP3).unwrap();
    assert!(near(
        convert_gamut([1.0, 1.0, 1.0], &p3),
        [1.0, 1.0, 1.0],
        1e-3
    ));
}
