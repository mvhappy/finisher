//! `eval_frame` (plan 4C Task 15) over projects built literally from model types.

mod eval_common;

use eval_common::*;
use fin_core::eval::{
    ItemKind, Layer, MaskPlan, NestedPlan, PlanCrop, PlanTransform, PlanValue, Role, SourceRef,
    UnmeasuredReason, eval_frame,
};
use fin_core::{
    BlendMode, CoreError, Crop, Curve, Interp, Item, ItemBody, Mask, Measured, ParamValue, Project,
    Rational, TextMeasurer, TextRequest, TrackKind, TransitionKind,
};
use proptest::prelude::*;

fn frame(p: &Project, t: &str) -> Vec<Layer> {
    eval_frame(p, "tl1", r(t), None).unwrap().layers
}

fn one(p: &Project, t: &str) -> Layer {
    let mut layers = frame(p, t);
    assert_eq!(layers.len(), 1, "{layers:?}");
    layers.remove(0)
}

fn source(asset: &str, t: &str) -> Option<SourceRef> {
    Some(SourceRef {
        asset: asset.into(),
        time: r(t),
    })
}

#[test]
fn speed_maps_source_time() {
    let items = vec![
        with_speed(clip("c1", secs(2), secs(10), secs(4)), r("3/2")),
        with_speed(clip("c2", secs(6), secs(1), secs(4)), r("30000/1001")),
    ];
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, items)],
    )]);
    let l = one(&p, "3");
    assert_eq!(
        (l.item.as_str(), l.kind, l.local),
        ("c1", ItemKind::Clip, secs(1))
    );
    assert_eq!(l.source, source("a1", "23/2"));
    let l = one(&p, "7");
    assert_eq!(l.source, source("a1", "31001/1001"));
    // The end is exclusive; before the start nothing shows.
    assert_eq!(one(&p, "6").item, "c2");
    assert!(frame(&p, "1").is_empty());
    assert!(frame(&p, "10").is_empty());
}

#[test]
fn transition_layers_and_progress() {
    let mut t1 = track(
        "t1",
        TrackKind::Video,
        vec![
            clip("c1", secs(0), secs(5), secs(4)),
            clip("c2", secs(4), secs(20), secs(4)),
        ],
    );
    t1.transitions
        .push(crossfade("tr1", "c1", "c2", secs(1), Curve::EqualPower));
    let p = project_with(vec![timeline("tl1", vec![t1])]);

    let at = |t: &str| {
        frame(&p, t)
            .into_iter()
            .map(|l| {
                let tr = l
                    .transition
                    .map(|s| (s.id, s.kind, s.curve, s.progress, s.role));
                (l.item, l.local, l.source.unwrap().time, tr)
            })
            .collect::<Vec<_>>()
    };
    let state = |progress, role| {
        Some((
            "tr1".to_string(),
            TransitionKind::Crossfade,
            Curve::EqualPower,
            progress,
            role,
        ))
    };
    assert_eq!(at("3"), [("c1".into(), secs(3), secs(8), None)]);
    // The window is [cut − d/2, cut + d/2) = [7/2, 9/2): A continues, B starts early.
    assert_eq!(
        at("7/2"),
        [
            ("c1".into(), r("7/2"), r("17/2"), state(0.0, Role::Out)),
            ("c2".into(), r("-1/2"), r("39/2"), state(0.0, Role::In)),
        ]
    );
    assert_eq!(
        at("4"),
        [
            ("c1".into(), secs(4), secs(9), state(0.5, Role::Out)),
            ("c2".into(), secs(0), secs(20), state(0.5, Role::In)),
        ]
    );
    assert_eq!(
        at("17/4"),
        [
            ("c1".into(), r("17/4"), r("37/4"), state(0.75, Role::Out)),
            ("c2".into(), r("1/4"), r("81/4"), state(0.75, Role::In)),
        ]
    );
    assert_eq!(at("9/2"), [("c2".into(), r("1/2"), r("41/2"), None)]);
}

