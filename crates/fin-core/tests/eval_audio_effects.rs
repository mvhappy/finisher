//! `AudioItem.effects` (plan 6C Task 6, Decision 15): each enabled effect of an audio clip with
//! its numbers as piecewise segments over the item's span and its other values resolved.

mod eval_common;

use std::collections::BTreeSet;

use eval_common::*;
use fin_core::eval::{AudioEffectPlan, AudioItem, ParamSegment, PlanValue, eval_audio};
use fin_core::{Interp, Item, ItemBody, ParamValue, Project, Rational, TrackKind};

fn items(p: &Project, from: &str, to: &str) -> Vec<AudioItem> {
    eval_audio(p, "tl1", r(from), r(to)).unwrap().items
}

fn only(p: &Project, from: &str, to: &str) -> AudioItem {
    let mut v = items(p, from, to);
    assert_eq!(v.len(), 1, "{v:?}");
    v.remove(0)
}

/// One audio clip `c1` on `[0, len)` of `tl1` with `fx` and keyframes `keys`.
fn with_effects(
    len: i64,
    fx: Vec<fin_core::Effect>,
    keys: Vec<(&str, Vec<fin_core::Keyframe>)>,
) -> Project {
    let mut c = clip("c1", secs(0), secs(0), secs(len));
    c.common.effects = fx;
    for (path, k) in keys {
        c = keyed(c, path, k);
    }
    project_with(vec![timeline(
        "tl1",
        vec![track("t1", TrackKind::Audio, vec![c])],
    )])
}

fn seg(from: Rational, to: Rational, v: [f64; 2], interp: Interp, u: [f64; 2]) -> ParamSegment {
    ParamSegment {
        from,
        to,
        v,
        interp,
        u,
    }
}

fn hold(from: Rational, to: Rational, v: f64) -> ParamSegment {
    seg(from, to, [v, v], Interp::Hold, [0.0, 1.0])
}

fn fx<'a>(item: &'a AudioItem, id: &str) -> &'a AudioEffectPlan {
    item.effects
        .iter()
        .find(|e| e.id == id)
        .unwrap_or_else(|| panic!("no effect {id} in {:?}", item.effects))
}

fn text(s: &str) -> PlanValue {
    PlanValue::Text(s.into())
}

#[test]
fn disabled_effect_absent() {
    let p = with_effects(
        4,
        vec![
            effect("fx1", "eq", &[], false),
            effect("fx2", "compressor", &[], true),
        ],
        vec![],
    );
    let item = only(&p, "0", "4");
    let ids: Vec<&str> = item.effects.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["fx2"]);
}

#[test]
fn effects_in_item_order() {
    let p = with_effects(
        4,
        vec![
            effect("fx3", "gate", &[], true),
            effect("fx1", "eq", &[], true),
            effect("fx2", "deEsser", &[], true),
        ],
        vec![],
    );
    let item = only(&p, "0", "4");
    let got: Vec<(&str, &str)> = item
        .effects
        .iter()
        .map(|e| (e.id.as_str(), e.kind.as_str()))
        .collect();
    assert_eq!(got, [("fx3", "gate"), ("fx1", "eq"), ("fx2", "deEsser")]);
}

#[test]
fn all_params_present_with_defaults() {
    let p = with_effects(
        4,
        vec![
            effect("fx1", "compressor", &[("ratio", num(2.0))], true),
            effect(
                "fx2",
                "eq",
                &[("band3Type", ParamValue::Text("peak".into()))],
                true,
            ),
        ],
        vec![],
    );
    let item = only(&p, "1", "3");
    let (a, b) = (secs(1), secs(3));
    let comp = fx(&item, "fx1");
    let names: BTreeSet<&str> = comp.numbers.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        BTreeSet::from(["attack", "knee", "makeup", "ratio", "release", "threshold"])
    );
    assert_eq!(comp.numbers["ratio"], [hold(a, b, 2.0)]);
    assert_eq!(comp.numbers["threshold"], [hold(a, b, -18.0)]);
    assert_eq!(comp.numbers["release"], [hold(a, b, 100.0)]);
    assert_eq!(comp.values.len(), 1);
    assert_eq!(comp.values["detector"], text("rms"));

    let eq = fx(&item, "fx2");
    assert_eq!(eq.numbers.len(), 24, "{:?}", eq.numbers.keys());
    assert_eq!(eq.values.len(), 8, "{:?}", eq.values.keys());
    assert_eq!(eq.values["band3Type"], text("peak"));
    assert_eq!(eq.values["band1Type"], text("off"));
    assert_eq!(eq.numbers["band1Freq"], [hold(a, b, 80.0)]);
    assert_eq!(eq.numbers["band8Freq"], [hold(a, b, 10_000.0)]);
    assert_eq!(eq.numbers["band5Q"], [hold(a, b, 0.707)]);
    // `enabled` is the effect's own flag, not a param.
    assert!(!eq.numbers.contains_key("enabled") && !eq.values.contains_key("enabled"));
}

