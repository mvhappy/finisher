//! `fin-text` over fonts generated at test time (always run, on every OS and on wasm32).

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::{Case, MapBytes, face_ref, layout_hash};
use fin_core::layout::{
    Align, FaceRef, Measured, TextLayout, TextMeasurer, TextRequest, TextStyle,
};
use fin_testfonts as tf;
use fin_text::Measurer;
use serde::Deserialize;

/// The generated fonts, by corpus name, and a provider holding them.
struct Fonts {
    bytes: MapBytes,
    refs: BTreeMap<String, FaceRef>,
}

fn fonts() -> Fonts {
    let mut bytes = MapBytes::default();
    let mut refs = BTreeMap::new();
    let ttc = tf::collection(&[tf::test_sans(), tf::build(&tf::LATIN_BOLD)]);
    for (name, data) in [
        ("sans", tf::test_sans()),
        ("rtl", tf::test_rtl()),
        ("han", tf::test_han()),
        ("latinBold", tf::build(&tf::LATIN_BOLD)),
        ("var", tf::test_variable()),
        ("overhang", tf::test_overhang()),
        ("color", tf::test_color()),
    ] {
        let sha = bytes.add(data.clone());
        refs.insert(name.to_string(), face_ref(&sha, &data, 0));
    }
    let sha = bytes.add(ttc.clone());
    for face in 0..2 {
        refs.insert(format!("ttc#{face}"), face_ref(&sha, &ttc, face));
    }
    Fonts { bytes, refs }
}

impl Fonts {
    fn faces(&self, names: &[&str]) -> Vec<FaceRef> {
        names.iter().map(|n| self.refs[*n].clone()).collect()
    }
    fn measurer(self) -> (Measurer, BTreeMap<String, FaceRef>) {
        (Measurer::new(Arc::new(self.bytes)), self.refs)
    }
}

fn style(size: f64) -> TextStyle {
    TextStyle {
        family: "TestSans".into(),
        weight: 400,
        italic: false,
        size,
        line_height: 1.25,
        letter_spacing: 0.0,
        align: Align::Left,
    }
}

fn req(words: &[&str], faces: Vec<FaceRef>, st: TextStyle) -> TextRequest {
    TextRequest {
        words: words.iter().map(|w| w.to_string()).collect(),
        style: st,
        faces,
        max_width: None,
        max_lines: None,
    }
}

fn layout(m: &Measurer, r: &TextRequest) -> TextLayout {
    match m.measure(r).unwrap() {
        Measured::Layout(l) => l,
        other => panic!("expected a layout, got {other:?}"),
    }
}

#[derive(Deserialize)]
struct Corpus {
    cases: Vec<Case>,
}

fn corpus() -> Vec<Case> {
    serde_json::from_str::<Corpus>(include_str!("corpus/generated.json"))
        .unwrap()
        .cases
}

fn run_corpus(m: &Measurer, refs: &BTreeMap<String, FaceRef>) -> Vec<(String, TextLayout)> {
    corpus()
        .iter()
        .map(|c| {
            let faces = c.fonts.iter().map(|f| refs[f].clone()).collect();
            (c.id.clone(), layout(m, &c.request(faces)))
        })
        .collect()
}

/// The cross-machine check: the generated corpus hashes equal the committed `expected.json`.
#[test]
fn generated_corpus_matches_expected() {
    let (m, refs) = fonts().measurer();
    let layouts = run_corpus(&m, &refs);
    let hashes: BTreeMap<String, String> = layouts
        .iter()
        .map(|(id, l)| (id.clone(), layout_hash(l)))
        .collect();
    if common::blessing() {
        common::bless("generated", hashes, &layouts);
        return;
    }
    for (id, l) in &layouts {
        println!(
            "{id} {} {}",
            layout_hash(l),
            serde_json::to_string(l).unwrap()
        );
    }
    assert_eq!(
        hashes,
        common::expected().generated,
        "rerun with FIN_TEXT_BLESS=1 after review"
    );
}

#[test]
fn same_bytes_same_layout() {
    let (a, refs) = fonts().measurer();
    let (b, _) = fonts().measurer();
    let first = run_corpus(&a, &refs);
    assert_eq!(first, run_corpus(&b, &refs));
    // A cached system answers like a fresh one.
    assert_eq!(first, run_corpus(&a, &refs));
}