#[test]
fn invalid_transitions_are_skipped() {
    // Items that no longer touch, an unknown item, a zero duration: no window.
    let mut t1 = track(
        "t1",
        TrackKind::Video,
        vec![
            clip("c1", secs(0), secs(0), secs(4)),
            clip("c2", secs(5), secs(0), secs(4)),
        ],
    );
    t1.transitions
        .push(crossfade("tr1", "c1", "c2", secs(1), Curve::Linear));
    t1.transitions
        .push(crossfade("tr2", "c2", "c9", secs(1), Curve::Linear));
    t1.transitions
        .push(crossfade("tr3", "c1", "c2", secs(0), Curve::Linear));
    let p = project_with(vec![timeline("tl1", vec![t1])]);
    assert!(frame(&p, "41/10").is_empty());
    assert!(one(&p, "39/10").transition.is_none());
}

#[test]
fn nested_source_time_and_group_layer() {
    let inner = timeline(
        "tl3",
        vec![track(
            "t9",
            TrackKind::Video,
            vec![clip("c9", secs(0), secs(5), secs(30))],
        )],
    );
    let middle = timeline(
        "tl2",
        vec![track(
            "t8",
            TrackKind::Video,
            vec![nested("n2", secs(1), secs(20), "tl3", secs(2))],
        )],
    );
    let outer = timeline(
        "tl1",
        vec![track(
            "t1",
            TrackKind::Video,
            vec![nested("n1", secs(2), secs(5), "tl2", secs(3))],
        )],
    );
    let p = project_with(vec![outer, middle, inner]);
    let l = one(&p, "4");
    assert_eq!(
        (l.kind, l.local, l.source.clone()),
        (ItemKind::Nested, secs(2), None)
    );
    // tl2 at sourceIn + local = 5; n2 local 4; tl3 at 2 + 4 = 6; c9 reads 5 + 6.
    let res = fin_core::Resolution {
        width: 1080,
        height: 1920,
    };
    assert_eq!(
        l.nested,
        Some(NestedPlan {
            timeline: "tl2".into(),
            time: secs(5),
            resolution: res
        })
    );
    let kids = l.children.unwrap();
    assert_eq!(kids.len(), 1);
    assert_eq!(
        (kids[0].item.as_str(), kids[0].track.as_str(), kids[0].local),
        ("n2", "t8", secs(4))
    );
    assert_eq!(kids[0].nested.as_ref().unwrap().time, secs(6));
    let grand = kids[0].children.as_ref().unwrap();
    assert_eq!(
        (grand[0].item.as_str(), grand[0].source.clone()),
        ("c9", source("a1", "11"))
    );
    // Another timeline can be evaluated on its own.
    let plan = eval_frame(&p, "tl3", secs(1), None).unwrap();
    assert_eq!(
        (plan.timeline.as_str(), plan.layers[0].source.clone()),
        ("tl3", source("a1", "6"))
    );
}

fn nested(
    id: &str,
    start: Rational,
    duration: Rational,
    timeline: &str,
    source_in: Rational,
) -> Item {
    let body = ItemBody::Nested {
        timeline: timeline.into(),
        source_in,
    };
    Item::new(id, start, duration, body)
}

#[test]
fn nested_cycle_is_refused() {
    let a = timeline(
        "tl1",
        vec![track(
            "t1",
            TrackKind::Video,
            vec![nested("n1", secs(0), secs(9), "tl2", secs(0))],
        )],
    );
    let b = timeline(
        "tl2",
        vec![track(
            "t2",
            TrackKind::Video,
            vec![nested("n2", secs(0), secs(9), "tl1", secs(0))],
        )],
    );
    let p = project_with(vec![a, b]);
    let e = eval_frame(&p, "tl1", secs(1), None).unwrap_err();
    assert_eq!(e.code, "NESTED_CYCLE");
    assert_eq!(
        e.details["timelines"],
        serde_json::json!(["tl1", "tl2", "tl1"])
    );
    // Outside the nested items there is no cycle to meet.
    assert!(
        eval_frame(&p, "tl1", secs(10), None)
            .unwrap()
            .layers
            .is_empty()
    );

    // A chain deeper than 16 levels is refused too.
    let n = 18;
    let tls = (1..=n)
        .map(|i| {
            let items = if i < n {
                vec![nested(
                    &format!("n{i}"),
                    secs(0),
                    secs(9),
                    &format!("tl{}", i + 1),
                    secs(0),
                )]
            } else {
                vec![]
            };
            timeline(
                &format!("tl{i}"),
                vec![track(&format!("t{i}"), TrackKind::Video, items)],
            )
        })
        .collect();
    let e = eval_frame(&project_with(tls), "tl1", secs(1), None).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["maxDepth"].as_u64()),
        ("NESTED_CYCLE", Some(16))
    );

    let e = eval_frame(&p, "tl9", secs(1), None).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["kind"].as_str()),
        ("NOT_FOUND", Some("timeline"))
    );
    let missing = timeline(
        "tl1",
        vec![track(
            "t1",
            TrackKind::Video,
            vec![nested("n1", secs(0), secs(9), "tl7", secs(0))],
        )],
    );
    // A nested item whose timeline is missing draws nothing; the rest of the frame draws.
    let mut missing = missing;
    missing.tracks[0]
        .items
        .push(clip("c1", secs(9), secs(0), secs(1)));
    let p = project_with(vec![missing]);
    assert!(frame(&p, "1").is_empty());
    assert_eq!(frame(&p, "9")[0].item, "c1");
}

