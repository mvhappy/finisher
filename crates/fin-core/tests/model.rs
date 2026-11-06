use std::collections::BTreeMap;

use fin_core::{
    AudioParams, BlendMode, CaptionBlock, CaptionSource, Common, Crop, DEFAULT_RESOLUTION, Effect,
    FontFormat, FontPin, FontSource, Fonts, ID_KINDS, IdAlloc, IdKind, Interp, Item, ItemBody,
    Keyframe, Mask, MemIds, ParamValue, Project, Rational, Resolution, SCHEMA_VERSION, TrackKind,
    Transform, Word, font_key, json_schema, new_project,
};
use serde_json::json;

fn rate() -> Rational {
    "30000/1001".parse().unwrap()
}

fn r(s: &str) -> Rational {
    s.parse().unwrap()
}

#[test]
fn new_project_has_default_timeline_and_tracks() {
    let p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    assert_eq!(p.timelines.len(), 1);
    let tl = &p.timelines[0];
    assert_eq!((tl.id.as_str(), tl.name.as_str()), ("tl1", "Timeline 1"));
    assert_eq!(tl.rate, rate());
    assert_eq!(
        tl.resolution,
        Resolution {
            width: 1080,
            height: 1920
        }
    );
    assert_eq!(tl.resolution, DEFAULT_RESOLUTION);
    let tracks: Vec<_> = tl
        .tracks
        .iter()
        .map(|t| (t.id.as_str(), t.kind, t.name.as_str()))
        .collect();
    assert_eq!(
        tracks,
        [
            ("t1", TrackKind::Video, "V1"),
            ("t2", TrackKind::Audio, "A1")
        ]
    );
    for t in &tl.tracks {
        assert!(!t.mute && !t.solo && !t.lock && t.gain == 0.0 && t.transitions.is_empty());
    }
    assert!(tl.markers.is_empty());
    assert_eq!(p.rate, rate());
    assert_eq!(p.schema_version, SCHEMA_VERSION);
    assert_eq!(SCHEMA_VERSION, 2);
    assert!(p.bins.is_empty() && p.assets.is_empty() && p.styles.is_empty());
    assert!(p.fonts.is_empty());
}

/// A rev-0 project encodes to exactly these bytes: no bins, styles or fonts, no track flags,
/// no markers (plan 4C Decision 11).
#[test]
fn defaults_are_omitted() {
    let p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    let golden = concat!(
        r#"{"id":"p1","name":"Demo","schemaVersion":2,"rate":"30000/1001","timelines":["#,
        r#"{"id":"tl1","name":"Timeline 1","rate":"30000/1001","#,
        r#""resolution":{"width":1080,"height":1920},"tracks":["#,
        r#"{"id":"t1","kind":"video","name":"V1","items":[]},"#,
        r#"{"id":"t2","kind":"audio","name":"A1","items":[]}]}],"assets":{}}"#
    );
    assert_eq!(serde_json::to_string(&p).unwrap(), golden);
    let item = Item::clip("c1", "a1", r("0"), r("0"), r("1"));
    assert_eq!(
        serde_json::to_string(&item).unwrap(),
        r#"{"id":"c1","start":"0","duration":"1","kind":"clip","asset":"a1","sourceIn":"0"}"#
    );
}

#[test]
fn project_json_round_trips_byte_identically() {
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    p.timelines[0].tracks[0].items.push(Item::clip(
        "c1",
        "a1",
        Rational::zero(),
        r("1/2"),
        r("62/5"),
    ));
    let json = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert!(json.contains("\"schemaVersion\":2") && json.contains("\"sourceIn\":\"1/2\""));
}