/// Keys at 1 s and 3 s over `[0, 4)`: constant before, the ramp, constant after.
#[test]
fn keyframed_gain_segments_cut_at_keys() {
    let p = with_effects(
        4,
        vec![effect("fx1", "eq", &[("band2Gain", num(3.0))], true)],
        vec![(
            "effects.fx1.band2Gain",
            vec![
                kf(secs(1), num(-6.0), Interp::Linear),
                kf(secs(3), num(6.0), Interp::Hold),
            ],
        )],
    );
    let item = only(&p, "0", "4");
    let gain = &fx(&item, "fx1").numbers["band2Gain"];
    assert_eq!(
        gain,
        &[
            hold(secs(0), secs(1), -6.0),
            seg(secs(1), secs(3), [-6.0, 6.0], Interp::Linear, [0.0, 1.0]),
            hold(secs(3), secs(4), 6.0),
        ]
    );
    // Untouched params stay one segment.
    assert_eq!(
        fx(&item, "fx1").numbers["band2Q"],
        [hold(secs(0), secs(4), 0.707)]
    );
}

/// A range cutting into a keyframe span keeps the span's ends and carries the progress.
#[test]
fn segments_clipped_to_range() {
    let ease = Interp::Bezier {
        x1: 0.42,
        y1: 0.0,
        x2: 0.58,
        y2: 1.0,
    };
    let p = with_effects(
        8,
        vec![effect("fx1", "compressor", &[], true)],
        vec![(
            "effects.fx1.threshold",
            vec![
                kf(secs(1), num(-40.0), ease),
                kf(secs(5), num(-10.0), Interp::Linear),
            ],
        )],
    );
    let item = only(&p, "2", "6");
    assert_eq!((item.from, item.to), (secs(2), secs(6)));
    let th = &fx(&item, "fx1").numbers["threshold"];
    assert_eq!(
        th,
        &[
            seg(secs(2), secs(5), [-40.0, -10.0], ease, [0.25, 1.0]),
            hold(secs(5), secs(6), -10.0),
        ]
    );
    let item = only(&p, "3", "4");
    assert_eq!(
        fx(&item, "fx1").numbers["threshold"],
        [seg(secs(3), secs(4), [-40.0, -10.0], ease, [0.5, 0.75])]
    );
    // Every number covers the item's span without gaps.
    for segs in fx(&item, "fx1").numbers.values() {
        assert_eq!(segs.first().unwrap().from, item.from);
        assert_eq!(segs.last().unwrap().to, item.to);
        assert!(segs.windows(2).all(|w| w[0].to == w[1].from));
    }
}

fn nested(id: &str, start: Rational, duration: Rational, tl: &str, source_in: Rational) -> Item {
    let body = ItemBody::Nested {
        timeline: tl.into(),
        source_in,
    };
    Item::new(id, start, duration, body)
}