#[test]
fn adjustment_layer_has_effects_only() {
    let mut adj = Item::new("adj1", secs(0), secs(10), ItemBody::Adjustment {});
    adj.common.effects = vec![
        effect("fx1", "colorAdjust", &[("exposure", num(1.0))], true),
        effect("fx2", "blur", &[("radius", num(4.0))], false),
        effect("fx3", "blur", &[], true),
    ];
    adj.common.transform.position = [100.0, 0.0];
    adj.common.opacity = 0.5;
    let adj = keyed(
        adj,
        "effects.fx3.radius",
        vec![
            kf(secs(0), num(0.0), Interp::Linear),
            kf(secs(4), num(8.0), Interp::Linear),
        ],
    );
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![adj])],
    )]);
    let l = one(&p, "1");
    assert_eq!(l.kind, ItemKind::Adjustment);
    assert!(
        l.source.is_none() && l.caption.is_none() && l.graphic.is_none() && l.children.is_none()
    );
    assert_eq!(
        (l.transform, l.crop),
        (PlanTransform::default(), PlanCrop::default())
    );
    assert_eq!(l.opacity, 0.5);
    let fx: Vec<(&str, Vec<(&str, PlanValue)>)> = l
        .effects
        .iter()
        .map(|e| {
            (
                e.id.as_str(),
                e.params
                    .iter()
                    .map(|(k, v)| (k.as_str(), v.clone()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        fx,
        [
            // A registry effect's unset params come at their defaults.
            (
                "fx1",
                vec![
                    ("contrast", PlanValue::Number(0.0)),
                    ("exposure", PlanValue::Number(1.0)),
                    ("saturation", PlanValue::Number(1.0)),
                    ("temperature", PlanValue::Number(0.0)),
                    ("tint", PlanValue::Number(0.0)),
                ]
            ),
            ("fx3", vec![("radius", PlanValue::Number(2.0))])
        ]
    );
}

#[test]
fn mask_values_resolved() {
    let mut c = clip("c1", secs(0), secs(0), secs(10));
    c.common.mask = Some(Mask {
        matte: "art1".into(),
        feather: 2.0,
        choke: 0.5,
        invert: false,
    });
    let c = keyed(
        c,
        "mask.feather",
        vec![
            kf(secs(0), num(0.0), Interp::Linear),
            kf(secs(2), num(10.0), Interp::Linear),
        ],
    );
    let c = keyed(
        c,
        "mask.invert",
        vec![kf(secs(3), ParamValue::Bool(true), Interp::Hold)],
    );
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let mask = |t| one(&p, t).mask.unwrap();
    assert_eq!(
        mask("1"),
        MaskPlan {
            matte: "art1".into(),
            feather: 5.0,
            choke: 0.5,
            invert: true
        }
    );
    assert_eq!(mask("9").feather, 10.0);
    assert!(one(&p, "1").transition.is_none());
    let bare = project_with(vec![timeline(
        "tl1",
        vec![track(
            "t1",
            TrackKind::Video,
            vec![clip("c1", secs(0), secs(0), secs(1))],
        )],
    )]);
    assert!(one(&bare, "0").mask.is_none());
}

#[test]
fn keyframed_common_params_resolve_and_clamp() {
    let mut c = clip("c1", secs(1), secs(0), secs(10));
    c.common.blend = BlendMode::Screen;
    c.common.crop = Crop {
        left: 0.1,
        right: 0.0,
        top: 0.0,
        bottom: 1.5,
    };
    let c = keyed(
        c,
        "opacity",
        vec![
            kf(secs(0), num(0.0), Interp::Linear),
            kf(secs(2), num(2.0), Interp::Linear),
        ],
    );
    let c = keyed(
        c,
        "transform.position",
        vec![
            kf(secs(0), ParamValue::Vec2([0.0, 0.0]), Interp::Linear),
            kf(secs(4), ParamValue::Vec2([40.0, -80.0]), Interp::Linear),
        ],
    );
    let c = keyed(
        c,
        "transform.rotation",
        vec![
            kf(secs(0), num(0.0), Interp::Linear),
            kf(secs(1), num(90.0), Interp::Linear),
        ],
    );
    // A keyframe of the wrong shape falls back to the stored value.
    let c = keyed(
        c,
        "transform.scale",
        vec![kf(secs(0), num(3.0), Interp::Linear)],
    );
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let l = one(&p, "2");
    assert_eq!(l.local, secs(1));
    assert_eq!(l.opacity, 1.0);
    assert_eq!(
        l.transform,
        PlanTransform {
            position: [10.0, -20.0],
            scale: [1.0, 1.0],
            rotation: 90.0,
            anchor: [0.5, 0.5]
        }
    );
    assert_eq!(
        l.crop,
        PlanCrop {
            left: 0.1,
            right: 0.0,
            top: 0.0,
            bottom: 1.0
        }
    );
    assert_eq!(l.blend, BlendMode::Screen);
    assert_eq!(one(&p, "3/2").opacity, 0.5);
    assert_eq!(one(&p, "5").opacity, 1.0);
}

#[test]
fn image_and_graphic_layers() {
    let img = Item::new(
        "img1",
        secs(0),
        secs(5),
        ItemBody::Image { asset: "a2".into() },
    );
    let slots = [
        ("title".to_string(), ParamValue::Text("Hello".into())),
        ("tint".to_string(), ParamValue::Text("#ff0000".into())),
    ];
    let g = Item::new(
        "g1",
        secs(1),
        secs(5),
        ItemBody::Graphic {
            asset: "a3".into(),
            slots: slots.into_iter().collect(),
            source_in: fin_core::Rational::zero(),
        },
    );
    let g = keyed(
        g,
        "slots.tint",
        vec![
            kf(secs(0), ParamValue::Text("#ff0000".into()), Interp::Linear),
            kf(secs(2), ParamValue::Text("#0000ff".into()), Interp::Linear),
        ],
    );
    let g = keyed(
        g,
        "slots.title",
        vec![
            kf(secs(0), ParamValue::Text("Hello".into()), Interp::Linear),
            kf(secs(1), ParamValue::Text("Bye".into()), Interp::Linear),
        ],
    );
    let tracks = vec![
        track("t1", TrackKind::Video, vec![img]),
        track("t3", TrackKind::Video, vec![g]),
    ];
    let p = project_with(vec![timeline("tl1", tracks)]);
    let layers = frame(&p, "2");
    assert_eq!(layers[0].source, source("a2", "0"));
    let gl = &layers[1];
    assert_eq!(
        (gl.kind, gl.source.clone()),
        (ItemKind::Graphic, source("a3", "1"))
    );
    let plan = gl.graphic.as_ref().unwrap();
    assert_eq!((plan.asset.as_str(), plan.time), ("a3", secs(1)));
    assert_eq!(plan.slots["tint"], PlanValue::Color([0.5, 0.0, 0.5, 1.0]));
    assert_eq!(plan.slots["title"], PlanValue::Text("Bye".into()));
}

#[test]
fn caption_words_and_unmeasured() {
    let cap = caption(
        "cap1",
        secs(2),
        secs(3),
        &[("Hello", "-1", "1"), ("big", "1", "2"), ("world", "2", "4")],
    );
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("c1", TrackKind::Caption, vec![cap])],
    )]);
    let l = one(&p, "4");
    let c = l.caption.as_ref().unwrap();
    assert_eq!(c.style, "preset:bold");
    let words: Vec<_> = c
        .words
        .iter()
        .map(|w| (w.text.as_str(), w.start, w.end, w.active))
        .collect();
    // Times clamp to [0, duration]; the word spoken at local 2 is active.
    assert_eq!(
        words,
        [
            ("Hello", secs(0), secs(1), false),
            ("big", secs(1), secs(2), false),
            ("world", secs(2), secs(3), true)
        ]
    );
    assert_eq!(
        c.unmeasured.as_ref().unwrap().reason,
        UnmeasuredReason::NoMeasurer
    );
    let json = serde_json::to_value(&l).unwrap();
    assert!(json["caption"]["layout"].is_null() && json["caption"].get("layout").is_some());
    assert_eq!(
        json["caption"]["unmeasured"],
        serde_json::json!({"reason": "noMeasurer", "keys": []})
    );

    // With a measurer but no pin for the style's face: unmeasured, the measurer never asked.
    let plan = eval_frame(&p, "tl1", secs(4), Some(&Refuse)).unwrap();
    let c = plan.layers[0].caption.as_ref().unwrap();
    assert_eq!(
        (c.layout.is_none(), c.unmeasured.as_ref().unwrap().reason),
        (true, UnmeasuredReason::FontUnpinned)
    );
    assert!(plan.text_engine.is_none());
}

