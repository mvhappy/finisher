//! Keyframe interpolation and the bézier solver (plan 4C Task 15, Decision 42).

mod eval_common;

use eval_common::{kf, num, r, secs};
use fin_core::eval::{
    PRESETS, PlanValue, ValueType, bezier_y, colour_hex, interpolate, parse_colour,
};
use fin_core::{Interp, ParamValue};

/// `cubic-bezier(x1, y1, x2, y2)` at u, solved offline by 200-step bisection over exact
/// fractions and printed at 60 significant digits (rounded to `f64` here).
/// Control points and `(u, y)` rows.
type Row = ([f64; 4], &'static [(f64, f64)]);

const REFERENCE: &[Row] = &[
    (
        [0.25, 0.1, 0.25, 1.0],
        &[
            (0.1, 0.09479630571604325),
            (0.25, 0.40851059135539586),
            (0.5, 0.802403387584857),
            (0.75, 0.9604589783489741),
            (0.9, 0.9943164774845563),
        ],
    ),
    (
        [0.42, 0.0, 1.0, 1.0],
        &[
            (0.1, 0.01702660965156294),
            (0.25, 0.09346465071882486),
            (0.5, 0.3153568125725393),
            (0.75, 0.6218618691748902),
            (0.9, 0.8394278457624665),
        ],
    ),
    (
        [0.0, 0.0, 0.58, 1.0],
        &[
            (0.1, 0.16057215423753346),
            (0.25, 0.3781381308251098),
            (0.5, 0.6846431874274607),
            (0.75, 0.9065353492811752),
            (0.9, 0.982973390348437),
        ],
    ),
    (
        [0.42, 0.0, 0.58, 1.0],
        &[
            (0.1, 0.019722453548311196),
            (0.25, 0.12916193104731982),
            (0.5, 0.5),
            (0.75, 0.8708380689526802),
            (0.9, 0.9802775464516889),
        ],
    ),
    (
        [0.68, -0.55, 0.265, 1.55],
        &[
            (0.1, -0.06629147733352873),
            (0.25, -0.08280710882832257),
            (0.5, 0.6066798972237717),
            (0.75, 1.0891657748134211),
            (0.9, 1.06237319516637),
        ],
    ),
    (
        [0.1, 0.9, 0.9, 0.1],
        &[
            (0.1, 0.32135955434463864),
            (0.25, 0.44682230456626826),
            (0.5, 0.5),
            (0.75, 0.5531776954337317),
            (0.9, 0.6786404456553613),
        ],
    ),
    (
        [1.0, 0.0, 0.0, 1.0],
        &[
            (0.1, 0.003761674958083166),
            (0.25, 0.029724605511925198),
            (0.5, 0.5),
            (0.75, 0.9702753944880748),
            (0.9, 0.9962383250419168),
        ],
    ),
    (
        [0.0, 1.0, 1.0, 0.0],
        &[
            (0.1, 0.38740031697727517),
            (0.25, 0.479055466999209),
            (0.5, 0.5),
            (0.75, 0.520944533000791),
            (0.9, 0.6125996830227248),
        ],
    ),
];

#[test]
fn bezier_matches_reference_table() {
    for (c, rows) in REFERENCE {
        assert_eq!(bezier_y(*c, 0.0), 0.0);
        assert_eq!(bezier_y(*c, 1.0), 1.0);
        for &(u, want) in *rows {
            let got = bezier_y(*c, u);
            assert!((got - want).abs() < 1e-6, "{c:?} at {u}: {got} vs {want}");
        }
    }
}

#[test]
fn bezier_is_total_on_degenerate_and_out_of_range_controls() {
    // x1 = x2 = 0 or 1 make x' vanish at an end; out-of-range x controls are clamped (CSS).
    for c in [
        [0.0, 0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0, 1.0],
        [1.5, 0.0, -0.5, 1.0],
        [0.0, 2.0, 1.0, -1.0],
    ] {
        for i in 0..=20 {
            let u = f64::from(i) / 20.0;
            assert!(bezier_y(c, u).is_finite(), "{c:?} at {u}");
        }
    }
    assert_eq!(
        bezier_y([1.5, 0.0, -0.5, 1.0], 0.3),
        bezier_y([1.0, 0.0, 0.0, 1.0], 0.3)
    );
    // u outside [0, 1] clamps.
    assert_eq!(bezier_y(PRESETS[0].1, -1.0), 0.0);
    assert_eq!(bezier_y(PRESETS[0].1, 2.0), 1.0);
    // Linear controls are the identity (to the solver's tolerance).
    let lin = bezier_y([0.0, 0.0, 1.0, 1.0], 0.3);
    assert!((lin - 0.3).abs() < 1e-6, "{lin}");
}

#[test]
fn presets_are_css_values() {
    let names: Vec<&str> = PRESETS.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, ["ease", "easeIn", "easeOut", "easeInOut"]);
    assert_eq!(PRESETS[0].1, [0.25, 0.1, 0.25, 1.0]);
    assert_eq!(PRESETS[1].1, [0.42, 0.0, 1.0, 1.0]);
    assert_eq!(PRESETS[2].1, [0.0, 0.0, 0.58, 1.0]);
    assert_eq!(PRESETS[3].1, [0.42, 0.0, 0.58, 1.0]);
}