/// Common fields away from every default, so each one is written.
fn busy_common() -> Common {
    Common {
        transform: Transform {
            position: [-12.5, 3.0],
            scale: [2.0, 0.5],
            rotation: -0.0,
            anchor: [0.0, 1.0],
        },
        crop: Crop {
            left: 0.1,
            right: 0.0,
            top: 0.25,
            bottom: 1e-300,
        },
        opacity: 0.75,
        blend: BlendMode::Screen,
        effects: vec![Effect {
            id: "fx3".into(),
            kind: "blur".into(),
            enabled: false,
            params: BTreeMap::from([("radius".into(), ParamValue::Number(4.5))]),
        }],
        mask: Some(Mask {
            matte: "art1".into(),
            feather: 2.0,
            choke: 0.0,
            invert: true,
        }),
        audio: Some(AudioParams {
            gain: -6.0,
            pan: 0.25,
            fade_in: r("1/2"),
            fade_out: r("0"),
        }),
        keyframes: BTreeMap::from([(
            "opacity".into(),
            vec![
                Keyframe {
                    at: r("0"),
                    value: ParamValue::Number(0.0),
                    interp: Interp::Hold,
                },
                Keyframe {
                    at: r("1"),
                    value: ParamValue::Vec2([1.0, -2.0]),
                    interp: Interp::Bezier {
                        x1: 0.25,
                        y1: 0.1,
                        x2: 0.25,
                        y2: 1.0,
                    },
                },
                Keyframe {
                    at: r("2"),
                    value: ParamValue::Bool(true),
                    interp: Interp::Linear,
                },
            ],
        )]),
        link: Some("lk2".into()),
    }
}

fn item(id: &str, body: ItemBody, common: Common) -> Item {
    Item {
        id: id.into(),
        start: r("3/2"),
        duration: r("2"),
        body,
        common,
    }
}

fn every_kind() -> Vec<Item> {
    let caption = CaptionBlock {
        words: vec![Word {
            text: "hello".into(),
            start: r("0"),
            end: r("1/2"),
        }],
        style: "preset:clean".into(),
        overrides: BTreeMap::from([("fill".into(), ParamValue::Text("#ffffff".into()))]),
        source: Some(CaptionSource {
            artifact: "art2".into(),
            word_range: [0, 1],
        }),
    };
    vec![
        item(
            "c1",
            ItemBody::Clip {
                asset: "a1".into(),
                source_in: r("1/3"),
                speed: r("2"),
            },
            busy_common(),
        ),
        item(
            "cap1",
            ItemBody::Caption { block: caption },
            Common::default(),
        ),
        item(
            "g1",
            ItemBody::Graphic {
                asset: "a2".into(),
                slots: BTreeMap::from([("title".into(), ParamValue::Text("Hi".into()))]),
            },
            Common::default(),
        ),
        item(
            "img1",
            ItemBody::Image { asset: "a3".into() },
            busy_common(),
        ),
        item("adj1", ItemBody::Adjustment {}, busy_common()),
        item(
            "nst1",
            ItemBody::Nested {
                timeline: "tl2".into(),
                source_in: r("0"),
            },
            Common::default(),
        ),
    ]
}

#[test]
fn item_json_round_trips_every_kind() {
    for it in every_kind() {
        let json = serde_json::to_string(&it).unwrap();
        let back: Item = serde_json::from_str(&json).unwrap();
        assert_eq!(back, it, "{json}");
        assert_eq!(serde_json::to_string(&back).unwrap(), json);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["kind"], it.kind_name(), "{json}");
        // `-0.0` is not the default `0.0`: it is written and reads back as itself.
        if it.common.transform.rotation.to_bits() == (-0.0f64).to_bits() {
            assert!(json.contains(r#""rotation":-0.0"#), "{json}");
            assert!(back.common.transform.rotation.is_sign_negative());
        }
    }
    // Every common default is omitted; the clip's speed 1 too.
    let plain = item(
        "c2",
        ItemBody::Clip {
            asset: "a1".into(),
            source_in: r("0"),
            speed: r("1"),
        },
        Common::default(),
    );
    assert_eq!(
        serde_json::to_value(&plain).unwrap(),
        json!({ "id": "c2", "start": "3/2", "duration": "2", "kind": "clip", "asset": "a1",
                "sourceIn": "0" })
    );
    assert!(Common::default().is_default() && !busy_common().is_default());
}

/// A clip item's constructor, and its ids (plan 4C Decision 24): its own, its asset, its
/// effects' and its link group's.
#[test]
fn clip_constructor_and_item_ids() {
    let it = every_kind().remove(0);
    let mut plain = Item::clip("c1", "a1", r("0"), r("0"), r("8"));
    assert_eq!(plain.kind_name(), "clip");
    assert_eq!(plain.ids(), ["c1", "a1"]);
    let mut busy = it.clone();
    let listed: Vec<String> = busy.ids().iter().map(|s| s.to_string()).collect();
    let refs: Vec<String> = busy.refs_mut().into_iter().map(|s| s.clone()).collect();
    assert_eq!(listed, refs);
    for r in plain.refs_mut() {
        r.push('x');
    }
    assert_eq!(plain.ids(), ["c1x", "a1x"]);
}

#[test]
fn fonts_registry_round_trips() {
    let pin = |source| FontPin {
        family: "Noto Sans".into(),
        weight: 700,
        italic: false,
        face: 0,
        sha256: "ab".repeat(32),
        size: 123_456,
        format: FontFormat::Ttf,
        scripts: vec!["Cyrl".into(), "Latn".into()],
        source,
    };
    let fonts = Fonts {
        pins: BTreeMap::from([
            (
                font_key("Noto Sans", 700, false),
                pin(FontSource::Catalog {
                    entry: "noto-sans".into(),
                    style: "bold".into(),
                    catalog: "2026.11.1".into(),
                }),
            ),
            (
                font_key("Brand", 400, true),
                FontPin {
                    format: FontFormat::Ttc,
                    face: 2,
                    ..pin(FontSource::Asset { asset: "a4".into() })
                },
            ),
        ]),
        fallback: vec![font_key("Noto Sans", 700, false)],
    };
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    p.fonts = fonts.clone();
    let json = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back.fonts, fonts);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    let v = serde_json::to_value(&fonts).unwrap();
    assert_eq!(
        v["pins"]["Noto Sans|700|normal"]["source"],
        json!({ "type": "catalog", "entry": "noto-sans", "style": "bold", "catalog": "2026.11.1" })
    );
    assert_eq!(
        v["pins"]["Brand|400|italic"]["source"],
        json!({ "type": "asset", "asset": "a4" })
    );
    assert_eq!(v["pins"]["Brand|400|italic"]["format"], "ttc");
    assert_eq!(v["fallback"], json!(["Noto Sans|700|normal"]));
}