/// A measurer the eval must not call while no face resolves.
struct Refuse;

impl TextMeasurer for Refuse {
    fn measure(&self, _: &TextRequest) -> Result<Measured, CoreError> {
        panic!("no face resolves, so nothing is measured")
    }
    fn fingerprint(&self) -> String {
        "refuse/0".into()
    }
}

#[test]
fn muted_video_track_is_hidden() {
    let mut v1 = track(
        "t1",
        TrackKind::Video,
        vec![clip("c1", secs(0), secs(0), secs(5))],
    );
    v1.mute = true;
    let mut v2 = track(
        "t3",
        TrackKind::Video,
        vec![clip("c2", secs(0), secs(0), secs(5))],
    );
    v2.solo = true;
    let a1 = track(
        "t2",
        TrackKind::Audio,
        vec![clip("c3", secs(0), secs(0), secs(5))],
    );
    let mut c1 = track(
        "t4",
        TrackKind::Caption,
        vec![caption("cap1", secs(0), secs(5), &[("Hi", "0", "1")])],
    );
    c1.mute = true;
    let p = project_with(vec![timeline("tl1", vec![v1, a1, v2, c1])]);
    let ids: Vec<String> = frame(&p, "1").into_iter().map(|l| l.item).collect();
    assert_eq!(ids, ["c2"]);
}

