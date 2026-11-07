//! Geometry, coverage and mip levels against hand-computed values (spec §5.1).

use fin_compositor::{
    base_box, coverage, crop_quad, invert, layer_matrix, mip_level, transform_point,
};
use fin_core::Resolution;
use fin_core::eval::{PlanCrop, PlanTransform};
use fin_media::{Rotation, Size};

fn res(width: u32, height: u32) -> Resolution {
    Resolution { width, height }
}

fn close(a: [f64; 2], b: [f64; 2]) -> bool {
    (a[0] - b[0]).abs() < 1e-9 && (a[1] - b[1]).abs() < 1e-9
}

#[test]
fn contain_fits_and_centres() {
    let b = base_box(Size::new(1920, 1080), res(1080, 1920));
    assert_eq!((b.x, b.y, b.width, b.height), (0.0, 656.25, 1080.0, 607.5));
    let b = base_box(Size::new(1080, 1920), res(1920, 1080));
    assert_eq!((b.x, b.y, b.width, b.height), (656.25, 0.0, 607.5, 1080.0));
    let b = base_box(Size::new(640, 360), res(1920, 1080));
    assert_eq!((b.x, b.y, b.width, b.height), (0.0, 0.0, 1920.0, 1080.0));
}

#[test]
fn the_default_transform_is_the_base_box() {
    let frame = res(1920, 1080);
    let b = base_box(Size::new(1080, 1920), frame);
    let m = layer_matrix(&PlanTransform::default(), b, frame, Rotation::R0);
    assert!(close(transform_point(&m, [0.0, 0.0]), [656.25, 0.0]));
    assert!(close(transform_point(&m, [1.0, 1.0]), [1263.75, 1080.0]));
}

#[test]
fn rotations_anchors_mirroring_and_crop() {
    let frame = res(200, 100);
    let b = base_box(Size::new(200, 100), frame);
    let t = PlanTransform::default();
    // The coded picture of a 90° source (100×200 coded, 200×100 shown... here square for
    // clarity): coded (0, 0) shows at the display's top-right.
    let m = layer_matrix(&t, b, frame, Rotation::R90);
    assert!(close(transform_point(&m, [0.0, 0.0]), [200.0, 0.0]));
    assert!(close(transform_point(&m, [0.0, 1.0]), [0.0, 0.0]));
    let m = layer_matrix(&t, b, frame, Rotation::R180);
    assert!(close(transform_point(&m, [0.0, 0.0]), [200.0, 100.0]));
    let m = layer_matrix(&t, b, frame, Rotation::R270);
    assert!(close(transform_point(&m, [0.0, 0.0]), [0.0, 100.0]));
    // Anchor at the top-left corner: that corner lands at the frame centre plus position.
    let corner = PlanTransform {
        anchor: [0.0, 0.0],
        position: [10.0, -5.0],
        ..t
    };
    let m = layer_matrix(&corner, b, frame, Rotation::R0);
    assert!(close(transform_point(&m, [0.0, 0.0]), [110.0, 45.0]));
    assert!(close(transform_point(&m, [1.0, 1.0]), [310.0, 145.0]));
    // Rotation is clockwise about the anchor: 90° sends the item's right edge down.
    let rot = PlanTransform {
        rotation: 90.0,
        ..t
    };
    let m = layer_matrix(&rot, b, frame, Rotation::R0);
    assert!(close(transform_point(&m, [1.0, 0.5]), [100.0, 150.0]));
    // A negative scale mirrors about the anchor.
    let mirror = PlanTransform {
        scale: [-1.0, 1.0],
        ..t
    };
    let m = layer_matrix(&mirror, b, frame, Rotation::R0);
    assert!(close(transform_point(&m, [0.0, 0.0]), [200.0, 0.0]));
    // Crop cuts fractions of the base box from each side without moving the rest.
    let crop = PlanCrop {
        left: 0.25,
        right: 0.0,
        top: 0.1,
        bottom: 0.5,
    };
    let q = crop_quad(&t, b, frame, &crop).unwrap();
    assert!(close(q[0], [50.0, 10.0]) && close(q[2], [200.0, 50.0]));
    assert!(
        crop_quad(
            &t,
            b,
            frame,
            &PlanCrop {
                left: 0.6,
                right: 0.4,
                ..crop
            }
        )
        .is_none()
    );
    // The inverse undoes the matrix; a zero scale has none.
    let inv = invert(&layer_matrix(&rot, b, frame, Rotation::R90)).unwrap();
    let m = layer_matrix(&rot, b, frame, Rotation::R90);
    assert!(close(
        transform_point(&inv, transform_point(&m, [0.3, 0.7])),
        [0.3, 0.7]
    ));
    assert!(
        invert(&layer_matrix(
            &PlanTransform {
                scale: [0.0, 1.0],
                ..t
            },
            b,
            frame,
            Rotation::R0
        ))
        .is_none()
    );
}