#[test]
fn font_key_is_canonical() {
    assert_eq!(font_key("Noto Sans", 400, false), "Noto Sans|400|normal");
    assert_eq!(font_key("Inter", 700, true), "Inter|700|italic");
    // The family is kept as written: no case folding or trimming.
    assert_eq!(font_key(" inter ", 50, false), " inter |50|normal");
    assert_ne!(font_key("Inter", 400, false), font_key("inter", 400, false));
}

#[test]
fn schema_has_the_contract_defs() {
    let schema = json_schema();
    let defs = schema["$defs"].as_object().unwrap();
    for name in [
        "Project",
        "Tx",
        "Op",
        "Diff",
        "CoreError",
        "CommittedTx",
        "Timeline",
        "Item",
        "Fonts",
        "FontPin",
        "Asset",
    ] {
        assert!(defs.contains_key(name), "missing {name}");
    }
    assert_eq!(defs["Rational"]["type"], "string");
    assert_eq!(schema, json_schema(), "schema must be deterministic");
}

#[test]
fn mem_ids_continue_after_a_project() {
    let p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    let mut ids = MemIds::after(&p);
    assert_eq!(ids.next(IdKind::Timeline), "tl2");
    assert_eq!(ids.next(IdKind::Track), "t3");
    assert_eq!(ids.next(IdKind::Track), "t4");
    assert_eq!(ids.next(IdKind::Clip), "c1");
    assert_eq!(ids.next(IdKind::Asset), "a1");
}

#[test]
fn mem_ids_skip_past_highest_suffix() {
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    p.timelines[0].tracks[0].items.push(Item::clip(
        "c7",
        "a9",
        Rational::zero(),
        Rational::zero(),
        Rational::from_int(1),
    ));
    let mut ids = MemIds::after(&p);
    assert_eq!(ids.next(IdKind::Clip), "c8");
}

