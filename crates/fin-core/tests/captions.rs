//! Caption blocks, SRT, text alignment, presets and safe zones (plan 4C Task 18), and the
//! `caption import` builder (`compose::caption_import`, Decision 17).

mod common;

use common::*;
use fin_core::captions::{
    BlockDraft, align_text, blocks, parse_srt, preset, presets, safe_box, safe_zones,
};
use fin_core::compose::caption_import;
use fin_core::params::{ScopeOf, check_range, check_shape, find, get_style};
use fin_core::{CaptionStyle, ItemBody, Op, Rational, Track, TrackKind, Word};

fn w(text: &str, start: &str, end: &str) -> Word {
    Word {
        text: text.into(),
        start: r(start),
        end: r(end),
    }
}

fn texts(b: &BlockDraft) -> Vec<&str> {
    b.words.iter().map(|w| w.text.as_str()).collect()
}

fn style(max_chars: u32, max_lines: u32, gap: &str, max_duration: Option<&str>) -> CaptionStyle {
    CaptionStyle {
        max_chars,
        max_lines,
        gap: r(gap),
        max_duration: max_duration.map(r),
        ..CaptionStyle::default()
    }
}

#[test]
fn blocks_split_on_gap_chars_lines_duration() {
    // A pause of 1 s (≥ gap 1/2) starts a new block; block times are absolute, word times
    // relative to the block.
    let words = [
        w("one", "0", "1/2"),
        w("two", "1/2", "1"),
        w("three", "2", "5/2"),
    ];
    let got = blocks(&words, &style(32, 2, "1/2", None));
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].start, got[0].end), (r("0"), r("1")));
    assert_eq!(texts(&got[0]), ["one", "two"]);
    assert_eq!((got[1].start, got[1].end), (r("2"), r("5/2")));
    assert_eq!(got[1].words[0], w("three", "0", "1/2"));
    // A gap of 0 never splits.
    assert_eq!(blocks(&words, &style(32, 2, "0", None)).len(), 1);
    // Lines of at most 7 characters, at most 2 lines: "aa bb" "cc dd" | "ee".
    let words: Vec<Word> = ["aa", "bb", "cc", "dd", "ee"]
        .iter()
        .enumerate()
        .map(|(i, t)| w(t, &i.to_string(), &(i + 1).to_string()))
        .collect();
    let got = blocks(&words, &style(7, 2, "0", None));
    let got: Vec<Vec<&str>> = got.iter().map(texts).collect();
    assert_eq!(got, [vec!["aa", "bb", "cc", "dd"], vec!["ee"]]);
    // One line each.
    let got = blocks(&words, &style(5, 1, "0", None));
    let got: Vec<Vec<&str>> = got.iter().map(texts).collect();
    assert_eq!(got, [vec!["aa", "bb"], vec!["cc", "dd"], vec!["ee"]]);
    // Characters are Unicode scalar values, not bytes: "привет" is 6.
    let words = [w("привет", "0", "1"), w("мир", "1", "2")];
    assert_eq!(blocks(&words, &style(10, 1, "0", None)).len(), 1);
    // A word longer than a line gets a line of its own (it overflows; measurement says so).
    let words = [w("supercalifragilistic", "0", "1"), w("x", "1", "2")];
    let got = blocks(&words, &style(5, 1, "0", None));
    assert_eq!(
        got.iter().map(texts).collect::<Vec<_>>(),
        [vec!["supercalifragilistic"], vec!["x"]]
    );
    // maxDuration: a block never spans more (a single word may).
    let words: Vec<Word> = (0..5)
        .map(|i| w("w", &i.to_string(), &(i + 1).to_string()))
        .collect();
    let got = blocks(&words, &style(100, 5, "0", Some("2")));
    assert_eq!(
        got.iter().map(|b| (b.start, b.end)).collect::<Vec<_>>(),
        [(r("0"), r("2")), (r("2"), r("4")), (r("4"), r("5"))]
    );
    assert!(blocks(&[], &style(32, 2, "1/2", None)).is_empty());
}