#[test]
fn coverage_on_axis_aligned_half_pixel_and_rotated_edges() {
    let sq = [[10.0, 10.0], [20.0, 10.0], [20.0, 20.0], [10.0, 20.0]];
    assert_eq!(coverage(&sq, [15.0, 15.0]), 1.0);
    assert_eq!(coverage(&sq, [10.0, 15.0]), 0.5, "centre on the edge");
    assert_eq!(coverage(&sq, [9.5, 15.0]), 0.0);
    assert_eq!(coverage(&sq, [10.0, 10.0]), 0.25, "a corner");
    // The other winding gives the same.
    let rev = [sq[3], sq[2], sq[1], sq[0]];
    assert_eq!(coverage(&rev, [10.0, 15.0]), 0.5);
    // An edge at a half pixel: x = 10.25 seen from the centre 10.5 is d = 0.25 inside.
    let half = [[10.25, 0.0], [30.0, 0.0], [30.0, 30.0], [10.25, 30.0]];
    assert_eq!(coverage(&half, [10.5, 15.0]), 0.75);
    // A big square rotated 30° about (50, 50): its left edge passes through the rotated
    // point; a pixel 0.2 px inside along the inward normal has coverage 0.7.
    let (s, c) = (libm::sin(30f64.to_radians()), libm::cos(30f64.to_radians()));
    let rot = |x: f64, y: f64| {
        [
            50.0 + (x - 50.0) * c - (y - 50.0) * s,
            50.0 + (x - 50.0) * s + (y - 50.0) * c,
        ]
    };
    let q = [
        rot(10.0, 10.0),
        rot(90.0, 10.0),
        rot(90.0, 90.0),
        rot(10.0, 90.0),
    ];
    let on_edge = rot(10.0, 50.0);
    let inside = [on_edge[0] + 0.2 * c, on_edge[1] + 0.2 * s];
    assert!((coverage(&q, inside) - 0.7).abs() < 1e-12);
    assert!((coverage(&q, on_edge) - 0.5).abs() < 1e-12);
    // A degenerate quad covers nothing.
    assert_eq!(
        coverage(
            &[[0.0, 0.0], [1.0, 1.0], [2.0, 2.0], [3.0, 3.0]],
            [1.0, 1.0]
        ),
        0.0
    );
}

#[test]
fn mip_levels() {
    let src = Size::new(100, 100);
    for (s, want) in [
        (2.0, 0.0),
        (1.0, 0.0),
        (0.7, 0.514_573_172_829_758_3),
        (0.5, 1.0),
        (0.25, 2.0),
        (0.3, 1.736_965_594_166_206_3),
    ] {
        let m = [s * 100.0, 0.0, 0.0, 0.0, s * 100.0, 0.0, 0.0, 0.0, 1.0];
        assert!(
            (mip_level(&m, src) - want).abs() < 1e-12,
            "scale {s}: {}",
            mip_level(&m, src)
        );
    }
    // The smaller axis decides; rotation does not change it.
    let m = [100.0, 0.0, 0.0, 0.0, 25.0, 0.0, 0.0, 0.0, 1.0];
    assert_eq!(mip_level(&m, src), 2.0);
    let (s, c) = (0.5f64.sqrt(), 0.5f64.sqrt());
    let m = [
        c * 50.0,
        -s * 50.0,
        0.0,
        s * 50.0,
        c * 50.0,
        0.0,
        0.0,
        0.0,
        1.0,
    ];
    assert!((mip_level(&m, src) - 1.0).abs() < 1e-12);
}