#[test]
fn layer_order_is_track_order() {
    let tracks = vec![
        track(
            "t1",
            TrackKind::Video,
            vec![clip("c1", secs(0), secs(0), secs(5))],
        ),
        track(
            "t2",
            TrackKind::Audio,
            vec![clip("c2", secs(0), secs(0), secs(5))],
        ),
        track(
            "t4",
            TrackKind::Caption,
            vec![caption("cap1", secs(0), secs(5), &[("Hi", "0", "1")])],
        ),
        track(
            "t3",
            TrackKind::Video,
            vec![Item::new(
                "img1",
                secs(0),
                secs(5),
                ItemBody::Image { asset: "a2".into() },
            )],
        ),
    ];
    let p = project_with(vec![timeline("tl1", tracks)]);
    let ids: Vec<(String, String)> = frame(&p, "1")
        .into_iter()
        .map(|l| (l.track, l.item))
        .collect();
    let want =
        [("t1", "c1"), ("t4", "cap1"), ("t3", "img1")].map(|(a, b)| (a.to_string(), b.to_string()));
    assert_eq!(ids, want);
}

#[test]
fn non_finite_values_are_internal() {
    let mut c = clip("c1", secs(0), secs(0), secs(5));
    c.common.transform.rotation = f64::INFINITY;
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let e = eval_frame(&p, "tl1", secs(1), None).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["path"].as_str()),
        ("INTERNAL", Some("/layers/0/transform"))
    );
    let mut c = clip("c1", secs(0), secs(0), secs(5));
    c.common.effects = vec![effect("fx1", "blur", &[("radius", num(f64::NAN))], true)];
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let e = eval_frame(&p, "tl1", secs(1), None).unwrap_err();
    assert_eq!(e.details["path"], "/layers/0/effects/0/params/radius");
}