#[test]
fn fallback_follows_face_order() {
    let f = fonts();
    let (sans_first, han_first) = (f.faces(&["sans", "han"]), f.faces(&["han", "sans"]));
    let (sans_sha, han_sha) = (sans_first[0].sha256.clone(), han_first[0].sha256.clone());
    let (m, _) = f.measurer();
    // 100 px: one font unit is 0.1 px = 6.4/64 px. Widths are ink: every generated glyph is
    // drawn 50 units inside its advance on both sides.
    let a = layout(&m, &req(&["a中"], sans_first, style(100.0)));
    let b = layout(&m, &req(&["a中"], han_first, style(100.0)));
    assert_eq!(
        a.words[0].width,
        (500 + 1000 - 100) * 64 / 10,
        "a from TestSans, 中 from TestHan"
    );
    assert_eq!(
        b.words[0].width,
        (700 + 1000 - 100) * 64 / 10,
        "both from TestHan"
    );
    assert_eq!(a.faces, [sans_sha.clone(), han_sha.clone()]);
    assert_eq!(b.faces, [han_sha], "TestSans is never needed");
    assert!(a.missing.is_empty() && b.missing.is_empty());
}

#[test]
fn missing_bytes_are_unavailable() {
    let f = fonts();
    let faces = f.faces(&["sans", "han", "rtl"]);
    let mut partial = MapBytes::default();
    partial.add(tf::test_sans());
    let m = Measurer::new(Arc::new(partial));
    let got = m
        .measure(&req(&["Hi"], faces.clone(), style(48.0)))
        .unwrap();
    assert_eq!(
        got,
        Measured::Unavailable {
            missing: vec![faces[1].sha256.clone(), faces[2].sha256.clone()]
        }
    );
    // A face index the file does not have is unusable bytes: unavailable, never an error.
    let mut bad = faces[0].clone();
    bad.face = 3;
    let got = m
        .measure(&req(&["Hi"], vec![bad.clone()], style(48.0)))
        .unwrap();
    assert_eq!(
        got,
        Measured::Unavailable {
            missing: vec![bad.sha256]
        }
    );
}

#[test]
fn rtl_run_is_rtl() {
    let f = fonts();
    let faces = f.faces(&["rtl"]);
    let (m, _) = f.measurer();
    let l = layout(&m, &req(&["אבג", "דהו"], faces, style(50.0)));
    assert_eq!(l.lines.len(), 1);
    assert!(l.lines[0].rtl);
    assert!(
        l.words[0].x > l.words[1].x,
        "the first word is on the right: {l:?}"
    );
    assert_eq!(l.words[0].width, (3 * 600 - 100) * 50 * 64 / 1000);
}

#[test]
fn wrap_and_overflow() {
    let f = fonts();
    let faces = f.faces(&["sans"]);
    let (m, _) = f.measurer();
    let words = [
        "The", "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
    ];
    let mut r = req(&words, faces, style(64.0));
    r.max_width = Some(400.0);
    let l = layout(&m, &r);
    assert!(l.lines.len() >= 3, "{l:?}");
    assert!(!l.overflow);
    assert!(
        l.lines
            .iter()
            .all(|line| line.x >= 0 && line.x + line.width <= 400 * 64)
    );
    // Lines partition the words in order, and each word sits on its line.
    assert_eq!(l.lines[0].first_word, 0);
    assert_eq!(l.lines.last().unwrap().end_word as usize, words.len());
    for pair in l.lines.windows(2) {
        assert_eq!(pair[0].end_word, pair[1].first_word);
        assert_eq!(pair[1].top, pair[0].top + pair[0].height);
    }
    for (i, line) in l.lines.iter().enumerate() {
        for w in line.first_word..line.end_word {
            assert_eq!(l.words[w as usize].line as usize, i);
        }
    }
    assert_eq!(l.height, l.lines.len() as i32 * 80 * 64);
    r.max_lines = Some(2);
    assert!(layout(&m, &r).overflow);
    r.max_lines = None;
    r.words = vec!["Wwwwwwwwwwwwwwww".into()];
    let long = layout(&m, &r);
    assert_eq!(long.lines.len(), 1, "never broken inside a word");
    assert!(long.overflow);
}

