mod common;

use common::ids_json::{ids_cover_every_id_in_json, ids_cover_json};
use fin_core::{
    AssetAdd, AssetMove, BinAdd, BinMove, BinRemove, BlendMode, CaptionBlock, Curve, DeleteMode,
    Edge, Effect, InsertMode, Interp, Item, ItemBody, ItemDelete, ItemInsert, ItemLink, ItemMove,
    ItemRemove, ItemRestore, ItemSlide, ItemSlip, ItemSplit, ItemTrim, ItemUnlink, Keyframe,
    Marker, MarkerAdd, MarkerRemove, MarkerSet, Op, ParamSet, ParamValue, Rational, Resolution,
    SlideSides, Timeline, TimelineAdd, TimelineRemove, TimelineRestore, TimelineSet, Track,
    TrackAdd, TrackKind, TrackMove, TrackRemove, TrackRestore, TrackSet, Transition,
    TransitionKind, TransitionRemove, TransitionSet, TrimMode, Word,
};
use fin_core::{EffectAdd, EffectMove, EffectRemove, EffectRestore, KeyframeRemove, KeyframeSet};
use serde_json::json;

/// A clip item with everything that carries an id: effects, a link group, keyframes.
pub fn busy_clip() -> Item {
    let r = Rational::from_int;
    let mut item = Item::clip("c1", "a1", r(0), r(0), r(1));
    item.common.effects = vec![
        Effect {
            id: "fx1".into(),
            kind: "blur".into(),
            enabled: true,
            params: [("radius".to_string(), ParamValue::Number(2.5))].into(),
        },
        Effect {
            id: "fx2".into(),
            kind: "colorAdjust".into(),
            enabled: false,
            params: Default::default(),
        },
    ];
    item.common.link = Some("lk1".into());
    item.common.blend = BlendMode::Screen;
    item.common.keyframes.insert(
        "opacity".into(),
        vec![Keyframe {
            at: r(0),
            value: ParamValue::Number(0.5),
            interp: Interp::Linear,
        }],
    );
    item
}

/// A caption item in project style `st2`.
pub fn caption() -> Item {
    let r = Rational::from_int;
    let body = ItemBody::Caption {
        block: CaptionBlock {
            words: vec![Word {
                text: "hi".into(),
                start: r(0),
                end: r(1),
            }],
            style: "st2".into(),
            overrides: Default::default(),
            source: None,
        },
    };
    let mut item = Item::new("cap1", r(0), r(1), body);
    item.common.effects.push(Effect {
        id: "fx3".into(),
        kind: "blur".into(),
        enabled: true,
        params: Default::default(),
    });
    item
}

/// A track holding [`busy_clip`], a caption and a transition between two other clips.
pub fn busy_track() -> Track {
    let r = Rational::from_int;
    let mut t = Track::new("t1", TrackKind::Video, "V1");
    t.items = vec![
        busy_clip(),
        Item::clip("c2", "a1", r(1), r(0), r(1)),
        Item::clip("c3", "a2", r(2), r(0), r(1)),
    ];
    t.transitions.push(Transition {
        id: "tr1".into(),
        between: ["c2".into(), "c3".into()],
        kind: TransitionKind::Dissolve,
        duration: r(1),
        curve: Curve::Linear,
    });
    t
}

/// A timeline with a marker, [`busy_track`] and a caption track.
pub fn busy_timeline() -> Timeline {
    let r = Rational::from_int;
    let mut captions = Track::new("t4", TrackKind::Caption, "C1");
    captions.items.push(caption());
    Timeline {
        id: "tl2".into(),
        name: "B-roll".into(),
        rate: r(30),
        resolution: Resolution {
            width: 1080,
            height: 1920,
        },
        markers: vec![Marker {
            id: "m3".into(),
            at: r(1),
            duration: None,
            label: "Cue".into(),
            color: None,
        }],
        tracks: vec![busy_track(), captions],
    }
}

