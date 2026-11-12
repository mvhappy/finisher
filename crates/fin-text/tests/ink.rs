//! Ink boxes (generated fonts; always run): line and word boxes and the overflow check come
//! from the glyphs' positioned outlines, not their advances; the direction mark that forces a
//! line's paragraph direction moves nothing, whatever the letter spacing.
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
