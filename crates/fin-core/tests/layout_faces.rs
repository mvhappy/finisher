//! What a project needs from fonts, and which pinned faces a caption is measured with (plan 4C
//! Task 18, Decisions 35–36): `layout::font_needs`, `faces_for`, `scripts_of`.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::captions::preset;
use fin_core::layout::{faces_for, font_needs, scripts_of};
use fin_core::{
    CaptionBlock, CaptionStyle, FaceRef, FontFormat, FontPin, FontSource, InsertMode, ItemBody,
    ItemInsert, Op, ParamValue, Project, StyleAdd, Track, TrackKind, Word, font_key,
};

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn pin(family: &str, weight: u16, sha256: &str, scripts: &[&str]) -> FontPin {
    FontPin {
        family: family.into(),
        weight,
        italic: false,
        face: u32::from(weight == 400 && family == "Noto Sans CJK"),
        sha256: sha256.into(),
        size: 10,
        format: FontFormat::Ttf,
        scripts: scripts.iter().map(|s| (*s).to_string()).collect(),
        source: FontSource::Catalog {
            entry: family.to_lowercase().replace(' ', "-"),
            style: weight.to_string(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn words(text: &str) -> Vec<Word> {
    text.split_whitespace()
        .enumerate()
        .map(|(i, t)| Word {
            text: t.into(),
            start: secs(i64::try_from(i).unwrap()),
            end: secs(i64::try_from(i).unwrap() + 1),
        })
        .collect()
}

fn caption(text: &str, overrides: &[(&str, ParamValue)]) -> Op {
    let ws = words(text);
    let duration = secs(i64::try_from(ws.len()).unwrap());
    ItemInsert {
        id: "$cap".into(),
        track: "t3".into(),
        at: secs(100),
        duration,
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: ws,
                style: "preset:bold".into(),
                overrides: overrides
                    .iter()
                    .map(|(k, v)| ((*k).to_string(), v.clone()))
                    .collect(),
                source: None,
            },
        },
    }
    .into()
}

/// Inter Bold as the primary, and a fallback order of Noto Sans, Arabic, CJK and emoji.
fn pinned() -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    let pins = [
        pin("Inter", 700, &sha('a'), &["Cyrl", "Grek", "Latn"]),
        pin("Noto Sans", 700, &sha('b'), &["Cyrl", "Grek", "Latn"]),
        pin("Noto Sans Arabic", 700, &sha('c'), &["Arab"]),
        pin(
            "Noto Sans CJK",
            700,
            &sha('d'),
            &["Hang", "Hani", "Hira", "Kana"],
        ),
        pin("Noto Color Emoji", 400, &sha('e'), &["Zsye"]),
    ];
    for (i, pin) in pins.into_iter().enumerate() {
        let key = font_key(&pin.family, pin.weight, pin.italic);
        if i > 0 {
            p.fonts.fallback.push(key.clone());
        }
        p.fonts.pins.insert(key, pin);
    }
    p
}

fn face(p: &Project, key: &str) -> FaceRef {
    let pin = &p.fonts.pins[key];
    FaceRef {
        sha256: pin.sha256.clone(),
        face: pin.face,
        family: pin.family.clone(),
        weight: pin.weight,
        italic: pin.italic,
    }
}

#[test]
fn scripts_of_words() {
    let s = scripts_of(&words("Hi Привет 1, 😀 ©"));
    assert_eq!(s.into_iter().collect::<Vec<_>>(), ["Cyrl", "Latn", "Zsye"]);
}

#[test]
fn font_needs_lists_styles_and_scripts() {
    let p = pinned();
    let add = StyleAdd {
        id: "$st".into(),
        name: "Light".into(),
        from: None,
        set: [("text.weight".to_string(), ParamValue::Number(300.0))].into(),
    };
    let ops = vec![
        add.into(),
        caption("Hi мир", &[]),
        caption("中文 😀", &[("text.italic", ParamValue::Bool(true))]),
    ];
    let mut ops2 = ops;
    if let Op::ItemInsert(i) = &mut ops2[2] {
        i.at = secs(200);
        i.id = "$cap2".into();
    }
    let p = apply(&p, ops2).project;
    let needs = font_needs(&p);
    let bold = &preset("preset:bold").unwrap().text;
    let faces: Vec<(String, u16, bool)> = needs.faces.into_iter().collect();
    let mut want = vec![
        ("Inter".to_string(), 300, false),
        (bold.family.clone(), bold.weight, false),
        (bold.family.clone(), bold.weight, true),
    ];
    want.sort();
    want.dedup();
    assert_eq!(faces, want);
    assert_eq!(
        needs.scripts.into_iter().collect::<Vec<_>>(),
        ["Cyrl", "Hani", "Latn", "Zsye"]
    );
    // Presets no caption uses are not needed.
    assert!(font_needs(&project()).faces.is_empty());
}

#[test]
fn faces_for_orders_primary_then_matching_fallbacks() {
    let p = pinned();
    let style = CaptionStyle::default(); // Inter Bold
    let inter = font_key("Inter", 700, false);
    let got = faces_for(&p, &style, &words("Hi 中文 😀")).unwrap();
    let want: Vec<FaceRef> = [
        inter.as_str(),
        "Noto Sans|700|normal",
        "Noto Sans CJK|700|normal",
        "Noto Color Emoji|400|normal",
    ]
    .iter()
    .map(|k| face(&p, k))
    .collect();
    assert_eq!(got, want);
    // Only the fallbacks whose scripts meet the text's, in the project's order.
    let mut q = p.clone();
    q.fonts.fallback.reverse();
    let got = faces_for(&q, &style, &words("مرحبا 😀")).unwrap();
    assert_eq!(
        got,
        [
            face(&q, &inter),
            face(&q, "Noto Color Emoji|400|normal"),
            face(&q, "Noto Sans Arabic|700|normal"),
        ]
    );
    // Digits and punctuation need nothing beyond the primary.
    assert_eq!(
        faces_for(&p, &style, &words("12, 3!")).unwrap(),
        [face(&p, &inter)]
    );
}

#[test]
fn faces_for_reports_unpinned_keys() {
    let mut p = pinned();
    let style = CaptionStyle {
        text: fin_core::TextStyle {
            weight: 900,
            ..CaptionStyle::default().text
        },
        ..CaptionStyle::default()
    };
    assert_eq!(
        faces_for(&p, &style, &words("Hi")).unwrap_err(),
        ["Inter|900|normal"]
    );
    // A script no pinned face covers is reported as `script:<code>`.
    p.fonts.fallback.retain(|k| !k.contains("Arabic"));
    assert_eq!(
        faces_for(&p, &CaptionStyle::default(), &words("مرحبا")).unwrap_err(),
        ["script:Arab"]
    );
    assert_eq!(
        faces_for(&p, &style, &words("Hi مرحبا")).unwrap_err(),
        ["Inter|900|normal", "script:Arab"]
    );
    let _: BTreeMap<String, ParamValue> = BTreeMap::new();
}