/// A busy project: two video tracks with a transition, keyframes of every interp, a nested
/// item, a graphic, a caption and an adjustment layer.
fn busy() -> Project {
    let [x1, y1, x2, y2] = fin_core::eval::PRESETS[0].1;
    let ease = Interp::Bezier { x1, y1, x2, y2 };
    let c1 = keyed(
        clip("c1", secs(0), secs(5), secs(4)),
        "opacity",
        vec![
            kf(secs(0), num(0.0), ease),
            kf(secs(2), num(1.0), Interp::Hold),
        ],
    );
    let c2 = keyed(
        with_speed(clip("c2", secs(4), secs(20), secs(4)), r("1/2")),
        "transform.position",
        vec![
            kf(secs(0), ParamValue::Vec2([0.0, 0.0]), Interp::Linear),
            kf(secs(3), ParamValue::Vec2([30.0, 60.0]), Interp::Hold),
        ],
    );
    let mut v1 = track("t1", TrackKind::Video, vec![c1, c2]);
    v1.transitions
        .push(crossfade("tr1", "c1", "c2", r("2/3"), Curve::Linear));
    let g = Item::new(
        "g1",
        secs(3),
        secs(3),
        ItemBody::Graphic {
            asset: "a3".into(),
            slots: [("tint".to_string(), ParamValue::Text("#336699".into()))]
                .into_iter()
                .collect(),
            source_in: fin_core::Rational::zero(),
        },
    );
    let n = Item::new(
        "n1",
        secs(0),
        secs(9),
        ItemBody::Nested {
            timeline: "tl2".into(),
            source_in: r("1/3"),
        },
    );
    let mut adj = Item::new("adj1", secs(0), secs(9), ItemBody::Adjustment {});
    adj.common.effects = vec![effect(
        "fx1",
        "colorAdjust",
        &[("saturation", num(0.5))],
        true,
    )];
    let tracks = vec![
        v1,
        track("t3", TrackKind::Video, vec![g]),
        track("t5", TrackKind::Video, vec![n]),
        track(
            "t4",
            TrackKind::Caption,
            vec![caption(
                "cap1",
                secs(3),
                secs(3),
                &[("One", "0", "1"), ("two", "1", "2")],
            )],
        ),
        track("t6", TrackKind::Video, vec![adj]),
    ];
    let inner = timeline(
        "tl2",
        vec![track(
            "t9",
            TrackKind::Video,
            vec![Item::new(
                "img1",
                secs(0),
                secs(20),
                ItemBody::Image { asset: "a2".into() },
            )],
        )],
    );
    project_with(vec![timeline("tl1", tracks), inner])
}

#[test]
fn plan_json_is_stable() {
    let p = busy();
    let plan = eval_frame(&p, "tl1", r("4"), None).unwrap();
    assert_eq!(plan, eval_frame(&p.clone(), "tl1", r("4"), None).unwrap());
    let json = serde_json::to_string(&plan).unwrap();
    let back: fin_core::eval::FramePlan = serde_json::from_str(&json).unwrap();
    assert_eq!(back, plan);
    // The golden is reviewed by hand; `FIN_EVAL_BLESS=1` rewrites it.
    let pretty = serde_json::to_string_pretty(&plan).unwrap() + "\n";
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/eval_frame_golden.json");
    if std::env::var("FIN_EVAL_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(path, &pretty).unwrap();
    }
    assert_eq!(pretty, std::fs::read_to_string(path).unwrap());
}

prop_compose! {
    fn interp()(k in 0..3u8, x1 in 0.0..=1.0f64, y1 in -2.0..3.0f64, x2 in 0.0..=1.0f64, y2 in -2.0..3.0f64) -> Interp {
        match k { 0 => Interp::Hold, 1 => Interp::Linear, _ => Interp::Bezier { x1, y1, x2, y2 } }
    }
}

prop_compose! {
    fn keys(len: usize)(ats in proptest::collection::vec(-20..200i64, 1..5), vals in proptest::collection::vec(-1e4..1e4f64, 4), ints in proptest::collection::vec(interp(), 4)) -> Vec<fin_core::Keyframe> {
        let _ = len;
        let mut ats = ats;
        ats.sort();
        ats.iter().enumerate().map(|(i, a)| kf(Rational::new(*a, 30).unwrap(), num(vals[i]), ints[i])).collect()
    }
}