/// `MemIds::after` sees every entity that has an id (plan 4C Task 6).
#[test]
fn ids_after_counts_every_kind() {
    use fin_core::{Bin, CaptionStyle, Marker, Timeline, Track, Transition, TransitionKind};
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    let mut tl2 = Timeline {
        id: "tl4".into(),
        name: "B".into(),
        ..p.timelines[0].clone()
    };
    tl2.tracks = vec![Track::new("t9", TrackKind::Caption, "C1")];
    tl2.markers.push(Marker {
        id: "m5".into(),
        at: r("1"),
        duration: None,
        label: "x".into(),
        color: None,
    });
    let mut items = every_kind();
    for (it, id) in items
        .iter_mut()
        .zip(["c3", "cap6", "g2", "img8", "adj4", "nst7"])
    {
        it.id = id.into();
    }
    items[0].common.effects[0].id = "fx11".into();
    items[0].common.link = Some("lk12".into());
    tl2.tracks[0].items = items;
    tl2.tracks[0].transitions.push(Transition {
        id: "tr13".into(),
        between: ["c3".into(), "cap6".into()],
        kind: TransitionKind::Crossfade,
        duration: r("1"),
        curve: Default::default(),
    });
    p.timelines.push(tl2);
    p.bins.insert(
        "b14".into(),
        Bin {
            id: "b14".into(),
            name: "Bin".into(),
            parent: None,
        },
    );
    p.styles.insert("st15".into(), CaptionStyle::default());
    let mut ids = MemIds::after(&p);
    let next: Vec<String> = ID_KINDS.iter().map(|k| ids.next(*k)).collect();
    assert_eq!(
        next,
        [
            "a1", "t10", "c4", "tl5", "br1", "cap7", "g3", "img9", "adj5", "nst8", "m6", "tr14",
            "b15", "fx12", "lk13", "st16"
        ]
    );
}

/// One prefix per kind (owner, plan 4C Decision 10), and no id names two kinds.
#[test]
fn new_id_prefixes_are_unambiguous() {
    let prefixes: Vec<&str> = ID_KINDS.iter().map(|k| k.prefix()).collect();
    assert_eq!(
        prefixes,
        [
            "a", "t", "c", "tl", "br", "cap", "g", "img", "adj", "nst", "m", "tr", "b", "fx", "lk",
            "st"
        ]
    );
    for (i, a) in prefixes.iter().enumerate() {
        assert!(a.bytes().all(|b| b.is_ascii_lowercase()), "{a}");
        for b in &prefixes[i + 1..] {
            assert_ne!(a, b);
        }
    }
    // An id `<prefix><digits>` is that kind's only: the allocator of every other kind would
    // never produce it, and `MemIds::after` raises that kind alone.
    for kind in ID_KINDS {
        let mut p = new_project("p".into(), "n".into(), rate()).unwrap();
        p.styles
            .insert(format!("{}41", kind.prefix()), Default::default());
        let mut ids = MemIds::after(&p);
        let mut fresh = MemIds::after(&new_project("p".into(), "n".into(), rate()).unwrap());
        for other in ID_KINDS {
            let (got, base) = (ids.next(other), fresh.next(other));
            if other == kind {
                assert_eq!(got, format!("{}42", kind.prefix()));
            } else {
                assert_eq!(got, base, "{} raised {}", kind.prefix(), other.prefix());
            }
        }
    }
    for (body, kind) in every_kind().iter().map(|i| &i.body).zip([
        IdKind::Clip,
        IdKind::Caption,
        IdKind::Graphic,
        IdKind::Image,
        IdKind::Adjustment,
        IdKind::Nested,
    ]) {
        assert_eq!(IdKind::of_item(body), kind);
        assert!(kind.is_item());
    }
}

#[test]
fn new_project_rejects_non_positive_rate() {
    for bad in [0, -30] {
        let e = new_project("p".into(), "n".into(), Rational::from_int(bad)).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
    }
}

#[test]
fn rational_to_f64() {
    assert_eq!(r("1/2").to_f64(), 0.5);
    assert_eq!(r("-3").to_f64(), -3.0);
    assert_eq!(r("30000/1001").to_f64(), 30000.0 / 1001.0);
    assert_eq!(Rational::default(), Rational::zero());
}

#[test]
fn schema_requires_details_and_keeps_rational_refs() {
    let schema = json_schema();
    let err = &schema["$defs"]["CoreError"];
    let required: Vec<&str> = err["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"details"), "{required:?}");
    assert!(err["properties"].get("details").is_some());
    for (def, field) in [
        ("Project", "rate"),
        ("Timeline", "rate"),
        ("Item", "start"),
        ("Item", "duration"),
        ("ItemInsert", "at"),
        ("ItemInsert", "duration"),
        ("ItemTrim", "delta"),
        ("Asset", "duration"),
        ("Marker", "duration"),
    ] {
        let f = &schema["$defs"][def]["properties"][field];
        assert_eq!(f["$ref"], "#/$defs/Rational", "{def}.{field}");
    }
}