#[test]
fn hold_linear_bezier_interp() {
    let base = num(7.0);
    let ty = ValueType::Number;
    let at = |kfs: &[fin_core::Keyframe], t: &str| interpolate(kfs, r(t), &base, ty);

    let hold = [
        kf(secs(0), num(0.0), Interp::Hold),
        kf(secs(2), num(10.0), Interp::Hold),
    ];
    assert_eq!(at(&hold, "1"), PlanValue::Number(0.0));
    assert_eq!(at(&hold, "2"), PlanValue::Number(10.0));

    let lin = [
        kf(secs(0), num(0.0), Interp::Linear),
        kf(secs(2), num(10.0), Interp::Linear),
    ];
    assert_eq!(at(&lin, "1/2"), PlanValue::Number(2.5));
    assert_eq!(at(&lin, "1"), PlanValue::Number(5.0));

    let [x1, y1, x2, y2] = PRESETS[3].1;
    let bez = [
        kf(secs(0), num(0.0), Interp::Bezier { x1, y1, x2, y2 }),
        kf(secs(4), num(100.0), Interp::Linear),
    ];
    let PlanValue::Number(q) = at(&bez, "1") else {
        panic!()
    };
    assert!((q - 12.916193104731982).abs() < 1e-4, "{q}");
    assert_eq!(at(&bez, "2"), PlanValue::Number(50.0));

    // The earlier keyframe's interp governs the segment.
    let mixed = [
        kf(secs(0), num(0.0), Interp::Hold),
        kf(secs(1), num(4.0), Interp::Linear),
        kf(secs(2), num(8.0), Interp::Hold),
    ];
    assert_eq!(at(&mixed, "1/2"), PlanValue::Number(0.0));
    assert_eq!(at(&mixed, "3/2"), PlanValue::Number(6.0));

    // No keyframes: the stored value.
    assert_eq!(at(&[], "1"), PlanValue::Number(7.0));

    // Vec2 interpolates per component; bools and text step.
    let v = [
        kf(secs(0), ParamValue::Vec2([0.0, 10.0]), Interp::Linear),
        kf(secs(1), ParamValue::Vec2([4.0, 20.0]), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&v, r("1/4"), &ParamValue::Vec2([0.0, 0.0]), ValueType::Vec2),
        PlanValue::Vec2([1.0, 12.5])
    );
    let b = [
        kf(secs(0), ParamValue::Bool(false), Interp::Linear),
        kf(secs(1), ParamValue::Bool(true), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&b, r("9/10"), &ParamValue::Bool(false), ValueType::Step),
        PlanValue::Bool(false)
    );
    // A Step type holds even numbers (enums stored as numbers never blend).
    assert_eq!(
        interpolate(&lin, r("1"), &base, ValueType::Step),
        PlanValue::Number(0.0)
    );
    // Mismatched shapes hold the earlier value.
    let bad = [
        kf(secs(0), num(1.0), Interp::Linear),
        kf(secs(1), ParamValue::Vec2([0.0, 0.0]), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&bad, r("1/2"), &base, ty),
        PlanValue::Number(1.0)
    );
}