fn every_variant() -> Vec<Op> {
    let r = Rational::from_int;
    let mut add = AssetAdd::media("$a", "a1.mov", r(2), None, None, "c9");
    add.bin = Some("b1".into());
    let ops: Vec<Op> = vec![
        add.into(),
        ItemInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: r(0),
            duration: r(1),
            mode: InsertMode::Ripple,
            body: ItemBody::Image { asset: "a1".into() },
        }
        .into(),
        ItemTrim {
            item: "c1".into(),
            edge: Edge::Start,
            delta: r(1),
            mode: TrimMode::Ripple,
            alone: true,
        }
        .into(),
        ItemDelete {
            item: "c1".into(),
            mode: DeleteMode::Lift,
        }
        .into(),
        ItemSlip {
            item: "c1".into(),
            delta: r(1),
        }
        .into(),
        ItemSplit {
            item: "c1".into(),
            offset: r(1),
            id: "$r".into(),
        }
        .into(),
        ItemMove {
            item: "c1".into(),
            track: "t2".into(),
            at: r(3),
            mode: InsertMode::Overwrite,
        }
        .into(),
        ItemSlide {
            item: "c1".into(),
            delta: r(1),
            sides: Some(SlideSides {
                before: Some("c2".into()),
                after: Some("c3".into()),
            }),
        }
        .into(),
        ItemLink {
            items: vec!["c1".into(), "$c".into()],
            id: "$lk".into(),
        }
        .into(),
        ItemUnlink {
            group: "lk1".into(),
            items: Some(vec!["c4".into()]),
        }
        .into(),
        TransitionSet {
            id: "$tr".into(),
            track: "t1".into(),
            between: ["c1".into(), "$c".into()],
            kind: TransitionKind::Dissolve,
            duration: r(1),
            curve: fin_core::Curve::EqualPower,
        }
        .into(),
        TransitionRemove {
            transition: "tr1".into(),
        }
        .into(),
        TrackAdd {
            id: "$t".into(),
            timeline: "tl1".into(),
            kind: TrackKind::Audio,
            index: Some(0),
            name: Some("Music".into()),
        }
        .into(),
        TrackRemove {
            track: "t1".into(),
            with_items: true,
        }
        .into(),
        TrackMove {
            track: "t1".into(),
            index: 1,
        }
        .into(),
        TrackSet {
            track: "t1".into(),
            path: "gain".into(),
            value: Some(ParamValue::Number(-3.0)),
        }
        .into(),
        TimelineAdd {
            id: "$tl".into(),
            name: "B-roll".into(),
            rate: r(25),
            resolution: Resolution {
                width: 1920,
                height: 1080,
            },
        }
        .into(),
        TimelineSet {
            timeline: "tl1".into(),
            path: "name".into(),
            value: None,
        }
        .into(),
        TimelineRemove {
            timeline: "tl2".into(),
        }
        .into(),
        MarkerAdd {
            id: "$m".into(),
            timeline: "tl1".into(),
            at: r(2),
            duration: Some(r(1)),
            label: "Cue".into(),
            color: Some("#ff0000".into()),
        }
        .into(),
        MarkerSet {
            marker: "m1".into(),
            path: "label".into(),
            value: Some(ParamValue::Text("Intro".into())),
        }
        .into(),
        MarkerRemove {
            marker: "m1".into(),
        }
        .into(),
        BinAdd {
            id: "$b".into(),
            name: "Footage".into(),
            parent: Some("b1".into()),
        }
        .into(),
        BinMove {
            bin: "b2".into(),
            parent: Some("b1".into()),
        }
        .into(),
        BinRemove { bin: "b2".into() }.into(),
        AssetMove {
            asset: "a1".into(),
            bin: Some("b1".into()),
        }
        .into(),
        TrackRestore {
            track: busy_track(),
            timeline: "tl1".into(),
            index: 0,
        }
        .into(),
        TimelineRestore {
            timeline: busy_timeline(),
            index: 1,
        }
        .into(),
        Op::AssetRemove { id: "a1".into() },
        ItemRemove {
            id: "c1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        ItemRestore {
            item: busy_clip(),
            track: "t1".into(),
            mode: DeleteMode::Ripple,
        }
        .into(),
        Op::ClipInsert {
            id: "$c".into(),
            track: "t1".into(),
            at: r(0),
            asset: "a1".into(),
            source_in: r(0),
            duration: r(1),
        },
        Op::ClipTrim {
            clip: "c1".into(),
            edge: Edge::Start,
            delta: r(1),
        },
        Op::ClipDelete { clip: "c1".into() },
        ParamSet {
            item: "c1".into(),
            path: "effects.fx1.asset".into(),
            value: Some(ParamValue::Asset { asset: "a2".into() }),
        }
        .into(),
        EffectAdd {
            item: "c1".into(),
            id: "$fx".into(),
            kind: "lut".into(),
            at: Some(0),
            params: [(
                "asset".to_string(),
                ParamValue::Asset { asset: "a2".into() },
            )]
            .into(),
        }
        .into(),
        EffectRemove {
            item: "c1".into(),
            effect: "fx1".into(),
        }
        .into(),
        EffectMove {
            item: "c1".into(),
            effect: "fx1".into(),
            to: 1,
        }
        .into(),
        KeyframeSet {
            item: "c1".into(),
            path: "opacity".into(),
            at: r(1),
            value: ParamValue::Number(0.5),
            interp: Interp::Linear,
        }
        .into(),
        KeyframeRemove {
            item: "c1".into(),
            path: "opacity".into(),
            at: r(1),
        }
        .into(),
        EffectRestore {
            item: "c1".into(),
            effect: Effect {
                id: "fx4".into(),
                kind: "lut".into(),
                enabled: true,
                params: [(
                    "asset".to_string(),
                    ParamValue::Asset { asset: "a3".into() },
                )]
                .into(),
            },
            index: 0,
            keyframes: [(
                "effects.fx4.intensity".to_string(),
                vec![Keyframe {
                    at: r(0),
                    value: ParamValue::Number(1.0),
                    interp: Interp::Hold,
                }],
            )]
            .into(),
        }
        .into(),
    ];
    let mut ops = ops;
    ops.extend(styles_and_fonts());
    // Adding a variant breaks this match, so the list above cannot silently fall behind.
    for op in &ops {
        match op {
            Op::AssetAdd(_)
            | Op::ItemInsert(_)
            | Op::ItemTrim(_)
            | Op::ItemDelete(_)
            | Op::ItemSlip(_)
            | Op::ItemSplit(_)
            | Op::ItemMove(_)
            | Op::ItemSlide(_)
            | Op::ItemLink(_)
            | Op::ItemUnlink(_)
            | Op::TransitionSet(_)
            | Op::TransitionRemove(_)
            | Op::TrackAdd(_)
            | Op::TrackRemove(_)
            | Op::TrackMove(_)
            | Op::TrackSet(_)
            | Op::TimelineAdd(_)
            | Op::TimelineSet(_)
            | Op::TimelineRemove(_)
            | Op::MarkerAdd(_)
            | Op::MarkerSet(_)
            | Op::MarkerRemove(_)
            | Op::BinAdd(_)
            | Op::BinMove(_)
            | Op::BinRemove(_)
            | Op::AssetMove(_)
            | Op::TrackRestore(_)
            | Op::TimelineRestore(_)
            | Op::AssetRemove { .. }
            | Op::ItemRemove(_)
            | Op::ItemRestore(_)
            | Op::ClipInsert { .. }
            | Op::ClipTrim { .. }
            | Op::ClipDelete { .. }
            | Op::ParamSet(_)
            | Op::EffectAdd(_)
            | Op::EffectRemove(_)
            | Op::EffectMove(_)
            | Op::KeyframeSet(_)
            | Op::KeyframeRemove(_)
            | Op::EffectRestore(_)
            | Op::StyleAdd(_)
            | Op::StyleSet(_)
            | Op::StyleRemove(_)
            | Op::FontPin(_)
            | Op::FontUnpin(_)
            | Op::FontRepin(_)
            | Op::CaptionSetText(_)
            | Op::CaptionSetWords(_)
            | Op::CaptionSplit(_)
            | Op::CaptionMerge(_)
            | Op::CaptionSetStyle(_) => {}
        }
    }
    ops
}