#[test]
fn srt_parses_and_distributes() {
    let srt = "\u{feff}1\r\n00:00:01,000 --> 00:00:03,000\r\nHello <i>big</i>\r\nworld\r\n\r\n\
               2\n00:00:04.500 --> 00:00:05,500\n{\\an8}Bye\n\n";
    let words = parse_srt(srt).unwrap();
    // "Hello big world" over 2 s by characters (5 + 3 + 5 = 13).
    assert_eq!(
        words,
        [
            w("Hello", "1", "23/13"),
            w("big", "23/13", "29/13"),
            w("world", "29/13", "3"),
            w("Bye", "9/2", "11/2"),
        ]
    );
    // A cue whose text is only tags gives no words; hours count.
    let words = parse_srt(
        "1\n01:00:00,000 --> 01:00:01,000\n<b></b>\n\n2\n01:00:02,000 --> 01:00:03,000\nx\n",
    )
    .unwrap();
    assert_eq!(words, [w("x", "3602", "3603")]);
    assert!(parse_srt("").unwrap().is_empty());
}

#[test]
fn srt_errors_name_the_line() {
    for (srt, line) in [
        ("1\n00:00:01,000 -> 00:00:02,000\nhi\n", 2),
        (
            "1\n00:00:01,000 --> 00:00:02,000\nhi\n\nx\n00:00:03,000 --> 00:00:04,000\nyo\n",
            5,
        ),
        ("1\n00:00:03,000 --> 00:00:02,000\nhi\n", 2),
        (
            "1\n00:00:01,000 --> 00:00:03,000\nhi\n\n2\n00:00:02,000 --> 00:00:04,000\nyo\n",
            6,
        ),
        ("1\n00:00:01,000 --> 00:00:02,000\n", 2),
        ("1\n00:61:01,000 --> 00:62:02,000\nhi\n", 2),
    ] {
        let e = parse_srt(srt).unwrap_err();
        assert_eq!(e.code, "SRT_INVALID", "{srt:?}");
        assert_eq!(e.details["line"], line, "{srt:?}: {e:?}");
        assert!(e.fix.is_some());
    }
}

#[test]
fn align_keeps_matching_times() {
    let old = [
        w("Hello", "0", "1"),
        w("there", "1", "2"),
        w("world", "2", "3"),
    ];
    // Same words (case and punctuation aside) keep their times; the text is the new one.
    let got = align_text(&old, "hello, there World!");
    assert_eq!(
        got,
        [
            w("hello,", "0", "1"),
            w("there", "1", "2"),
            w("World!", "2", "3"),
        ]
    );
    // A replaced word takes the old word's span.
    let got = align_text(&old, "Hello friend world");
    assert_eq!(got[1], w("friend", "1", "2"));
    assert_eq!((got[0].start, got[2].end), (r("0"), r("3")));
    // A removed word leaves its span.
    let got = align_text(&old, "Hello world");
    assert_eq!(got, [w("Hello", "0", "1"), w("world", "2", "3")]);
}

#[test]
fn align_distributes_by_chars() {
    let old = [w("a", "0", "1"), w("b", "3", "4")];
    // Two new words between the anchors share the gap [1, 3] by characters (1 : 3).
    let got = align_text(&old, "a x yyy b");
    assert_eq!(got[1], w("x", "1", "3/2"));
    assert_eq!(got[2], w("yyy", "3/2", "3"));
    // No gap: the inserted word shares the previous anchor's span (1 : 2).
    let old = [w("ab", "0", "3"), w("c", "3", "4")];
    let got = align_text(&old, "ab d c");
    assert_eq!(got, [w("ab", "0", "2"), w("d", "2", "3"), w("c", "3", "4")]);
    // At the start with no gap: the next anchor's span.
    let old = [w("ab", "0", "3")];
    let got = align_text(&old, "d ab");
    assert_eq!(got, [w("d", "0", "1"), w("ab", "1", "3")]);
    // Nothing matches: the whole old span by characters.
    let old = [w("x", "1", "2"), w("y", "2", "4")];
    let got = align_text(&old, "aaa b");
    assert_eq!(got, [w("aaa", "1", "13/4"), w("b", "13/4", "4")]);
    assert!(align_text(&old, "  ").is_empty());
}

