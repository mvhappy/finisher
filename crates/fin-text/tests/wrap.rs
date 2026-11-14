//! Line breaking only between the request's words, paragraph bidi across lines, word
//! validation and saturating sums (generated fonts; always run).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::generated_fonts;
use fin_core::layout::{
    Align, FaceRef, Measured, TextLayout, TextMeasurer, TextRequest, TextStyle,
};
use fin_text::Measurer;

fn setup() -> (Measurer, BTreeMap<String, FaceRef>) {
    let (bytes, refs) = generated_fonts();
    (Measurer::new(Arc::new(bytes)), refs)
}

fn req(
    refs: &BTreeMap<String, FaceRef>,
    fonts: &[&str],
    words: &[&str],
    max: Option<f64>,
) -> TextRequest {
    TextRequest {
        words: words.iter().map(|w| w.to_string()).collect(),
        style: TextStyle {
            family: "TestSans".into(),
            weight: 400,
            italic: false,
            size: 50.0,
            line_height: 1.25,
            letter_spacing: 0.0,
            align: Align::Left,
        },
        faces: fonts.iter().map(|f| refs[*f].clone()).collect(),
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

/// Lines partition the words in order; every word lies on the line that lists it; only a
/// line of one word may be wider than the box.
fn assert_word_lines(l: &TextLayout, n: usize, max: Option<f64>) {
    assert_eq!(l.lines.first().map(|x| x.first_word), Some(0));
    assert_eq!(l.lines.last().map(|x| x.end_word as usize), Some(n));
    for (i, line) in l.lines.iter().enumerate() {
        assert!(
            line.first_word < line.end_word,
            "line {i} has no word: {l:?}"
        );
        if i > 0 {
            assert_eq!(l.lines[i - 1].end_word, line.first_word);
        }
        for w in line.first_word..line.end_word {
            let b = &l.words[w as usize];
            assert_eq!(b.line as usize, i, "word {w}");
            assert!(
                b.x >= line.x && b.x + b.width <= line.x + line.width,
                "word {w} outside line {i}: {l:?}"
            );
        }
        if let Some(m) = max {
            let fits = line.x >= 0 && line.x + line.width <= (m * 64.0) as i32;
            assert!(
                fits || line.end_word - line.first_word == 1,
                "line {i} too wide: {l:?}"
            );
        }
    }
}

#[test]
fn hyphen_never_breaks_a_word() {
    let (m, refs) = setup();
    let r = req(&refs, &["sans"], &["aaaa-bbbb", "c"], Some(150.0));
    let l = layout(&m, &r);
    assert_word_lines(&l, 2, Some(150.0));
    assert_eq!(l.lines.len(), 2);
    // 9 glyphs of 500 units at 50 px: 225 px of advance, drawn from 2.5 px (the first side
    // bearing) to 222.5 px (the last): 220 px of ink, one line, wider than the box.
    assert_eq!((l.words[0].width, l.lines[0].width), (220 * 64, 220 * 64));
    assert_eq!(l.lines[0].x, 160, "2.5 px");
    assert!(l.overflow);
    let wide = layout(&m, &req(&refs, &["sans"], &["aaaa-bbbb", "c"], Some(300.0)));
    assert_eq!(wide.lines.len(), 1);
    assert!(!wide.overflow);
}

#[test]
fn slash_and_url_stay_whole() {
    let (m, refs) = setup();
    for word in ["aaaa/bbbb", "https://example.com/a-b?c=d"] {
        let l = layout(&m, &req(&refs, &["sans"], &["x", word, "y"], Some(150.0)));
        assert_word_lines(&l, 3, Some(150.0));
        assert_eq!(l.lines.len(), 3, "{word}: {l:?}");
        assert_eq!(l.words[1].line, 1);
        assert!(l.missing.is_empty(), "{word}: {:?}", l.missing);
    }
}

#[test]
fn long_cjk_word_takes_one_line() {
    let (m, refs) = setup();
    let l = layout(
        &m,
        &req(
            &refs,
            &["sans", "han"],
            &["中文中文中文中文", "字"],
            Some(120.0),
        ),
    );
    assert_word_lines(&l, 2, Some(120.0));
    assert_eq!(l.lines.len(), 2);
    // Eight full-width ideographs (400 px of advance), less a 2.5 px side bearing each end.
    assert_eq!(l.words[0].width, 395 * 64, "eight full-width ideographs");
    assert!(l.overflow);
}

#[test]
fn word_with_inner_space_is_one_unit() {
    let (m, refs) = setup();
    let l = layout(&m, &req(&refs, &["sans"], &["aaaa bbbb", "c"], Some(150.0)));
    assert_word_lines(&l, 2, Some(150.0));
    assert_eq!((l.lines.len(), l.words[0].line, l.words[1].line), (2, 0, 1));
}

#[test]
fn rtl_paragraph_over_lines() {
    let (m, refs) = setup();
    let words = ["אבג", "דהו", "זחט", "יכל"];
    let mut r = req(&refs, &["rtl", "sans"], &words, Some(200.0));
    r.style.align = Align::Right;
    let l = layout(&m, &r);
    assert_word_lines(&l, 4, Some(200.0));
    assert_eq!(l.lines.len(), 2, "two 90 px words and a space per line");
    for line in &l.lines {
        assert!(line.rtl);
        // The last glyph's 50-unit right side bearing (2.5 px) is not ink.
        assert_eq!(
            line.x + line.width,
            200 * 64 - 160,
            "right-aligned in the box"
        );
    }
    assert!(l.words[0].x > l.words[1].x && l.words[2].x > l.words[3].x);
    // A later line that starts with Latin keeps the paragraph's direction: in `abc דהו` the
    // Hebrew word is visually first (left), as the RTL paragraph orders it.
    let l = layout(
        &m,
        &req(
            &refs,
            &["rtl", "sans"],
            &["שלום", "עולם", "abc", "דהו"],
            Some(260.0),
        ),
    );
    assert_word_lines(&l, 4, Some(260.0));
    assert_eq!((l.lines.len(), l.lines[1].first_word), (2, 2));
    assert!(l.lines.iter().all(|x| x.rtl));
    assert!(l.words[3].x < l.words[2].x, "{l:?}");
}

#[test]
fn mixed_bidi_over_lines() {
    let (m, refs) = setup();
    let l = layout(
        &m,
        &req(
            &refs,
            &["sans", "rtl"],
            &["Hello", "שלום", "עולם", "end"],
            Some(250.0),
        ),
    );
    assert_word_lines(&l, 4, Some(250.0));
    assert_eq!(l.lines.len(), 2);
    // The second line starts with Hebrew but the paragraph is LTR: Hebrew first, then `end`.
    assert!(l.lines.iter().all(|x| !x.rtl));
    assert_eq!(l.lines[1].first_word, 2);
    assert!(l.words[2].x < l.words[3].x, "{l:?}");
    // On one unbounded line, the Hebrew run is reversed between the Latin words.
    let one = layout(
        &m,
        &req(
            &refs,
            &["sans", "rtl"],
            &["Hello", "שלום", "עולם", "end"],
            None,
        ),
    );
    let w = &one.words;
    assert!(
        w[0].x < w[2].x && w[2].x < w[1].x && w[1].x < w[3].x,
        "{w:?}"
    );
}

#[test]
fn greedy_breaks_partition_words_at_every_width() {
    let (m, refs) = setup();
    let words = [
        "The",
        "quick",
        "brown-fox",
        "jumps",
        "over",
        "the",
        "lazy",
        "dog",
        "中文",
    ];
    for max in (0..700).step_by(37) {
        let max = Some(max as f64);
        let l = layout(&m, &req(&refs, &["sans", "han"], &words, max));
        assert_word_lines(&l, words.len(), max);
    }
}

#[test]
fn control_characters_are_refused() {
    let (m, refs) = setup();
    for bad in [
        "ab\ncd",
        "Приве\nт",
        "a\rb",
        "a\tb",
        "a\u{b}b",
        "a\u{c}b",
        "a\u{85}b",
        "a\u{2028}b",
        "a\u{2029}b",
    ] {
        let err = m
            .measure(&req(&refs, &["sans"], &["ok", bad], None))
            .unwrap_err();
        assert_eq!(err.code, "INVALID_OP", "{bad:?}");
        assert_eq!(err.details["word"], 1);
        assert!(err.fix.is_some());
    }
}

#[test]
fn negative_letter_spacing_keeps_widths_positive() {
    let (m, refs) = setup();
    let mut r = req(&refs, &["sans"], &["abc", "def"], Some(100.0));
    r.style.letter_spacing = -0.5;
    let l = layout(&m, &r);
    assert!(
        l.words.iter().all(|w| w.width >= 0) && l.lines.iter().all(|x| x.width >= 0),
        "{l:?}"
    );
}

#[test]
fn sums_saturate_instead_of_overflowing() {
    let (m, refs) = setup();
    let words = vec!["a"; 600];
    let mut r = req(&refs, &["sans"], &words, Some(1.0));
    r.style.size = 4096.0;
    r.style.line_height = 16.0;
    let l = layout(&m, &r);
    assert_eq!(l.lines.len(), 600);
    assert_eq!(l.height, i32::MAX);
    assert_eq!(l.lines.last().unwrap().top, i32::MAX);
    assert!(l.overflow);
}