/// The style and font ops (plan 4C Task 18): a style copied from a project style, an asset
/// pin; and the caption edits (Task 19).
fn styles_and_fonts() -> Vec<Op> {
    use fin_core::{FontFormat, FontPin, FontPinOp, FontRepin, FontSource, FontUnpin};
    use fin_core::{StyleAdd, StyleRemove, StyleSet};
    let pin = FontPin {
        family: "Test Sans".into(),
        weight: 400,
        italic: false,
        face: 0,
        sha256: "ab".repeat(32),
        size: 10,
        format: FontFormat::Ttf,
        scripts: vec!["Latn".into()],
        source: FontSource::Asset { asset: "a4".into() },
    };
    let key = "Test Sans|400|normal".to_string();
    vec![
        StyleAdd {
            id: "$st".into(),
            name: "Big".into(),
            from: Some("st2".into()),
            set: [("text.size".to_string(), ParamValue::Number(80.0))].into(),
        }
        .into(),
        StyleSet {
            style: "st2".into(),
            path: "color".into(),
            value: Some(ParamValue::Text("#ff0000".into())),
        }
        .into(),
        StyleRemove {
            style: "st2".into(),
        }
        .into(),
        FontPinOp {
            key: key.clone(),
            pin: pin.clone(),
            fallback: Some(0),
        }
        .into(),
        FontUnpin { key: key.clone() }.into(),
        FontRepin { key, pin }.into(),
        fin_core::CaptionSetText {
            item: "cap1".into(),
            text: "Hello there".into(),
        }
        .into(),
        fin_core::CaptionSetWords {
            item: "cap1".into(),
            words: vec![Word {
                text: "hi".into(),
                start: Rational::from_int(0),
                end: Rational::from_int(1),
            }],
        }
        .into(),
        fin_core::CaptionSplit {
            item: "cap1".into(),
            word: 1,
            id: "$cap".into(),
        }
        .into(),
        fin_core::CaptionMerge {
            item: "cap1".into(),
            with: "cap2".into(),
        }
        .into(),
        fin_core::CaptionSetStyle {
            item: "cap1".into(),
            style: "st2".into(),
            overrides: Some([("text.size".to_string(), ParamValue::Number(90.0))].into()),
        }
        .into(),
    ]
}