#[test]
fn align_is_exact_rational() {
    let old = [w("a", "0", "1/3"), w("z", "1", "10/7")];
    let got = align_text(&old, "a bb ccc dddd z");
    // The four new words split [1/3, 1] in 2 : 3 : 4 (bb, ccc, dddd) — exactly.
    let span = r("1").checked_sub(r("1/3")).unwrap();
    let ninth = span.checked_mul(r("1/9")).unwrap();
    assert_eq!(got[1].start, r("1/3"));
    let two_ninths = ninth.checked_mul(r("2")).unwrap();
    assert_eq!(got[1].end, r("1/3").checked_add(two_ninths).unwrap());
    assert_eq!(got[3].end, r("1"));
    for p in got.windows(2) {
        assert!(p[0].start <= p[0].end && p[0].end <= p[1].start, "{got:?}");
    }
    // Words are text split on whitespace: line breaks and tabs never reach a word.
    let got = align_text(&old, "a\nbb\tz");
    assert_eq!(
        got.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(),
        ["a", "bb", "z"]
    );
}

#[test]
fn presets_and_zones_are_valid() {
    let zones = safe_zones();
    let names: Vec<&str> = zones.iter().map(|z| z.name).collect();
    for want in ["tiktok", "reels", "shorts", "generic-9x16", "generic-16x9"] {
        assert!(names.contains(&want), "{names:?}");
    }
    for z in zones {
        assert!(
            z.left + z.right < z.width && z.top + z.bottom < z.height,
            "{z:?}"
        );
        assert!(!z.source.is_empty() && z.checked.len() == 10, "{z:?}");
    }
    assert!(!presets().is_empty());
    assert!(preset("preset:bold").is_some() && preset("bold").is_none());
    let base = CaptionStyle::default();
    for s in std::iter::once(&base).chain(presets()) {
        if !s.id.is_empty() {
            assert!(s.id.starts_with("preset:") && !s.name.is_empty(), "{s:?}");
        }
        assert!(names.contains(&s.safe_zone.as_str()), "{s:?}");
        // Every value a preset holds passes its registry spec.
        for spec in fin_core::params::specs()
            .iter()
            .filter(|x| x.scope == fin_core::params::Scope::Style)
        {
            let found = find(&ScopeOf::Style, spec.path).unwrap();
            if let Some(v) = get_style(s, spec.path).unwrap() {
                check_shape(found, spec.path, &v).unwrap();
                check_range(found, spec.path, &v).unwrap();
            }
        }
    }
    // The box a caption may fill: the zone scaled to the timeline resolution.
    let tiktok = zones.iter().find(|z| z.name == "tiktok").unwrap();
    let [x, y, bw, bh] = safe_box(
        tiktok,
        fin_core::Resolution {
            width: 1080,
            height: 1920,
        },
    );
    let k = 1080.0 / f64::from(tiktok.width);
    assert_eq!(x, f64::from(tiktok.left) * k);
    assert_eq!(bw, 1080.0 - f64::from(tiktok.left + tiktok.right) * k);
    assert!(y > 0.0 && bh > 0.0 && y + bh < 1920.0);
}

#[test]
fn caption_import_inserts_one_item_per_block() {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    let words = parse_srt(
        "1\n00:00:00,000 --> 00:00:01,000\nHi there\n\n2\n00:00:02,000 --> 00:00:03,000\nBye\n",
    )
    .unwrap();
    let c = caption_import(&p, "t3", secs(10), &words, "preset:bold").unwrap();
    assert_eq!(c.ops.len(), 2);
    assert_eq!(c.echo["blocks"], 2);
    let Op::ItemInsert(first) = &c.ops[0] else {
        panic!("{:?}", c.ops[0]);
    };
    assert_eq!((first.at, first.duration), (secs(10), secs(1)));
    let ItemBody::Caption { block } = &first.body else {
        panic!();
    };
    assert_eq!(block.style, "preset:bold");
    assert_eq!(block.words[1].start, r("2/7"));
    let a = apply(&p, c.ops);
    assert_eq!(
        starts(&a.project, 2),
        [("cap1".into(), secs(10)), ("cap2".into(), secs(12))]
    );
    // An unknown style is NOT_FOUND; a style is read off the project.
    assert_eq!(
        caption_import(&p, "t3", secs(0), &words, "st9")
            .unwrap_err()
            .code,
        "NOT_FOUND"
    );
    assert_eq!(
        caption_import(&p, "t3", secs(0), &[], "preset:bold")
            .unwrap()
            .ops
            .len(),
        0
    );
    let _ = Rational::zero();
}