#[test]
fn letter_spacing_and_alignment() {
    let f = fonts();
    let faces = f.faces(&["sans"]);
    let (m, _) = f.measurer();
    let plain = layout(&m, &req(&["abc"], faces.clone(), style(100.0)));
    // `abc` advances 1500 units, drawn 50..1450.
    assert_eq!((plain.lines[0].x, plain.width), (5 * 64, 1400 * 64 / 10));
    let mut spaced = style(100.0);
    spaced.letter_spacing = 0.1;
    let spaced = layout(&m, &req(&["abc"], faces.clone(), spaced));
    assert_eq!(
        spaced.width,
        plain.width + 2 * 10 * 64,
        "0.1 em = 10 px after each glyph; the ink ends at the last glyph's outline"
    );
    for (align, x) in [
        (Align::Left, 5 * 64),
        (Align::Center, (1000 - 150) * 64 / 2 + 5 * 64),
        (Align::Right, (1000 - 150) * 64 + 5 * 64),
    ] {
        let mut st = style(100.0);
        st.align = align;
        let mut r = req(&["abc"], faces.clone(), st);
        r.max_width = Some(1000.0);
        let l = layout(&m, &r);
        assert_eq!((l.lines[0].x, l.words[0].x), (x, x), "{align:?}");
    }
}

#[test]
fn platform_fallback_never_used() {
    // Nothing in the crate can build the platform's fallback or load system fonts.
    for (file, src) in [
        ("lib.rs", include_str!("../src/lib.rs")),
        ("system.rs", include_str!("../src/system.rs")),
        ("measure.rs", include_str!("../src/measure.rs")),
        ("fallback.rs", include_str!("../src/fallback.rs")),
        ("info.rs", include_str!("../src/info.rs")),
        ("intern.rs", include_str!("../src/intern.rs")),
    ] {
        for banned in [
            "PlatformFallback",
            "FontSystem::new(",
            "new_with_fonts",
            "new_with_locale_and_db(",
            "load_system_fonts",
            "Buffer::new(",
        ] {
            let code: String = src
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            assert!(!code.contains(banned), "{file} uses {banned}");
        }
    }
    // A code point the request's faces lack stays missing; no other font is consulted.
    let f = fonts();
    let faces = f.faces(&["sans"]);
    let sha = faces[0].sha256.clone();
    let (m, _) = f.measurer();
    let l = layout(&m, &req(&["a中€"], faces, style(48.0)));
    assert_eq!(l.missing, [0x20AC, 0x4E2D]);
    assert_eq!(l.faces, [sha]);
}

#[test]
fn interner_leaks_each_name_once() {
    assert_eq!(fin_text::interned("interner-test-family"), None);
    let a = fin_text::intern("interner-test-family");
    let b = fin_text::intern(&String::from("interner-test-family"));
    assert!(std::ptr::eq(a, b));
    assert!(fin_text::interned("interner-test-family").is_some_and(|c| std::ptr::eq(a, c)));
    assert!(fin_text::interned_count() >= 1);
    // A face is loaded under `fin:<sha256>:<face>`, interned on first use and reused by every
    // later system, in this measurer or another.
    let probe = tf::Spec {
        family: "InternProbe",
        ..tf::SANS
    };
    let mut bytes = MapBytes::default();
    let data = tf::build(&probe);
    let sha = bytes.add(data.clone());
    let name = format!("fin:{sha}:0");
    assert_eq!(fin_text::interned(&name), None);
    let m = Measurer::new(Arc::new(bytes));
    let faces = vec![face_ref(&sha, &data, 0)];
    layout(&m, &req(&["x"], faces.clone(), style(20.0)));
    let first = fin_text::interned(&name).expect("interned by the first system");
    let mut again = MapBytes::default();
    again.add(data);
    let m2 = Measurer::new(Arc::new(again));
    layout(&m2, &req(&["y"], faces, style(30.0)));
    assert!(std::ptr::eq(first, fin_text::interned(&name).unwrap()));
}

#[test]
fn systems_are_cached_per_face_list_up_to_eight() {
    let f = fonts();
    let names = ["sans", "rtl", "han", "latinBold", "ttc#0", "ttc#1"];
    let all = f.faces(&names);
    let (m, _) = f.measurer();
    for i in 0..names.len() {
        for j in 0..names.len() {
            if i != j {
                layout(
                    &m,
                    &req(&["a"], vec![all[i].clone(), all[j].clone()], style(20.0)),
                );
            }
        }
    }
    assert_eq!(m.cached_systems(), fin_text::SYSTEMS_CACHED);
}

#[test]
fn invalid_style_is_invalid_range() {
    let f = fonts();
    let faces = f.faces(&["sans"]);
    let (m, _) = f.measurer();
    let mut st = style(f64::NAN);
    let err = m
        .measure(&req(&["a"], faces.clone(), st.clone()))
        .unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.details["path"].as_str()),
        ("INVALID_RANGE", Some("size"))
    );
    st.size = 20.0;
    st.line_height = 0.0;
    let err = m
        .measure(&req(&["a"], faces.clone(), st.clone()))
        .unwrap_err();
    assert_eq!(err.details["path"], "lineHeight");
    st.line_height = 1.0;
    st.letter_spacing = -0.6;
    let err = m.measure(&req(&["a"], faces, st.clone())).unwrap_err();
    assert_eq!(err.details["path"], "letterSpacing");
    st.letter_spacing = 0.0;
    let err = m.measure(&req(&["a"], vec![], st)).unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.details["path"].as_str()),
        ("INVALID_OP", Some("faces"))
    );
}