/// Inner clip at 2 s inside a nested item at 10 s with `sourceIn` 1: outer = inner + 9.
#[test]
fn nested_effect_segments_shifted() {
    let mut c = clip("c9", secs(2), secs(0), secs(4));
    c.common.effects = vec![effect("fx1", "gate", &[], true)];
    let c = keyed(
        c,
        "effects.fx1.threshold",
        vec![
            kf(secs(1), num(-60.0), Interp::Linear),
            kf(secs(3), num(-30.0), Interp::Hold),
        ],
    );
    let inner = timeline("tl2", vec![track("t9", TrackKind::Audio, vec![c])]);
    let outer = timeline(
        "tl1",
        vec![track(
            "t1",
            TrackKind::Audio,
            vec![nested("n1", secs(10), secs(10), "tl2", secs(1))],
        )],
    );
    let p = project_with(vec![outer, inner]);
    let item = only(&p, "0", "30");
    assert_eq!(item.via, ["n1"]);
    assert_eq!((item.from, item.to), (secs(11), secs(15)));
    let gate = fx(&item, "fx1");
    assert_eq!(
        gate.numbers["threshold"],
        [
            hold(secs(11), secs(12), -60.0),
            seg(
                secs(12),
                secs(14),
                [-60.0, -30.0],
                Interp::Linear,
                [0.0, 1.0]
            ),
            hold(secs(14), secs(15), -30.0),
        ]
    );
    assert_eq!(gate.numbers["hold"], [hold(secs(11), secs(15), 50.0)]);
    // A range inside the nested item clips the inner segments too.
    let item = only(&p, "13", "14");
    assert_eq!(
        fx(&item, "fx1").numbers["threshold"],
        [seg(
            secs(13),
            secs(14),
            [-60.0, -30.0],
            Interp::Linear,
            [0.5, 1.0]
        )]
    );
}

/// A clip without effects serialises as before Decision 15: no `effects` key, so every
/// existing plan hash (the 4C corpus, the e2e reel golden) is unchanged.
#[test]
fn no_effects_hash_unchanged() {
    let p = with_effects(4, vec![], vec![]);
    let item = only(&p, "0", "4");
    assert!(item.effects.is_empty());
    let json = serde_json::to_value(&item).unwrap();
    assert!(json.get("effects").is_none(), "{json}");
    // A disabled effect is absent too, so it does not change the hash either.
    let q = with_effects(4, vec![effect("fx1", "eq", &[], false)], vec![]);
    assert_eq!(
        serde_json::to_string(&eval_audio(&q, "tl1", secs(0), secs(4)).unwrap()).unwrap(),
        serde_json::to_string(&eval_audio(&p, "tl1", secs(0), secs(4)).unwrap()).unwrap()
    );
}

/// A visual kind on an audio clip (only an import can put one there) does not sound; a kind
/// the registry does not know (a newer model's) is carried, typed by its values, for the mixer
/// to skip and report.
#[test]
fn visual_kind_dropped_unknown_kind_kept() {
    let p = with_effects(
        4,
        vec![
            effect("fx1", "blur", &[], true),
            effect(
                "fx2",
                "reverb",
                &[("mix", num(0.3)), ("mode", ParamValue::Text("hall".into()))],
                true,
            ),
        ],
        vec![(
            "effects.fx2.decay",
            vec![
                kf(secs(1), num(1.0), Interp::Linear),
                kf(secs(2), num(2.0), Interp::Linear),
            ],
        )],
    );
    let item = only(&p, "0", "4");
    assert_eq!(item.effects.len(), 1, "{:?}", item.effects);
    let rev = fx(&item, "fx2");
    assert_eq!(rev.kind, "reverb");
    assert_eq!(rev.numbers["mix"], [hold(secs(0), secs(4), 0.3)]);
    assert_eq!(rev.numbers["decay"].len(), 3);
    assert_eq!(rev.values["mode"], text("hall"));
}

/// A non-finite effect value is `INTERNAL` with its path, as gains are.
#[test]
fn non_finite_effect_number_is_internal() {
    let p = with_effects(
        4,
        vec![effect(
            "fx1",
            "compressor",
            &[("ratio", num(f64::INFINITY))],
            true,
        )],
        vec![],
    );
    let e = eval_audio(&p, "tl1", secs(0), secs(4)).unwrap_err();
    assert_eq!(e.code, "INTERNAL");
    assert_eq!(e.details["path"], "/items/0/effects/0/numbers/ratio/0");
}
