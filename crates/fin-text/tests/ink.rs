//! Ink boxes (generated fonts; always run): line and word boxes and the overflow check come
//! from the glyphs' positioned outlines, not their advances (review #57); the direction mark
//! that forces a line's paragraph direction moves nothing, whatever the letter spacing (#56).
//!
//! At 100 px one font unit is 0.1 px. `TestSans` `W` advances 800 units, drawn 50..750;
//! a `TestRtl` letter advances 600, drawn 50..550; `TestOverhang` is described in
//! `fin_testfonts::OVERHANG`.

mod common;

use std::sync::Arc;

use common::{MapBytes, face_ref};
use fin_core::layout::{
    Align, FaceRef, Measured, TextLayout, TextMeasurer, TextRequest, TextStyle,
};
use fin_testfonts as tf;
use fin_text::Measurer;

fn font(data: Vec<u8>) -> (Measurer, FaceRef) {
    let mut bytes = MapBytes::default();
    let sha = bytes.add(data.clone());
    (Measurer::new(Arc::new(bytes)), face_ref(&sha, &data, 0))
}

fn req(face: &FaceRef, words: &[&str], size: f64, max: Option<f64>) -> TextRequest {
    TextRequest {
        words: words.iter().map(|w| w.to_string()).collect(),
        style: TextStyle {
            family: face.family.clone(),
            weight: 400,
            italic: false,
            size,
            line_height: 1.25,
            letter_spacing: 0.0,
            align: Align::Left,
        },
        faces: vec![face.clone()],
        max_width: max,
        max_lines: None,
    }
}

fn layout(m: &Measurer, r: &TextRequest) -> TextLayout {
    match m.measure(r).unwrap() {
        Measured::Layout(l) => l,
        other => panic!("expected a layout, got {other:?}"),
    }
}

/// `(x, width)` of line 0 in px.
fn line_px(l: &TextLayout) -> (f64, f64) {
    let line = &l.lines[0];
    (f64::from(line.x) / 64.0, f64::from(line.width) / 64.0)
}

/// Where the pen starts a line `advance` px wide in a box `max` px wide.
fn pen(align: Align, advance: f64, max: f64) -> f64 {
    match align {
        Align::Left => 0.0,
        Align::Center => (max - advance) / 2.0,
        Align::Right => max - advance,
    }
}

const ALIGNS: [Align; 3] = [Align::Left, Align::Center, Align::Right];

/// One glyph, any tracking, any alignment: it sits exactly where it would with no direction
/// mark in front of it. The line is the glyph's advance plus the tracking after it; the ink is
/// the outline at the pen. Only a right-aligned glyph whose negative tracking eats its right
/// side bearing reaches past the box (the outline is really drawn there).
#[test]
fn tracking_gives_the_direction_mark_no_advance() {
    let (m, sans) = font(tf::test_sans());
    for tracking in [-0.1, 0.0, 0.1] {
        for align in ALIGNS {
            let mut r = req(&sans, &["W"], 100.0, Some(972.0));
            r.style.letter_spacing = tracking;
            r.style.align = align;
            let l = layout(&m, &r);
            let x = pen(align, 80.0 + 100.0 * tracking, 972.0) + 5.0;
            let what = format!("tracking {tracking}, {align:?}: {l:?}");
            assert_eq!(line_px(&l), (x, 70.0), "{what}");
            assert_eq!(
                (l.words[0].x, l.words[0].width),
                (l.lines[0].x, l.lines[0].width)
            );
            assert_eq!(l.overflow, x + 70.0 > 972.0, "{what}");
        }
    }
}

/// A Hebrew line in a Latin paragraph is shaped behind an LRM: the mark moves nothing.
#[test]
fn tracking_gives_a_needed_lrm_no_advance() {
    let mut bytes = MapBytes::default();
    let (sans, rtl) = (tf::test_sans(), tf::test_rtl());
    let faces = vec![
        face_ref(&bytes.add(sans.clone()), &sans, 0),
        face_ref(&bytes.add(rtl.clone()), &rtl, 0),
    ];
    let m = Measurer::new(Arc::new(bytes));
    for tracking in [-0.1, 0.1] {
        for align in ALIGNS {
            let mut r = req(&faces[0], &["Wa", "א"], 100.0, Some(150.0));
            r.faces = faces.clone();
            r.style.letter_spacing = tracking;
            r.style.align = align;
            let l = layout(&m, &r);
            assert_eq!(l.lines.len(), 2, "{l:?}");
            assert!(!l.lines[1].rtl);
            let x = pen(align, 60.0 + 100.0 * tracking, 150.0) + 5.0;
            let line = &l.lines[1];
            let got = (f64::from(line.x) / 64.0, f64::from(line.width) / 64.0);
            assert_eq!(got, (x, 50.0), "tracking {tracking}, {align:?}: {l:?}");
        }
    }
}