#[test]
fn ops_table_lists_every_variant() {
    let ops = every_variant();
    let mut names: Vec<&str> = ops.iter().map(Op::name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ops.len(), "names are unique");
    for op in &ops {
        let json = serde_json::to_value(op).unwrap();
        assert_eq!(json["op"], op.name());
        let back: Op = serde_json::from_value(json).unwrap();
        assert_eq!(&back, op);
        ids_cover_every_id_in_json(op);
        let mut m = op.clone();
        let mut refs: Vec<String> = Vec::new();
        m.for_each_ref(|r| refs.push(r.clone()));
        assert_eq!(
            op.ids(),
            refs,
            "{}: ids lists what for_each_ref reaches",
            op.name()
        );
        assert_eq!(op.is_alias(), op.name().starts_with("clip."));
    }
}

/// Decision 24: a restore lists every id nested in its item (B6).
#[test]
fn restore_lists_nested_ids() {
    let ops: Vec<Op> = [busy_clip(), caption()]
        .into_iter()
        .map(|item| {
            ItemRestore {
                item,
                track: "t3".into(),
                mode: DeleteMode::Lift,
            }
            .into()
        })
        .collect();
    assert_eq!(
        ops[0].ids(),
        ["c1", "a1", "fx1", "fx2", "lk1", "t3"],
        "{:?}",
        ops[0]
    );
    assert_eq!(ops[1].ids(), ["cap1", "st2", "fx3", "t3"]);
    for op in &ops {
        ids_cover_every_id_in_json(op);
    }
}

/// Decision 24: the track and timeline restores list every id nested in them, transitions and
/// the items they sit between included (B6).
#[test]
fn track_and_timeline_restores_list_nested_ids() {
    let track: Op = TrackRestore {
        track: busy_track(),
        timeline: "tl1".into(),
        index: 0,
    }
    .into();
    assert_eq!(
        track.ids(),
        [
            "t1", "c1", "a1", "fx1", "fx2", "lk1", "c2", "a1", "c3", "a2", "tr1", "c2", "c3", "tl1"
        ]
    );
    let timeline: Op = TimelineRestore {
        timeline: busy_timeline(),
        index: 1,
    }
    .into();
    let ids = timeline.ids();
    assert_eq!(ids[..3], ["tl2", "m3", "t1"]);
    assert_eq!(ids[ids.len() - 4..], ["t4", "cap1", "st2", "fx3"]);
    for op in [&track, &timeline] {
        ids_cover_every_id_in_json(op);
        // Renaming through refs_mut reaches every one of them.
        let mut renamed = op.clone();
        for r in renamed.refs_mut() {
            r.push('9');
        }
        assert!(renamed.ids().iter().all(|id| id.ends_with('9')));
        let json = serde_json::to_string(&renamed).unwrap();
        for id in op.ids() {
            assert!(!json.contains(&format!("\"{id}\"")), "{id} left in {json}");
        }
    }
}

#[test]
fn ids_json_helper_flags_an_unlisted_id() {
    let op = Op::ClipDelete { clip: "c1".into() };
    let mut json = serde_json::to_value(&op).unwrap();
    assert_eq!(ids_cover_json(&op, &json), Ok(()));
    json["other"] = json!("c9");
    assert_eq!(ids_cover_json(&op, &json), Err(vec!["c9".to_string()]));
    // Free-text fields are not ids.
    let mut json = serde_json::to_value(&op).unwrap();
    json["label"] = json!("c9");
    assert_eq!(ids_cover_json(&op, &json), Ok(()));
    // An id nested in a restored item is found.
    let mut item = busy_clip();
    item.common.link = None;
    let op: Op = ItemRestore {
        item,
        track: "t1".into(),
        mode: DeleteMode::Ripple,
    }
    .into();
    let mut json = serde_json::to_value(&op).unwrap();
    json["item"]["link"] = json!("lk7");
    assert_eq!(ids_cover_json(&op, &json), Err(vec!["lk7".to_string()]));
}