#[test]
fn unsorted_and_equal_time_keyframes() {
    let ty = ValueType::Number;
    let unsorted = [
        kf(secs(2), num(10.0), Interp::Linear),
        kf(secs(0), num(0.0), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&unsorted, secs(1), &num(0.0), ty),
        PlanValue::Number(5.0)
    );
    // Two keyframes at one time: the later one wins from that time on.
    let dup = [
        kf(secs(0), num(0.0), Interp::Linear),
        kf(secs(1), num(1.0), Interp::Linear),
        kf(secs(1), num(5.0), Interp::Linear),
        kf(secs(2), num(7.0), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&dup, secs(1), &num(0.0), ty),
        PlanValue::Number(5.0)
    );
    assert_eq!(
        interpolate(&dup, r("3/2"), &num(0.0), ty),
        PlanValue::Number(6.0)
    );
    assert_eq!(
        interpolate(&dup, r("1/2"), &num(0.0), ty),
        PlanValue::Number(0.5)
    );
}

#[test]
fn colour_interpolates_per_component() {
    let ty = ValueType::Color;
    let text = |s: &str| ParamValue::Text(s.into());
    let kfs = [
        kf(secs(0), text("#000000"), Interp::Linear),
        kf(secs(2), text("#FF336680"), Interp::Linear),
    ];
    let PlanValue::Color(c) = interpolate(&kfs, secs(1), &text("#000000"), ty) else {
        panic!()
    };
    let half = |b: f64| 0.5 * (b / 255.0);
    assert_eq!(c, [half(255.0), half(51.0), half(102.0), 0.5 + half(128.0)]);
    assert_eq!(colour_hex(c), "#801a33c0");
    // A static colour resolves; an unparsable one stays text.
    assert_eq!(
        interpolate(&[], secs(0), &text("#ffffff"), ty),
        PlanValue::Color([1.0; 4])
    );
    assert_eq!(
        interpolate(&[], secs(0), &text("red"), ty),
        PlanValue::Text("red".into())
    );
    // Overshooting easing clamps each component.
    let over = Interp::Bezier {
        x1: 0.68,
        y1: -0.55,
        x2: 0.265,
        y2: 1.55,
    };
    let kfs = [
        kf(secs(0), text("#000000"), over),
        kf(secs(1), text("#ffffff"), Interp::Linear),
    ];
    let PlanValue::Color(lo) = interpolate(&kfs, r("1/4"), &text("#000000"), ty) else {
        panic!()
    };
    assert_eq!(lo, [0.0, 0.0, 0.0, 1.0]);
}

#[test]
fn colour_parsing_and_hex_rounding() {
    assert_eq!(
        parse_colour("#ff0080"),
        Some([1.0, 0.0, 128.0 / 255.0, 1.0])
    );
    assert_eq!(
        parse_colour("#FF008000"),
        Some([1.0, 0.0, 128.0 / 255.0, 0.0])
    );
    for bad in ["ff0080", "#ff008", "#ff00800", "#gg0080", "", "#"] {
        assert_eq!(parse_colour(bad), None, "{bad}");
    }
    // floor(c × 255 + 0.5): 127.5 rounds half up; out-of-range components clamp.
    assert_eq!(colour_hex([0.0, 1.0, 0.5, 2.0]), "#00ff80ff");
    assert_eq!(colour_hex([-1.0, 0.25, 0.75, 0.5]), "#0040bf80");
    for byte in 0..=255u8 {
        let c = f64::from(byte) / 255.0;
        assert_eq!(
            colour_hex([c, c, c, c]),
            format!("#{byte:02x}{byte:02x}{byte:02x}{byte:02x}")
        );
    }
}

