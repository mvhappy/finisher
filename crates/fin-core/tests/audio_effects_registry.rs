//! The audio effect kinds (plan 6C Task 5, Decision 13): `eq` (eight flat bands),
//! `compressor`, `gate` and `deEsser` in the registry with their ranges and defaults, their
//! domain, and `effect.add` refusing an effect whose domain does not match the item's track
//! (`TRACK_KIND_MISMATCH`, policy) in both directions.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::params::{self, DefaultValue, EffectDomain, ParamType, Scope, ScopeOf};
use fin_core::{
    ApplyMode, EffectAdd, EffectRemove, InsertMode, Interp, Item, ItemMove, ItemSplit, KeyframeSet,
    MemIds, Op, ParamSet, ParamValue, Project, apply_internal, apply_tx, apply_tx_mode,
};
use serde_json::json;

fn add(item: &str, id: &str, kind: &str, params: &[(&str, ParamValue)]) -> Op {
    EffectAdd {
        item: item.into(),
        id: id.into(),
        kind: kind.into(),
        at: None,
        params: params
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<BTreeMap<_, _>>(),
    }
    .into()
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

/// `c1` (10 s) on video track `t1` and `c2` (10 s, audio only) on audio track `t2`.
fn av() -> Project {
    apply(
        &project(),
        vec![
            av_asset("$a", secs(100)),
            audio_asset("$s", secs(100)),
            insert("$c1", "t1", secs(0), "$a", secs(0), secs(10)),
            insert("$c2", "t2", secs(0), "$s", secs(0), secs(10)),
        ],
    )
    .project
}

/// `av()` with a compressor `fx1` on the audio clip `c2`.
fn compressed() -> Project {
    apply(&av(), vec![add("c2", "$fx", "compressor", &[])]).project
}

fn n(x: f64) -> ParamValue {
    ParamValue::Number(x)
}

fn text(s: &str) -> ParamValue {
    ParamValue::Text(s.into())
}

#[test]
fn audio_kinds_listed_with_domain() {
    assert_eq!(
        params::effect_kinds(),
        [
            "colorAdjust",
            "lut",
            "blur",
            "eq",
            "compressor",
            "gate",
            "deEsser"
        ]
    );
    for k in ["colorAdjust", "lut", "blur"] {
        assert_eq!(params::effect_domain(k), Some(EffectDomain::Visual), "{k}");
    }
    for k in ["eq", "compressor", "gate", "deEsser"] {
        assert_eq!(params::effect_domain(k), Some(EffectDomain::Audio), "{k}");
    }
    assert_eq!(params::effect_domain("reverb"), None);
    assert_eq!(serde_json::to_value(EffectDomain::Audio).unwrap(), "audio");
    assert_eq!(
        serde_json::to_value(EffectDomain::Visual).unwrap(),
        "visual"
    );
    // Every effect spec's kind is a listed kind, and every kind has `enabled`.
    for kind in params::effect_kinds() {
        let path = format!("{kind}.enabled");
        assert!(
            params::specs().iter().any(|s| s.path == path),
            "{kind} has enabled"
        );
    }
}

#[test]
fn eq_band_params_flat_and_typed() {
    let eq = ScopeOf::Effect { kind: "eq" };
    let freq = params::find(&eq, "band3Freq").unwrap();
    assert_eq!(freq.path, "eq.band3Freq");
    assert_eq!(freq.scope, Scope::Effect { kind: "eq" });
    assert_eq!(
        freq.ty,
        ParamType::Number {
            min: 20.0,
            max: 20_000.0
        }
    );
    assert_eq!(freq.unit, Some("Hz"));
    assert!(freq.animatable);
    let ty = params::find(&eq, "band3Type").unwrap();
    assert_eq!(
        ty.ty,
        ParamType::Enum {
            values: &[
                "off",
                "lowShelf",
                "highShelf",
                "peak",
                "lowPass",
                "highPass"
            ]
        }
    );
    assert!(!ty.animatable);
    assert_eq!(ty.default, DefaultValue::Text("off"));
    // No dotted band names: the registry's one-level rule.
    assert!(
        params::specs()
            .iter()
            .filter(|s| matches!(s.scope, Scope::Effect { .. }))
            .all(|s| s.path.matches('.').count() == 1)
    );
    // band9 does not exist: PARAM_UNKNOWN with the closest names, on an item path too.
    let e = params::find(&eq, "band9Freq").unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    let alts = e.details["alternatives"].as_array().unwrap();
    assert!(alts.contains(&json!("band1Freq")), "{alts:?}");
    let p = apply(&av(), vec![add("c2", "$fx", "eq", &[])]).project;
    let c2 = item(&p, "c2");
    let e = params::find(
        &ScopeOf::Item {
            item: c2,
            template: None,
        },
        "effects.fx1.band9Freq",
    )
    .unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert!(
        e.details["alternatives"]
            .as_array()
            .unwrap()
            .contains(&json!("effects.fx1.band1Freq"))
    );
}

/// Every audio spec, as Decision 13 lists it: `(path, min, max, default, unit)` for numbers
/// (animatable), `(path, values, default)` for enums (not animatable).
#[test]
fn defaults_match_decision_13() {
    type Num = (&'static str, f64, f64, f64, Option<&'static str>);
    let freqs = [80.0, 160.0, 315.0, 630.0, 1250.0, 2500.0, 5000.0, 10_000.0];
    let mut numbers: Vec<(String, f64, f64, f64, Option<&str>)> = Vec::new();
    let mut enums: Vec<(String, Vec<&str>, &str)> = Vec::new();
    let bands = [
        "off",
        "lowShelf",
        "highShelf",
        "peak",
        "lowPass",
        "highPass",
    ];
    for (i, f) in freqs.iter().enumerate() {
        let b = i + 1;
        enums.push((format!("eq.band{b}Type"), bands.to_vec(), "off"));
        numbers.push((format!("eq.band{b}Freq"), 20.0, 20_000.0, *f, Some("Hz")));
        numbers.push((format!("eq.band{b}Gain"), -24.0, 24.0, 0.0, Some("dB")));
        numbers.push((format!("eq.band{b}Q"), 0.1, 18.0, 0.707, None));
    }
    enums.push(("compressor.detector".into(), vec!["rms", "peak"], "rms"));
    let fixed: [Num; 16] = [
        ("compressor.threshold", -60.0, 0.0, -18.0, Some("dB")),
        ("compressor.ratio", 1.0, 20.0, 4.0, None),
        ("compressor.knee", 0.0, 24.0, 6.0, Some("dB")),
        ("compressor.attack", 0.1, 200.0, 10.0, Some("ms")),
        ("compressor.release", 5.0, 2000.0, 100.0, Some("ms")),
        ("compressor.makeup", 0.0, 24.0, 0.0, Some("dB")),
        ("gate.threshold", -90.0, 0.0, -50.0, Some("dB")),
        ("gate.range", -90.0, 0.0, -80.0, Some("dB")),
        ("gate.attack", 0.1, 50.0, 1.0, Some("ms")),
        ("gate.hold", 0.0, 500.0, 50.0, Some("ms")),
        ("gate.release", 5.0, 2000.0, 100.0, Some("ms")),
        ("deEsser.frequency", 2000.0, 12_000.0, 6500.0, Some("Hz")),
        ("deEsser.q", 0.5, 8.0, 2.0, None),
        ("deEsser.threshold", -60.0, 0.0, -30.0, Some("dB")),
        ("deEsser.ratio", 1.0, 20.0, 6.0, None),
        ("deEsser.range", 0.0, 24.0, 12.0, Some("dB")),
    ];
    for (p, lo, hi, d, u) in fixed {
        numbers.push((p.to_string(), lo, hi, d, u));
    }
    let audio: Vec<_> = params::specs()
        .iter()
        .filter(|s| {
            matches!(s.scope, Scope::Effect { kind }
                if params::effect_domain(kind) == Some(EffectDomain::Audio))
        })
        .collect();
    let enabled: Vec<&str> = audio
        .iter()
        .filter(|s| s.path.ends_with(".enabled"))
        .map(|s| s.path)
        .collect();
    assert_eq!(
        enabled,
        [
            "eq.enabled",
            "compressor.enabled",
            "gate.enabled",
            "deEsser.enabled"
        ]
    );
    for s in audio.iter().filter(|s| s.path.ends_with(".enabled")) {
        assert_eq!(
            (s.ty, s.default, s.animatable),
            (ParamType::Bool, DefaultValue::Bool(true), false)
        );
    }
    assert_eq!(audio.len(), 4 + numbers.len() + enums.len());
    for (path, min, max, default, unit) in &numbers {
        let s = audio
            .iter()
            .find(|s| s.path == path)
            .unwrap_or_else(|| panic!("no {path}"));
        assert_eq!(
            s.ty,
            ParamType::Number {
                min: *min,
                max: *max
            },
            "{path}"
        );
        assert_eq!(s.default, DefaultValue::Number(*default), "{path}");
        assert_eq!(s.unit, *unit, "{path}");
        assert!(s.animatable, "{path}");
        assert!(!s.fix.is_empty(), "{path}");
    }
    for (path, values, default) in &enums {
        let s = audio
            .iter()
            .find(|s| s.path == path)
            .unwrap_or_else(|| panic!("no {path}"));
        let ParamType::Enum { values: got } = s.ty else {
            panic!("{path} is not an enum: {:?}", s.ty)
        };
        assert_eq!(got, values.as_slice(), "{path}");
        assert_eq!(s.default, DefaultValue::Text(default), "{path}");
        assert!(!s.animatable, "{path}");
    }
}

#[test]
fn audio_effect_on_audio_clip_ok() {
    let p = av();
    for kind in ["eq", "compressor", "gate", "deEsser"] {
        let a = apply(&p, vec![add("c2", "$fx", kind, &[])]);
        let fx = &item(&a.project, "c2").common.effects[0];
        assert_eq!((fx.kind.as_str(), fx.enabled), (kind, true));
    }
    let a = apply(
        &p,
        vec![add(
            "c2",
            "$fx",
            "eq",
            &[
                ("band2Type", text("peak")),
                ("band2Gain", n(-3.5)),
                ("band2Q", n(2.0)),
            ],
        )],
    );
    let fx = &item(&a.project, "c2").common.effects[0];
    assert_eq!(fx.params["band2Type"], text("peak"));
    assert_eq!(fx.params["band2Gain"], n(-3.5));
    // An enum value outside its list is refused.
    let e = try_apply(
        &p,
        vec![add("c2", "$fx", "eq", &[("band1Type", text("notch"))])],
    )
    .unwrap_err();
    assert!(
        ["PARAM_TYPE", "INVALID_RANGE"].contains(&e.code.as_str()),
        "{e:?}"
    );
}

#[test]
fn audio_effect_on_video_track_refused() {
    let p = av();
    let op = add("c1", "$fx", "compressor", &[]);
    let e = apply_tx(&p, &tx(vec![op.clone()]), &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(e.code, "TRACK_KIND_MISMATCH");
    assert_eq!(
        (
            &e.details["item"],
            &e.details["kind"],
            &e.details["trackKind"]
        ),
        (&json!("c1"), &json!("compressor"), &json!("video"))
    );
    assert_eq!(
        e.fix.as_deref(),
        Some("add audio effects to clips on audio tracks")
    );
    // Policy: a stored op replays as it was.
    let mut literal = op;
    literal.for_each_ref(|r| {
        if r == "$fx" {
            *r = "fx9".into();
        }
    });
    apply_tx_mode(
        &p,
        &tx(vec![literal]),
        &mut MemIds::after(&p),
        ApplyMode::Replay,
    )
    .unwrap();
}

#[test]
fn visual_effect_on_audio_track_still_refused() {
    let p = av();
    let e = try_apply(&p, vec![add("c2", "$fx", "blur", &[])]).unwrap_err();
    assert_eq!(e.code, "TRACK_KIND_MISMATCH");
    assert_eq!(e.details["trackKind"], "audio");
    assert_eq!(
        e.fix.as_deref(),
        Some("add visual effects to items on video tracks")
    );
    // Visual effects on the video clip are fine.
    apply(&p, vec![add("c1", "$fx", "blur", &[])]);
}

#[test]
fn keyframe_on_compressor_threshold() {
    let p = compressed();
    let key = |at, v| -> Op {
        KeyframeSet {
            item: "c2".into(),
            path: "effects.fx1.threshold".into(),
            key: None,
            at: Some(secs(at)),
            value: n(v),
            interp: Interp::Linear,
        }
        .into()
    };
    let a = golden_apply(&p, vec![key(1, -30.0), key(3, -10.0)]);
    let keys = &item(&a.project, "c2").common.keyframes["effects.fx1.threshold"];
    assert_eq!(keys.len(), 2);
    let e = try_apply(&p, vec![key(1, -61.0)]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
}

#[test]
fn keyframe_on_band_type_refused() {
    let p = apply(&av(), vec![add("c2", "$fx", "eq", &[])]).project;
    let op: Op = KeyframeSet {
        item: "c2".into(),
        path: "effects.fx1.band1Type".into(),
        key: None,
        at: Some(secs(1)),
        value: text("peak"),
        interp: Interp::Hold,
    }
    .into();
    let e = try_apply(&p, vec![op]).unwrap_err();
    assert_eq!(e.code, "PARAM_NOT_ANIMATABLE");
    let p = compressed();
    let op: Op = KeyframeSet {
        item: "c2".into(),
        path: "effects.fx1.detector".into(),
        key: None,
        at: Some(secs(1)),
        value: text("peak"),
        interp: Interp::Hold,
    }
    .into();
    assert_eq!(
        try_apply(&p, vec![op]).unwrap_err().code,
        "PARAM_NOT_ANIMATABLE"
    );
}

#[test]
fn param_set_out_of_range() {
    let p = compressed();
    let set = |path: &str, v: ParamValue| -> Op {
        ParamSet {
            item: "c2".into(),
            path: path.into(),
            value: Some(v),
        }
        .into()
    };
    let e = try_apply(&p, vec![set("effects.fx1.ratio", n(25.0))]).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(
        (
            &e.details["path"],
            &e.details["min"],
            &e.details["max"],
            &e.details["value"]
        ),
        (
            &json!("effects.fx1.ratio"),
            &json!(1.0),
            &json!(20.0),
            &json!(25.0)
        )
    );
    assert!(e.fix.is_some());
    // In range, and an enum, are fine.
    let a = golden_apply(
        &p,
        vec![
            set("effects.fx1.ratio", n(20.0)),
            set("effects.fx1.detector", text("peak")),
        ],
    );
    let fx = &item(&a.project, "c2").common.effects[0];
    assert_eq!(
        (&fx.params["ratio"], &fx.params["detector"]),
        (&n(20.0), &text("peak"))
    );
}

#[test]
fn split_copies_audio_effects_with_ids() {
    let p = apply(
        &compressed(),
        vec![add("c2", "$fx", "eq", &[("band1Type", text("lowShelf"))])],
    )
    .project;
    let a = apply(
        &p,
        vec![
            ItemSplit {
                item: "c2".into(),
                offset: secs(4),
                id: "$r".into(),
            }
            .into(),
        ],
    );
    let right = &a.bindings["$r"];
    let (l, r) = (item(&a.project, "c2"), item(&a.project, right));
    assert_eq!(l.common.effects, r.common.effects);
    let ids: Vec<_> = r.common.effects.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["fx1", "fx2"]);
    assert_eq!(r.common.effects[1].params["band1Type"], text("lowShelf"));
}

#[test]
fn undo_effect_add_exact() {
    let p = av();
    for (kind, params) in [
        (
            "eq",
            vec![("band8Type", text("highPass")), ("band8Freq", n(12_000.0))],
        ),
        (
            "compressor",
            vec![("threshold", n(-24.0)), ("detector", text("peak"))],
        ),
        ("gate", vec![("hold", n(0.0)), ("range", n(-90.0))]),
        (
            "deEsser",
            vec![("frequency", n(7000.0)), ("range", n(24.0))],
        ),
    ] {
        let a = golden_apply(&p, vec![add("c2", "$fx", kind, &params)]);
        assert_eq!(item(&a.project, "c2").common.effects.len(), 1, "{kind}");
    }
}

// ---- the domain rule wherever an item with effects lands on a track (task 5 review) ----

/// `c1` (10 s, A/V) on video track `t1` and `c2` (10 s, from the same A/V asset) on audio
/// track `t2`, so either clip may move to the other track as far as its asset goes.
fn av_both() -> Project {
    apply(
        &project(),
        vec![
            av_asset("$a", secs(100)),
            insert("$c1", "t1", secs(0), "$a", secs(0), secs(10)),
            insert("$c2", "t2", secs(0), "$a", secs(0), secs(10)),
        ],
    )
    .project
}

fn mv(item: &str, track: &str, at: i64) -> Op {
    ItemMove {
        item: item.into(),
        track: track.into(),
        at: secs(at),
        mode: InsertMode::Ripple,
    }
    .into()
}

fn mismatch(e: &fin_core::CoreError, item: &str, kind: &str, track_kind: &str) {
    assert_eq!(e.code, "TRACK_KIND_MISMATCH", "{e:?}");
    assert_eq!(
        (
            &e.details["item"],
            &e.details["kind"],
            &e.details["trackKind"]
        ),
        (&json!(item), &json!(kind), &json!(track_kind)),
        "{e:?}"
    );
}

/// Policy, so a stored op replays as it was.
fn replays(p: &Project, op: Op) {
    apply_tx_mode(p, &tx(vec![op]), &mut MemIds::after(p), ApplyMode::Replay).unwrap();
}

#[test]
fn move_audio_effect_onto_video_track_refused() {
    let p = apply(&av_both(), vec![add("c2", "$fx", "compressor", &[])]).project;
    let e = try_apply(&p, vec![mv("c2", "t1", 20)]).unwrap_err();
    mismatch(&e, "c2", "compressor", "video");
    assert_eq!(
        e.fix.as_deref(),
        Some("add audio effects to clips on audio tracks")
    );
    replays(&p, mv("c2", "t1", 20));
    // Without the effect the move is fine, and so is a move along the audio track.
    apply(&av_both(), vec![mv("c2", "t1", 20)]);
    apply(&p, vec![mv("c2", "t2", 20)]);
}

#[test]
fn move_visual_effect_onto_audio_track_refused() {
    let p = apply(&av_both(), vec![add("c1", "$fx", "blur", &[])]).project;
    let e = try_apply(&p, vec![mv("c1", "t2", 20)]).unwrap_err();
    mismatch(&e, "c1", "blur", "audio");
    replays(&p, mv("c1", "t2", 20));
}

/// The compressor is removed, the bare clip moves to video, then the removal is undone: the
/// restore would put an audio effect on a video-track item.
#[test]
fn effect_restore_after_move_refused() {
    let p = apply(&av_both(), vec![add("c2", "$fx", "compressor", &[])]).project;
    let removed = apply(
        &p,
        vec![
            EffectRemove {
                item: "c2".into(),
                effect: "fx1".into(),
            }
            .into(),
        ],
    );
    let moved = apply(&removed.project, vec![mv("c2", "t1", 20)]).project;
    let e = apply_internal(
        &moved,
        &tx(removed.inverse.clone()),
        &mut MemIds::after(&moved),
    )
    .unwrap_err();
    mismatch(&e, "c2", "compressor", "video");
    apply_tx_mode(
        &moved,
        &tx(removed.inverse),
        &mut MemIds::after(&moved),
        ApplyMode::Replay,
    )
    .unwrap();
}

/// An `item.restore` naming a track of the other kind (a damaged or rebased inverse) cannot put
/// an item's audio effects on a video track either.
#[test]
fn item_restore_onto_other_domain_refused() {
    let p = apply(&av_both(), vec![add("c2", "$fx", "compressor", &[])]).project;
    let deleted = apply(&p, vec![delete("c2")]);
    let Op::ItemRestore(mut r) = deleted.inverse[0].clone() else {
        panic!("{:?}", deleted.inverse)
    };
    // As stored it is fine.
    apply_internal(
        &deleted.project,
        &tx(vec![Op::ItemRestore(r.clone())]),
        &mut MemIds::after(&deleted.project),
    )
    .unwrap();
    r.track = "t1".into();
    r.item.start = secs(20);
    let e = apply_internal(
        &deleted.project,
        &tx(vec![Op::ItemRestore(r)]),
        &mut MemIds::after(&deleted.project),
    )
    .unwrap_err();
    mismatch(&e, "c2", "compressor", "video");
}