prop_compose! {
    fn arb_project()(lens in proptest::collection::vec(1..90i64, 1..5), gaps in proptest::collection::vec(0..2i64, 4), speeds in proptest::collection::vec(1..90i64, 4),
                     op in keys(0), rot in keys(0), fx in keys(0), tr in proptest::collection::vec(0..60i64, 4)) -> Project {
        let mut start = 0i64;
        let mut items = Vec::new();
        for (i, len) in lens.iter().enumerate() {
            start += gaps[i] * 15;
            let id = format!("c{}", i + 1);
            let mut c = with_speed(clip(&id, Rational::new(start, 30).unwrap(), secs(1), Rational::new(*len, 30).unwrap()), Rational::new(speeds[i], 30).unwrap());
            c.common.effects = vec![effect("fx1", "blur", &[("radius", num(1.0))], true)];
            let c = keyed(c, "opacity", op.clone());
            let c = keyed(c, "transform.rotation", rot.clone());
            items.push(keyed(c, "effects.fx1.radius", fx.clone()));
            start += len;
        }
        let mut v1 = track("t1", TrackKind::Video, items.clone());
        for (i, pair) in items.windows(2).enumerate() {
            if pair[0].end().unwrap() == pair[1].start && tr[i] > 0 {
                v1.transitions.push(crossfade(&format!("tr{i}"), &pair[0].id, &pair[1].id, Rational::new(tr[i], 30).unwrap(), Curve::EqualPower));
            }
        }
        project_with(vec![timeline("tl1", vec![v1])])
    }
}

fn all_finite(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Number(n) => n.as_f64().is_some_and(f64::is_finite),
        serde_json::Value::Array(xs) => xs.iter().all(all_finite),
        serde_json::Value::Object(m) => m.values().all(all_finite),
        serde_json::Value::Null => false,
        _ => true,
    }
}

proptest! {
    #![proptest_config(prop_config(128))]
    #[test]
    fn no_nan_in_plans(p in arb_project(), t in -30..400i64) {
        let plan = eval_frame(&p, "tl1", Rational::new(t, 30).unwrap(), None).unwrap();
        prop_assert!(all_finite(&serde_json::to_value(&plan).unwrap()));
        for l in &plan.layers {
            prop_assert!((0.0..=1.0).contains(&l.opacity));
            if let Some(tr) = &l.transition {
                prop_assert!((0.0..1.0).contains(&tr.progress));
            }
        }
    }
}

/// A trim after `transition.set` can leave an item shorter than its transition (the op checks
/// the bound only when it is set): the window is clamped to the shorter item, so each half
/// stays within its item and an item's two windows never overlap.
#[test]
fn transition_longer_than_a_trimmed_item_is_clamped() {
    let mut t1 = track(
        "t1",
        TrackKind::Video,
        vec![
            clip("c1", secs(0), secs(5), secs(4)),
            clip("c2", secs(4), secs(20), secs(1)),
            clip("c3", secs(5), secs(40), secs(4)),
        ],
    );
    t1.transitions
        .push(crossfade("tr1", "c1", "c2", secs(4), Curve::Linear));
    t1.transitions
        .push(crossfade("tr2", "c2", "c3", secs(4), Curve::Linear));
    let p = project_with(vec![timeline("tl1", vec![t1])]);
    let at = |t: &str| {
        frame(&p, t)
            .into_iter()
            .map(|l| {
                let tr = l.transition.map(|s| (s.id, s.progress, s.role));
                (l.item, tr)
            })
            .collect::<Vec<_>>()
    };
    let tr = |id: &str, progress, role| Some((id.to_string(), progress, role));
    // Unclamped, tr1's window would be [2, 6) and c2 would show at 3.
    assert_eq!(at("3"), [("c1".into(), None)]);
    // Clamped to c2's one second: [7/2, 9/2) and [9/2, 11/2).
    assert_eq!(
        at("4"),
        [
            ("c1".into(), tr("tr1", 0.5, Role::Out)),
            ("c2".into(), tr("tr1", 0.5, Role::In)),
        ]
    );
    assert_eq!(
        at("9/2"),
        [
            ("c2".into(), tr("tr2", 0.0, Role::Out)),
            ("c3".into(), tr("tr2", 0.0, Role::In)),
        ]
    );
    assert_eq!(
        at("5"),
        [
            ("c2".into(), tr("tr2", 0.5, Role::Out)),
            ("c3".into(), tr("tr2", 0.5, Role::In)),
        ]
    );
    assert_eq!(at("11/2"), [("c3".into(), None)]);
}