#[test]
fn interp_outside_keyframes_clamps() {
    let ty = ValueType::Number;
    let kfs = [
        kf(secs(1), num(3.0), Interp::Linear),
        kf(secs(2), num(5.0), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&kfs, secs(0), &num(0.0), ty),
        PlanValue::Number(3.0)
    );
    assert_eq!(
        interpolate(&kfs, r("-5"), &num(0.0), ty),
        PlanValue::Number(3.0)
    );
    assert_eq!(
        interpolate(&kfs, secs(9), &num(0.0), ty),
        PlanValue::Number(5.0)
    );
    // Keyframes before the item's start (after a start trim) still shape the curve.
    let trimmed = [
        kf(r("-1"), num(0.0), Interp::Linear),
        kf(secs(1), num(10.0), Interp::Linear),
    ];
    assert_eq!(
        interpolate(&trimmed, secs(0), &num(0.0), ty),
        PlanValue::Number(5.0)
    );
}

#[test]
fn value_types_follow_the_registry() {
    use fin_core::params::ParamType;
    let cases = [
        (ParamType::Number { min: 0.0, max: 1.0 }, ValueType::Number),
        (
            ParamType::Vec2 {
                min: [0.0; 2],
                max: [1.0; 2],
            },
            ValueType::Vec2,
        ),
        (ParamType::Color, ValueType::Color),
        (ParamType::Bool, ValueType::Step),
        (ParamType::Enum { values: &["a"] }, ValueType::Step),
        (ParamType::Text { max_len: 4 }, ValueType::Step),
        (ParamType::AssetRef { kinds: &["lut"] }, ValueType::Step),
    ];
    for (ty, want) in cases {
        assert_eq!(ValueType::from(&ty), want, "{ty:?}");
    }
}

/// External review of 2026-11-09, #17: keyframes whose difference overflows 64 bits (times at
/// `(M−1)/M` and `M/(M−1)`, `M = i64::MAX`) still interpolate to a finite value: the exact
/// progress is `(M−1)/(2M−1)`, a hair under ½.
#[test]
fn progress_near_i64_max_denominators_is_finite() {
    let m = i64::MAX;
    let a = fin_core::Rational::new(m - 1, m).unwrap();
    let b = fin_core::Rational::new(m, m - 1).unwrap();
    let kfs = vec![
        kf(a, num(0.0), Interp::Linear),
        kf(b, num(1.0), Interp::Linear),
    ];
    let v = interpolate(&kfs, secs(1), &num(0.0), ValueType::Number);
    let PlanValue::Number(x) = v else {
        panic!("{v:?}")
    };
    assert!((x - 0.5).abs() < 1e-12, "{x}");
    // Bézier easing takes the same progress.
    let ease = vec![
        kf(
            a,
            num(0.0),
            Interp::Bezier {
                x1: 0.0,
                y1: 0.0,
                x2: 1.0,
                y2: 1.0,
            },
        ),
        kf(b, num(1.0), Interp::Linear),
    ];
    let v = interpolate(&ease, secs(1), &num(0.0), ValueType::Number);
    assert!(matches!(v, PlanValue::Number(x) if x.is_finite()), "{v:?}");
    // At the ends: exactly the keyframe values.
    assert_eq!(
        interpolate(&kfs, a, &num(0.0), ValueType::Number),
        PlanValue::Number(0.0)
    );
    assert_eq!(
        interpolate(&kfs, b, &num(0.0), ValueType::Number),
        PlanValue::Number(1.0)
    );
    // Far apart, with coprime huge denominators: monotone and finite.
    let lo = fin_core::Rational::new(1, m).unwrap();
    let hi = fin_core::Rational::new(m - 2, m - 1).unwrap();
    let wide = vec![
        kf(lo, num(0.0), Interp::Linear),
        kf(hi, num(1.0), Interp::Linear),
    ];
    let mut last = 0.0;
    for t in ["1/3", "1/2", "2/3", "9/10"] {
        let PlanValue::Number(x) = interpolate(&wide, r(t), &num(0.0), ValueType::Number) else {
            panic!()
        };
        assert!(x.is_finite() && x >= last, "{t}: {x}");
        last = x;
    }
    assert!((last - 0.9).abs() < 1e-9, "{last}");
}