/// The reviewer's case: a left-aligned `W` with −0.1 em tracking fits its box.
#[test]
fn negative_tracking_left_aligned_fits() {
    let (m, sans) = font(tf::test_sans());
    let mut r = req(&sans, &["W"], 100.0, Some(972.0));
    r.style.letter_spacing = -0.1;
    let l = layout(&m, &r);
    assert_eq!(line_px(&l), (5.0, 70.0));
    assert!(!l.overflow, "{l:?}");
}

/// The same for a right-to-left line (shaped behind an RLM): the letter's origin is the
/// left end of its tracked advance.
#[test]
fn tracking_gives_the_rtl_mark_no_advance() {
    let (m, rtl) = font(tf::test_rtl());
    for tracking in [-0.1, 0.0, 0.1] {
        for align in ALIGNS {
            let mut r = req(&rtl, &["א"], 100.0, Some(972.0));
            r.style.letter_spacing = tracking;
            r.style.align = align;
            let l = layout(&m, &r);
            assert!(l.lines[0].rtl);
            let x = pen(align, 60.0 + 100.0 * tracking, 972.0) + 5.0;
            assert_eq!(
                line_px(&l),
                (x, 50.0),
                "tracking {tracking}, {align:?}: {l:?}"
            );
            assert_eq!(l.overflow, x + 50.0 > 972.0);
        }
    }
    // A Latin line in a Hebrew paragraph is shaped behind an RLM (its own first strong
    // character is left-to-right): the mark moves nothing there either.
    let mut bytes = MapBytes::default();
    let (rtl_data, sans_data) = (tf::test_rtl(), tf::test_sans());
    let faces = vec![
        face_ref(&bytes.add(rtl_data.clone()), &rtl_data, 0),
        face_ref(&bytes.add(sans_data.clone()), &sans_data, 0),
    ];
    let both = Measurer::new(Arc::new(bytes));
    for tracking in [-0.1, 0.1] {
        for align in ALIGNS {
            let mut r = req(&faces[0], &["אבג", "W"], 100.0, Some(200.0));
            r.faces = faces.clone();
            r.style.letter_spacing = tracking;
            r.style.align = align;
            let l = layout(&both, &r);
            assert_eq!(l.lines.len(), 2, "{l:?}");
            assert!(l.lines[1].rtl);
            let x = pen(align, 80.0 + 100.0 * tracking, 200.0) + 5.0;
            let line = &l.lines[1];
            let got = (f64::from(line.x) / 64.0, f64::from(line.width) / 64.0);
            assert_eq!(got, (x, 70.0), "tracking {tracking}, {align:?}: {l:?}");
        }
    }
    // Two words keep the paragraph's order with tracking too.
    let mut r = req(&rtl, &["אב", "גד"], 100.0, None);
    r.style.letter_spacing = -0.1;
    let l = layout(&m, &r);
    assert!(l.words[0].x > l.words[1].x, "{l:?}");
}

/// A line needs no direction mark when its own first strong character already gives the
/// paragraph's direction, so nothing synthetic sits in front of its glyphs: a word that starts
/// with a combining mark keeps the mark where the font draws it (−35..−15 px), not shifted by
/// fallback positioning against a hidden mark glyph.
#[test]
fn leading_combining_mark_has_no_synthetic_base() {
    let (m, f) = font(tf::test_overhang());
    let l = layout(&m, &req(&f, &["\u{301}a"], 100.0, None));
    // The mark −35..−15, then `a` (pen 0) 5..45.
    assert_eq!(line_px(&l), (-35.0, 80.0), "{l:?}");
    assert!(layout(&m, &req(&f, &["\u{301}a"], 100.0, Some(100.0))).overflow);
}