/// A registry effect's params are typed by the registry and the unset ones come at their
/// defaults; an asset reference is carried as `{"asset": id}`; an unknown kind keeps only what
/// is stored.
#[test]
fn registry_effects_fill_defaults_and_carry_assets() {
    let mut adj = Item::new("adj1", secs(0), secs(10), ItemBody::Adjustment {});
    let lut = ParamValue::Asset { asset: "a9".into() };
    adj.common.effects = vec![
        effect("fx1", "lut", &[("asset", lut)], true),
        effect("fx2", "blur", &[], true),
        effect("fx3", "glow", &[("x", num(2.0))], true),
    ];
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![adj])],
    )]);
    let l = one(&p, "1");
    let params: Vec<serde_json::Value> = l
        .effects
        .iter()
        .map(|e| serde_json::to_value(&e.params).unwrap())
        .collect();
    assert_eq!(
        params,
        [
            serde_json::json!({ "asset": { "asset": "a9" }, "intensity": 1.0 }),
            serde_json::json!({ "radius": 0.0 }),
            serde_json::json!({ "x": 2.0 }),
        ]
    );
    assert_eq!(
        l.effects[0].params["asset"],
        PlanValue::Asset { asset: "a9".into() }
    );
}

/// S3: more than `MAX_NESTED_LAYERS` layers inside nested items in all is `NESTED_TOO_LARGE`;
/// at the cap a frame evaluates.
#[test]
fn nested_layer_total_is_capped() {
    let fan = |id: &str, n: usize, to: &str| {
        let tracks = (0..n)
            .map(|k| {
                let item = nested(&format!("n{k}"), secs(0), secs(9), to, secs(0));
                track(&format!("t{k}"), TrackKind::Video, vec![item])
            })
            .collect();
        timeline(id, tracks)
    };
    let leaf = |n: usize| {
        let tracks = (0..n)
            .map(|k| {
                track(
                    &format!("t{k}"),
                    TrackKind::Video,
                    vec![clip(&format!("c{k}"), secs(0), secs(0), secs(9))],
                )
            })
            .collect();
        timeline("tl2", tracks)
    };
    // 100 nested items of 100 layers each: 10,000, the cap (top-level layers do not count);
    // one more nested item passes it.
    let p = project_with(vec![fan("tl1", 100, "tl2"), leaf(100)]);
    assert_eq!(frame(&p, "1").len(), 100);
    let p = project_with(vec![fan("tl1", 101, "tl2"), leaf(100)]);
    let e = eval_frame(&p, "tl1", secs(1), None).unwrap_err();
    assert_eq!(
        (e.code.as_str(), e.details["max"].as_u64()),
        ("NESTED_TOO_LARGE", Some(10_000))
    );
}

/// S7: keyframed mask feather and choke never go below 0.
#[test]
fn mask_feather_and_choke_clamp_at_zero() {
    let mut c = clip("c1", secs(0), secs(0), secs(4));
    c.common.mask = Some(Mask {
        matte: "c2".into(),
        feather: 2.0,
        choke: 1.0,
        invert: false,
    });
    let c = keyed(
        c,
        "mask.feather",
        vec![kf(secs(0), num(-5.0), Interp::Hold)],
    );
    let c = keyed(c, "mask.choke", vec![kf(secs(0), num(-1.0), Interp::Hold)]);
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let m = frame(&p, "1")[0].mask.clone().unwrap();
    assert_eq!((m.feather, m.choke), (0.0, 0.0));
}

/// External review of 2026-11-09, #17: an opacity keyed at `(M−1)/M` and `M/(M−1)` evaluates
/// (it was `INTERNAL` at `/layers/0/opacity`, a NaN).
#[test]
fn keyframes_with_near_max_denominators_evaluate() {
    let m = i64::MAX;
    let (a, b) = (
        Rational::new(m - 1, m).unwrap(),
        Rational::new(m, m - 1).unwrap(),
    );
    let c = keyed(
        clip("c1", secs(0), secs(0), secs(10)),
        "opacity",
        vec![
            kf(a, num(0.0), Interp::Linear),
            kf(b, num(1.0), Interp::Linear),
        ],
    );
    let p = project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Video, vec![c])],
    )]);
    let o = one(&p, "1").opacity;
    assert!((o - 0.5).abs() < 1e-12, "{o}");
}
