//! Review #51: a keyframed-only graphic slot whose id contains a dot is evaluated like any
//! other slot. The slot id is everything after `slots.` at import, mutation and evaluation.

mod common;

use common::*;
use fin_core::eval::{PlanValue, eval_frame};
use fin_core::{
    AssetAdd, AssetKind, InsertMode, Interp, ItemBody, ItemInsert, KeyframeSet, LottieInfo, Op,
    ParamValue, Project, SlotSpec,
};

fn template(slot: &str) -> Op {
    let mut a = AssetAdd::media("$lot", "/m/brand.json", secs(10), None, None, "fp");
    a.kind = AssetKind::LottieTemplate;
    a.duration = Some(secs(4));
    a.lottie = Some(LottieInfo {
        width: 1080,
        height: 1920,
        rate: secs(30),
        duration: secs(4),
        slots: vec![SlotSpec {
            id: slot.into(),
            kind: "color".into(),
        }],
        fonts: Vec::new(),
    });
    a.into()
}

/// A graphic with no stored slot value, then a key on `slots.<slot>` at 0: the plan's value.
fn keyed_slot(slot: &str) -> Option<PlanValue> {
    let a = apply(&with_clips(&[]), vec![template(slot)]);
    let (p, asset) = (a.project, a.bindings["$lot"].clone());
    let ops: Vec<Op> = vec![
        ItemInsert {
            id: "$g".into(),
            track: "t1".into(),
            at: secs(0),
            duration: secs(4),
            mode: InsertMode::Ripple,
            body: ItemBody::Graphic {
                asset,
                slots: Default::default(),
                source_in: fin_core::Rational::zero(),
            },
        }
        .into(),
        KeyframeSet::timed(
            "$g",
            format!("slots.{slot}"),
            secs(0),
            ParamValue::Text("#ff0000".into()),
            Interp::Linear,
        )
        .into(),
    ];
    let p: Project = apply(&p, ops).project;
    let tl = p.timelines[0].id.clone();
    let frame = eval_frame(&p, &tl, secs(1), None).unwrap();
    let g = frame.layers[0].graphic.as_ref().unwrap();
    g.slots.get(slot).cloned()
}

#[test]
fn dotted_keyframed_slot_is_evaluated() {
    let red = Some(PlanValue::Color([1.0, 0.0, 0.0, 1.0]));
    assert_eq!(keyed_slot("tint"), red, "control: a simple id");
    assert_eq!(keyed_slot("brand.tint"), red);
    assert_eq!(keyed_slot("a.b.c"), red);
}