/// The reviewer's case: `W` advances 20 px but is drawn 5..75 px.
#[test]
fn right_overhang_overflows() {
    let (m, f) = font(tf::test_overhang());
    let l = layout(&m, &req(&f, &["W"], 100.0, Some(40.0)));
    assert_eq!(line_px(&l), (5.0, 70.0));
    assert_eq!(
        (l.width, l.words[0].x, l.words[0].width),
        (70 * 64, 5 * 64, 70 * 64)
    );
    assert!(l.overflow, "{l:?}");
    assert!(!layout(&m, &req(&f, &["W"], 100.0, Some(75.0))).overflow);
    assert!(!layout(&m, &req(&f, &["W"], 100.0, None)).overflow);
    // The native workflow's case: 1000 px in a 572 px box; drawn 50..750 px.
    let l = layout(&m, &req(&f, &["W"], 1000.0, Some(572.0)));
    assert_eq!(line_px(&l), (50.0, 700.0));
    assert!(l.overflow);
}

/// `J` advances 50 px but is drawn −20..45 px: past the left edge when left-aligned.
#[test]
fn left_overhang_overflows_and_aligns() {
    let (m, f) = font(tf::test_overhang());
    for (align, x) in [
        (Align::Left, -20.0),
        (Align::Center, pen(Align::Center, 50.0, 1000.0) - 20.0),
        (Align::Right, pen(Align::Right, 50.0, 1000.0) - 20.0),
    ] {
        let mut r = req(&f, &["J"], 100.0, Some(1000.0));
        r.style.align = align;
        let l = layout(&m, &r);
        assert_eq!(line_px(&l), (x, 65.0), "{align:?}");
        assert_eq!(l.overflow, align == Align::Left, "{align:?}");
    }
    // A right-aligned `W` (drawn 55 px past its advance) reaches past the right edge.
    let mut r = req(&f, &["W"], 100.0, Some(1000.0));
    r.style.align = Align::Right;
    let l = layout(&m, &r);
    assert_eq!(line_px(&l), (985.0, 70.0));
    assert!(l.overflow);
}

/// Word boxes are ink boxes too, and the line's is their union.
#[test]
fn word_boxes_are_ink() {
    let (m, f) = font(tf::test_overhang());
    // `J` 0..50 (ink −20..45), space 50..75, `W` 75..95 (ink 80..150).
    let l = layout(&m, &req(&f, &["J", "W"], 100.0, None));
    let words: Vec<(i32, i32)> = l.words.iter().map(|w| (w.x, w.width)).collect();
    assert_eq!(words, [(-20 * 64, 65 * 64), (80 * 64, 70 * 64)]);
    assert_eq!(line_px(&l), (-20.0, 170.0));
    assert_eq!(l.width, 170 * 64);
}

/// A zero-advance combining mark adds its own outline where it is drawn: over its base.
#[test]
fn combining_mark_ink() {
    let (m, f) = font(tf::test_overhang());
    let base = layout(&m, &req(&f, &["a"], 100.0, None));
    assert_eq!(line_px(&base), (5.0, 40.0));
    let marked = layout(&m, &req(&f, &["a\u{301}"], 100.0, None));
    assert!(marked.missing.is_empty(), "{marked:?}");
    let (x, width) = line_px(&marked);
    // The mark is drawn inside the base's ink span here, so the union is the base's.
    assert_eq!((x, width), (5.0, 40.0), "{marked:?}");
    // After `W` (advance 20 px, drawn 5..75) the shaper's fallback mark positioning centres
    // the mark on the base's advance with a 15 px offset: drawn 0..20 px. The offset counts
    // (without it the mark would be drawn −15..5 and the line would start at −15).
    let l = layout(&m, &req(&f, &["W\u{301}"], 100.0, None));
    assert_eq!(line_px(&l), (0.0, 75.0), "{l:?}");
}

/// A line whose first word already reaches past the left edge still breaks by its width: the
/// greedy breaker accepts a longer candidate that overflows no more than its first word does.
#[test]
fn overhang_at_line_start_keeps_greedy_breaking() {
    let (m, f) = font(tf::test_overhang());
    // `Ja` advances 100 (ink −20..95); with a 25 px space, `Ja Ja` reaches 220, `Ja Ja Ja` 345.
    let l = layout(&m, &req(&f, &["Ja", "Ja", "Ja"], 100.0, Some(300.0)));
    let lines: Vec<(u32, u32)> = l.lines.iter().map(|x| (x.first_word, x.end_word)).collect();
    assert_eq!(lines, [(0, 2), (2, 3)], "{l:?}");
    assert!(l.overflow);
}