#[test]
fn fingerprint_names_versions() {
    let fp = fin_text::fingerprint();
    let rev = format!("rev/{}", fin_text::FIN_TEXT_REVISION);
    for part in [
        "fin-text/0.0.0",
        rev.as_str(),
        "cosmic-text/0.19.",
        "harfrust/0.5.",
        "skrifa/0.40.",
        "read-fonts/0.37.",
        "unicode-bidi/0.3.",
        "unicode-linebreak/0.1.",
        "unicode-segmentation/1.",
        "unicode-script/0.5.",
    ] {
        assert!(fp.contains(part), "{fp}");
    }
    assert!(!fp.contains('\n') && !fp.contains("  "), "{fp}");
    let (m, _) = fonts().measurer();
    assert_eq!(m.fingerprint(), fp);
}

#[test]
fn font_info_reads_faces_and_scripts() {
    let info = |b: &[u8]| fin_text::font_info(b).unwrap();
    let sans = info(&tf::test_sans());
    assert_eq!(sans.len(), 1);
    let s = &sans[0];
    assert_eq!(
        (s.index, s.family.as_str(), s.weight, s.italic),
        (0, "TestSans", 400, false)
    );
    assert_eq!(s.scripts, ["Cyrl", "Latn"]);
    assert_eq!(info(&tf::test_rtl())[0].scripts, ["Hebr"]);
    assert_eq!(
        info(&tf::test_han())[0].scripts,
        ["Hani"],
        "three Latin letters are not Latin coverage"
    );
    let italic = tf::Spec {
        family: "Test Italic",
        weight: 300,
        italic: true,
        ..tf::SANS
    };
    let it = &info(&tf::build(&italic))[0];
    assert_eq!(
        (it.family.as_str(), it.weight, it.italic),
        ("Test Italic", 300, true)
    );

    let ttc = info(&tf::collection(&[
        tf::test_sans(),
        tf::build(&tf::LATIN_BOLD),
    ]));
    assert_eq!(ttc.len(), 2);
    assert_eq!((ttc[0].index, ttc[0].family.as_str()), (0, "TestSans"));
    let b = &ttc[1];
    assert_eq!(
        (b.index, b.family.as_str(), b.weight, b.italic),
        (1, "TestLatin", 700, false)
    );
    assert_eq!(b.scripts, ["Latn"]);

    // Every script a test font's text needs (the shared fin-core rule) is in its coverage.
    for spec in [&tf::SANS, &tf::RTL, &tf::HAN] {
        let covered = info(&tf::build(spec)).remove(0).scripts;
        let needed = fin_core::layout::scripts_of_text(spec.chars);
        let needed: Vec<String> = needed
            .into_iter()
            .filter(|s| !covered.contains(s))
            .collect();
        assert!(
            needed.is_empty() || spec.family == "TestHan",
            "{}: {needed:?}",
            spec.family
        );
    }

    let err = fin_text::font_info(b"definitely not a font").unwrap_err();
    assert_eq!(err.code, "FONT_INVALID");
}

#[test]
fn requested_weight_drives_wght() {
    let data = tf::test_variable();
    let mut bytes = MapBytes::default();
    let sha = bytes.add(data.clone());
    let face = face_ref(&sha, &data, 0);
    assert_eq!((face.family.as_str(), face.weight), ("TestVar", 400));
    let m = Measurer::new(Arc::new(bytes));
    let width = |weight: u16| {
        let mut st = style(100.0);
        st.weight = weight;
        layout(&m, &req(&["aa"], vec![face.clone()], st)).words[0].width
    };
    // `a` advances 550 units at the default; +200 units at wght 900, linearly from 400 (its
    // outline, 50..500, does not move); at 100 px one unit is 6.4/64 px. The ink of `aa` is
    // one advance and one outline less its side bearing: advance + 450.
    assert_eq!(width(400), (550 + 450) * 64 / 10);
    assert_eq!(
        width(300),
        (550 + 450) * 64 / 10,
        "no deltas below the default"
    );
    assert_eq!(width(700), (670 + 450) * 64 / 10);
    assert_eq!(width(900), (750 + 450) * 64 / 10);
}
