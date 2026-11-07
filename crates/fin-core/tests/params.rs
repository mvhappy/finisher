//! The settings registry and `param.set` (plan 4C Task 11): every spec path reads and writes
//! the typed model, defaults match the model's, errors carry their details and fix text, the
//! inverse is exact (resets included), animated paths are refused, conflicts are by parameter,
//! and graphic slots are checked against their template.

mod common;

use std::collections::BTreeMap;

use common::ids_json::ids_cover_every_id_in_json;
use common::*;
use fin_core::params::{
    self, DefaultValue, ParamSpec, ParamType, Scope, ScopeOf, find, specs, validate,
};
use fin_core::{
    ApplyMode, AssetAdd, AssetKind, AudioParams, ChangeKind, CommittedTx, CoreError, Effect,
    EntityKind, ImageInfo, InsertMode, Interp, Item, ItemBody, ItemInsert, Keyframe, LottieInfo,
    LutInfo, Marker, Mask, MemIds, Op, ParamSet, ParamValue, Project, SlotSpec, Track, TrackKind,
    apply_tx, apply_tx_mode, rebase_tx,
};
use serde_json::json;

fn set(item: &str, path: &str, value: Option<ParamValue>) -> Op {
    ParamSet {
        item: item.into(),
        path: path.into(),
        value,
    }
    .into()
}

fn num(x: f64) -> Option<ParamValue> {
    Some(ParamValue::Number(x))
}

fn text(s: &str) -> Option<ParamValue> {
    Some(ParamValue::Text(s.into()))
}

fn asset_ref(id: &str) -> ParamValue {
    ParamValue::Asset { asset: id.into() }
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

fn item_mut<'a>(p: &'a mut Project, id: &str) -> &'a mut Item {
    p.timelines
        .iter_mut()
        .flat_map(|t| &mut t.tracks)
        .flat_map(|t| &mut t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

fn template(slots: &[(&str, &str)]) -> LottieInfo {
    LottieInfo {
        width: 1080,
        height: 1920,
        rate: secs(30),
        duration: secs(3),
        slots: slots
            .iter()
            .map(|(id, kind)| SlotSpec {
                id: (*id).into(),
                kind: (*kind).into(),
            })
            .collect(),
        fonts: Vec::new(),
    }
}

fn library_asset(id: &str, kind: AssetKind) -> Op {
    let mut a = AssetAdd::media(id, format!("/m/{id}"), secs(3), None, None, "fp");
    a.kind = kind;
    match kind {
        AssetKind::LottieTemplate => {
            a.lottie = Some(template(&[
                ("title", "text"),
                ("tint", "color"),
                ("logo", "image"),
                ("size", "number"),
            ]))
        }
        AssetKind::Image => {
            a.duration = None;
            a.image = Some(ImageInfo {
                width: 4,
                height: 4,
            })
        }
        AssetKind::Lut => {
            a.duration = None;
            a.lut = Some(LutInfo { size: 33 })
        }
        _ => unreachable!(),
    }
    a.into()
}

fn graphic(id: &str, at: i64, slots: BTreeMap<String, ParamValue>) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        duration: secs(2),
        mode: InsertMode::Ripple,
        body: ItemBody::Graphic {
            asset: "a2".into(),
            slots,
        },
    }
    .into()
}

/// `c1` (10 s clip, effects `fx9` blur and `fx10` lut, a mask) on `t1`; `a2` a template, `a3`
/// an image, `a4` a LUT; graphic `g1` after `c1`; marker `m1` on `tl1`.
fn library() -> Project {
    let p = with_clips(&[10]);
    let mut p = apply(
        &p,
        vec![
            library_asset("$t", AssetKind::LottieTemplate),
            library_asset("$i", AssetKind::Image),
            library_asset("$l", AssetKind::Lut),
            graphic("$g", 10, BTreeMap::new()),
        ],
    )
    .project;
    let c1 = item_mut(&mut p, "c1");
    c1.common.effects = vec![
        Effect {
            id: "fx9".into(),
            kind: "blur".into(),
            enabled: true,
            params: BTreeMap::new(),
        },
        Effect {
            id: "fx10".into(),
            kind: "lut".into(),
            enabled: true,
            params: BTreeMap::new(),
        },
    ];
    c1.common.mask = Some(Mask {
        matte: "mat1".into(),
        feather: 0.0,
        choke: 0.0,
        invert: false,
    });
    p.timelines[0].markers.push(Marker {
        id: "m1".into(),
        at: secs(1),
        duration: None,
        label: "Intro".into(),
        color: None,
    });
    p
}

/// A valid value of `spec`'s type other than its default.
fn sample(spec: &ParamSpec) -> ParamValue {
    let differs = |v: ParamValue| match (&spec.default, &v) {
        (DefaultValue::Number(d), ParamValue::Number(x)) => d.to_bits() != x.to_bits(),
        _ => true,
    };
    match spec.ty {
        ParamType::Number { min, max } => {
            let v = ParamValue::Number(max);
            if differs(v.clone()) {
                v
            } else {
                ParamValue::Number(min)
            }
        }
        ParamType::Bool => ParamValue::Bool(!matches!(spec.default, DefaultValue::Bool(true))),
        ParamType::Enum { values } => ParamValue::Text(values[values.len() - 1].into()),
        ParamType::Color => ParamValue::Text("#11223344".into()),
        ParamType::Vec2 { max, .. } => ParamValue::Vec2(max),
        ParamType::Time { .. } => ParamValue::Text("1/2".into()),
        ParamType::Rational { min, .. } => ParamValue::Text(min.into()),
        ParamType::Size { min, .. } => ParamValue::Vec2([f64::from(min), f64::from(min)]),
        ParamType::Text { .. } => ParamValue::Text("x".into()),
        ParamType::AssetRef { kinds } => asset_ref(match kinds[0] {
            "lut" => "a4",
            _ => "a3",
        }),
    }
}

/// The value a spec's default stands for.
fn default_value(spec: &ParamSpec) -> Option<ParamValue> {
    match spec.default {
        DefaultValue::None => None,
        DefaultValue::Bool(b) => Some(ParamValue::Bool(b)),
        DefaultValue::Number(x) => Some(ParamValue::Number(x)),
        DefaultValue::Vec2(v) => Some(ParamValue::Vec2(v)),
        DefaultValue::Text(s) => Some(ParamValue::Text(s.into())),
    }
}

/// The item path that addresses `spec` on `library()`'s items: an effect param through `c1`'s
/// effect of that kind, a slot through `g1`.
fn item_target(spec: &ParamSpec) -> Option<(&'static str, String)> {
    match spec.scope {
        Scope::Item { kinds } if kinds.contains(&"clip") => Some(("c1", spec.path.to_string())),
        Scope::Item { .. } => {
            let slot = match spec.ty {
                ParamType::Text { .. } => "title",
                ParamType::Color => "tint",
                ParamType::AssetRef { .. } => "logo",
                ParamType::Number { .. } => "size",
                _ => panic!("slot spec {spec:?}"),
            };
            Some(("g1", format!("slots.{slot}")))
        }
        Scope::Effect { kind } => {
            let fx = match kind {
                "blur" => "fx9",
                "lut" => "fx10",
                _ => return None,
            };
            let param = spec.path.split_once('.').unwrap().1;
            Some(("c1", format!("effects.{fx}.{param}")))
        }
        _ => None,
    }
}

#[test]
fn every_spec_path_gets_and_sets() {
    let p = library();
    let mut seen = 0;
    for spec in specs() {
        let value = sample(spec);
        validate(spec, &value).unwrap_or_else(|e| panic!("{}: {e:?}", spec.path));
        match spec.scope {
            Scope::Item { .. } | Scope::Effect { .. } => {
                let Some((id, path)) = item_target(spec) else {
                    // colorAdjust: through an effect of that kind on a fresh item.
                    let mut it = Item::clip("c7", "a1", secs(0), secs(0), secs(1));
                    it.common.effects.push(Effect {
                        id: "fx1".into(),
                        kind: "colorAdjust".into(),
                        enabled: true,
                        params: BTreeMap::new(),
                    });
                    let path = format!("effects.fx1.{}", spec.path.split_once('.').unwrap().1);
                    params::set(&mut it, &path, Some(value.clone())).unwrap();
                    assert_eq!(params::get(&it, &path).unwrap(), Some(value), "{path}");
                    seen += 1;
                    continue;
                };
                let a = apply(&p, vec![set(id, &path, Some(value.clone()))]);
                let got = params::get(item(&a.project, id), &path).unwrap();
                assert_eq!(got, Some(value.clone()), "{path}");
                // A reset is the default again; sparse values (audio params an item did not
                // have, effect params, slots) are no longer stored.
                let back = apply(&a.project, vec![set(id, &path, None)]);
                let got = params::get(item(&back.project, id), &path).unwrap();
                let sparse = path.starts_with("audio.")
                    || path.starts_with("slots.")
                    || (path.starts_with("effects.") && !path.ends_with(".enabled"));
                let want = if sparse { None } else { default_value(spec) };
                assert_eq!(got, want, "{path} reset");
                seen += 1;
            }
            Scope::Track => {
                let mut t = Track::new("t9", TrackKind::Audio, "A9");
                let old = params::set_track(&mut t, spec.path, Some(value.clone())).unwrap();
                if spec.default != DefaultValue::None {
                    assert_eq!(old, default_value(spec), "{}", spec.path);
                }
                assert_eq!(params::get_track(&t, spec.path).unwrap(), Some(value));
                seen += 1;
            }
            Scope::Timeline => {
                let mut tl = p.timelines[0].clone();
                tl.tracks.iter_mut().for_each(|t| t.items.clear());
                params::set_timeline(&mut tl, spec.path, Some(value.clone())).unwrap();
                assert_eq!(params::get_timeline(&tl, spec.path).unwrap(), Some(value));
                seen += 1;
            }
            Scope::Marker => {
                let mut m = p.timelines[0].markers[0].clone();
                params::set_marker(&mut m, spec.path, Some(value.clone())).unwrap();
                assert_eq!(params::get_marker(&m, spec.path).unwrap(), Some(value));
                seen += 1;
            }
            Scope::Style => {
                // On the default style: the registry's default is the default style's; a set
                // reads back; a reset is the default again (or nothing, for optional parts).
                let base = fin_core::CaptionStyle::default();
                let mut st = base.clone();
                let was = params::get_style(&base, spec.path).unwrap();
                if spec.path != "name" {
                    assert_eq!(was, default_value(spec), "{} default", spec.path);
                }
                let old = params::set_style(&mut st, spec.path, Some(value.clone())).unwrap();
                assert_eq!(old, was, "{}", spec.path);
                assert_eq!(params::get_style(&st, spec.path).unwrap(), Some(value));
                if spec.path != "name" {
                    params::set_style(&mut st, spec.path, None).unwrap();
                    assert_eq!(st, base, "{} reset", spec.path);
                }
                seen += 1;
            }
        }
    }
    assert_eq!(seen, specs().len(), "every spec was exercised");
}

#[test]
fn defaults_match_model_defaults() {
    let mut clip = Item::clip("c1", "a1", secs(0), secs(0), secs(1));
    clip.common.mask = Some(Mask {
        matte: "mat1".into(),
        feather: 0.0,
        choke: 0.0,
        invert: false,
    });
    let mut voiced = clip.clone();
    voiced.common.audio = Some(AudioParams::default());
    let track = Track::new("t1", TrackKind::Video, "V1");
    let p = project();
    let marker = Marker {
        id: "m1".into(),
        at: secs(0),
        duration: None,
        label: String::new(),
        color: None,
    };
    for spec in specs() {
        let want = default_value(spec);
        let got = match spec.scope {
            Scope::Item { kinds } if kinds.contains(&"clip") => {
                if spec.path.starts_with("audio.") {
                    // No audio params: nothing stored; the default ones read as the default.
                    assert_eq!(params::get(&clip, spec.path).unwrap(), None);
                    params::get(&voiced, spec.path).unwrap()
                } else {
                    params::get(&clip, spec.path).unwrap()
                }
            }
            // A track's name is per track.
            Scope::Track if spec.path == "name" => continue,
            Scope::Track => params::get_track(&track, spec.path).unwrap(),
            Scope::Timeline => {
                // A timeline's name and rate are the project's; only the resolution defaults.
                if spec.path != "resolution" {
                    continue;
                }
                params::get_timeline(&p.timelines[0], spec.path).unwrap()
            }
            Scope::Marker => {
                if spec.path == "at" || spec.path == "label" {
                    continue;
                }
                params::get_marker(&marker, spec.path).unwrap()
            }
            // Slots and effect params are sparse: nothing stored reads as absent, and the
            // spec's default is what evaluation uses.
            Scope::Item { .. } | Scope::Effect { .. } | Scope::Style => continue,
        };
        assert_eq!(got, want, "{}", spec.path);
    }
    let fx = Effect {
        id: "fx1".into(),
        kind: "blur".into(),
        enabled: true,
        params: BTreeMap::new(),
    };
    clip.common.effects.push(fx);
    assert_eq!(params::get(&clip, "effects.fx1.radius").unwrap(), None);
    assert_eq!(
        params::get(&clip, "effects.fx1.enabled").unwrap(),
        Some(ParamValue::Bool(true))
    );
}

fn err(p: &Project, op: Op) -> CoreError {
    try_apply(p, vec![op]).unwrap_err()
}

#[test]
fn range_errors_carry_details_and_fix_text() {
    let p = library();
    let e = err(&p, set("c1", "opacity", num(1.5)));
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.message, "opacity 1.5 is outside 0 to 1");
    assert_eq!(
        e.details,
        json!({ "path": "opacity", "value": 1.5, "min": 0.0, "max": 1.0, "unit": null,
                "item": "c1", "opIndex": 0 })
    );
    assert_eq!(e.fix.as_deref(), Some("use an opacity from 0 to 1"));

    let e = err(&p, set("c1", "transform.rotation", num(-4000.0)));
    assert_eq!(
        e.message,
        "transform.rotation -4000 is outside -3600 to 3600 deg"
    );
    assert_eq!(
        e.fix.as_deref(),
        Some("use a rotation from -3600 to 3600 deg")
    );

    let e = err(
        &p,
        set(
            "c1",
            "transform.scale",
            Some(ParamValue::Vec2([1.0, 101.0])),
        ),
    );
    assert_eq!(
        e.message,
        "transform.scale 1,101 is outside -100,-100 to 100,100"
    );
    assert_eq!(e.details["min"], json!([-100.0, -100.0]));
    assert_eq!(
        e.fix.as_deref(),
        Some("use a scale from -100,-100 to 100,100 (x,y; 1 is the original size)")
    );

    let e = err(&p, set("c1", "effects.fx9.radius", num(300.0)));
    assert_eq!(e.message, "effects.fx9.radius 300 is outside 0 to 200 px");
    assert_eq!(e.details["path"], "effects.fx9.radius");
    assert_eq!(e.details["unit"], "px");
    assert_eq!(e.fix.as_deref(), Some("use a blur radius from 0 to 200 px"));

    let e = err(&p, set("c1", "audio.gain", num(30.0)));
    assert_eq!(e.message, "audio.gain 30 is outside -96 to 24 dB");
    assert_eq!(e.fix.as_deref(), Some("use a gain from -96 to 24 dB"));

    let e = err(&p, set("c1", "audio.fadeIn", text("-1")));
    assert_eq!(e.code, "INVALID_RANGE");
    assert_eq!(e.message, "audio.fadeIn -1 is below 0 s");
    assert_eq!(e.fix.as_deref(), Some("use a fade-in of at least 0 s"));

    // Types: the expected type and the spec's hint.
    let e = err(&p, set("c1", "opacity", text("half")));
    assert_eq!(e.code, "PARAM_TYPE");
    assert_eq!(e.message, "opacity takes a number, not text");
    assert_eq!(
        e.details,
        json!({ "path": "opacity", "expected": "number", "got": "text", "item": "c1",
                "opIndex": 0 })
    );
    assert_eq!(e.fix.as_deref(), Some("use an opacity from 0 to 1"));
    let e = err(&p, set("c1", "blend", text("dodge")));
    assert_eq!(e.code, "PARAM_TYPE");
    assert_eq!(
        e.message,
        "blend takes one of normal, multiply, screen, overlay, add, darken, lighten, not \"dodge\""
    );

    // Unknown paths name the closest ones.
    let e = err(&p, set("c1", "opacty", num(1.0)));
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert_eq!(e.message, "a clip item has no parameter opacty");
    assert_eq!(
        e.details["alternatives"],
        json!(["opacity", "speed", "crop.top"])
    );
    assert_eq!(
        e.fix.as_deref(),
        Some("use one of: opacity, speed, crop.top (fin schema param lists every path)")
    );
    let e = err(&p, set("g1", "speed", text("2")));
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert_eq!(e.message, "a graphic item has no parameter speed");
    let e = err(&p, set("c1", "effects.fx77.radius", num(1.0)));
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert_eq!(e.message, "item c1 has no effect fx77");
}

#[test]
fn param_set_inverse_exact() {
    let p = library();
    let cases = vec![
        set("c1", "opacity", num(0.25)),
        set(
            "c1",
            "transform.position",
            Some(ParamValue::Vec2([10.0, -20.5])),
        ),
        set("c1", "blend", text("screen")),
        set("c1", "crop.left", num(0.1)),
        // No audio params yet: the undo takes them away again.
        set("c1", "audio.gain", num(-6.0)),
        set("c1", "audio.fadeOut", text("1/2")),
        set("c1", "mask.invert", Some(ParamValue::Bool(true))),
        set("c1", "speed", text("1/2")),
        // A sparse effect param: the undo leaves it unset again.
        set("c1", "effects.fx9.radius", num(12.0)),
        set("c1", "effects.fx9.enabled", Some(ParamValue::Bool(false))),
        set("c1", "effects.fx10.asset", Some(asset_ref("a4"))),
        set("g1", "slots.title", text("Hello")),
        set("g1", "slots.logo", Some(asset_ref("a3"))),
        // A reset of a default value.
        set("c1", "opacity", None),
    ];
    for op in cases {
        let a = golden_apply(&p, vec![op.clone()]);
        ids_cover_every_id_in_json(&op);
        assert!(a.diff.shifts.is_empty(), "{op:?}");
        let [c] = &a.diff.changes[..] else {
            panic!("{:?}", a.diff)
        };
        let Op::ParamSet(s) = &op else { unreachable!() };
        assert_eq!(c.change, ChangeKind::Updated);
        assert_eq!(c.path.as_deref(), Some(s.path.as_str()));
        assert_eq!(c.track.as_deref(), Some("t1"));
        if let Some(fx) = s.path.strip_prefix("effects.") {
            // An effect's parameter is the effect's: `<item>/<fx>` (Decision 20).
            let fx = fx.split('.').next().unwrap();
            assert_eq!(
                (c.kind, c.id.as_str()),
                (EntityKind::Effect, &*format!("c1/{fx}"))
            );
        } else {
            assert_eq!((c.kind, c.id.as_str()), (EntityKind::Item, s.item.as_str()));
        }
    }
    // Explicit defaults stored in a sparse map come back as they were.
    let mut q = p.clone();
    item_mut(&mut q, "c1").common.effects[0]
        .params
        .insert("radius".into(), ParamValue::Number(0.0));
    golden_apply(&q, vec![set("c1", "effects.fx9.radius", num(4.0))]);
    // Values are canonical: -0.0 is stored as 0.0, non-finite numbers are refused.
    let a = apply(&p, vec![set("c1", "transform.rotation", num(-0.0))]);
    assert_eq!(
        item(&a.project, "c1").common.transform.rotation.to_bits(),
        0f64.to_bits()
    );
    let e = err(&p, set("c1", "transform.rotation", num(f64::NAN)));
    assert_eq!(
        (e.code.as_str(), &e.details["path"]),
        ("INVALID_RANGE", &json!("transform.rotation"))
    );
    let n = fin_core::normalize(tx(vec![set("c1", "opacity", num(-0.0))])).unwrap();
    let Op::ParamSet(s) = &n.ops[0] else { panic!() };
    assert!(matches!(s.value, Some(ParamValue::Number(x)) if x.to_bits() == 0));
}

#[test]
fn param_set_placeholders_resolve() {
    let p = library();
    let a = apply(
        &p,
        vec![
            library_asset("$lut2", AssetKind::Lut),
            set("c1", "effects.fx10.asset", Some(asset_ref("$lut2"))),
        ],
    );
    let lut = &a.bindings["$lut2"];
    assert_eq!(
        params::get(item(&a.project, "c1"), "effects.fx10.asset").unwrap(),
        Some(asset_ref(lut))
    );
    // An asset of another kind, or none, is refused.
    let e = err(&p, set("c1", "effects.fx10.asset", Some(asset_ref("a3"))));
    assert_eq!(e.code, "PARAM_TYPE");
    let e = err(&p, set("c1", "effects.fx10.asset", Some(asset_ref("a99"))));
    assert_eq!(e.code, "NOT_FOUND");
}

#[test]
fn param_set_on_animated_path_refused() {
    let mut p = library();
    item_mut(&mut p, "c1").common.keyframes.insert(
        "opacity".into(),
        vec![Keyframe {
            at: secs(0),
            value: ParamValue::Number(0.0),
            interp: Interp::Linear,
        }],
    );
    let e = err(&p, set("c1", "opacity", num(0.5)));
    assert_eq!(e.code, "PARAM_ANIMATED");
    assert_eq!(e.details["path"], "opacity");
    assert_eq!(e.details["keyframes"], 1);
    assert_eq!(
        e.fix.as_deref(),
        Some("change its keyframes (keyframe.set, keyframe.remove), or remove them all first")
    );
    // Policy: replay applies it; an unknown path is structural.
    let t = tx(vec![set("c1", "opacity", num(0.5))]);
    apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap();
    let t = tx(vec![set("c1", "opacity", num(7.0))]);
    apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap();
    let t = tx(vec![set("c1", "nope", num(0.5))]);
    let e = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    let t = tx(vec![set("c1", "opacity", text("x"))]);
    let e = apply_tx_mode(&p, &t, &mut MemIds::after(&p), ApplyMode::Replay).unwrap_err();
    assert_eq!(e.code, "PARAM_TYPE");
}

#[test]
fn param_set_speed_keeps_the_source_in_range() {
    let p = with_clips(&[60]);
    // 60 s at speed 2 reads 120 s of a 100 s asset.
    let e = err(&p, set("c1", "speed", text("2")));
    assert_eq!(e.code, "INVALID_RANGE", "{e:?}");
    let a = golden_apply(&p, vec![set("c1", "speed", text("3/2"))]);
    assert_eq!(clip(&a.project, "c1").speed, r("3/2"));
    let e = err(&p, set("c1", "speed", text("0")));
    assert_eq!(e.code, "INVALID_RANGE", "{e:?}");
}

#[test]
fn param_set_on_locked_track_refused() {
    let mut p = library();
    p.timelines[0].tracks[0].lock = true;
    assert_eq!(err(&p, set("c1", "opacity", num(0.5))).code, "TRACK_LOCKED");
}

// ---- conflicts ----

fn commit(p: &Project, ops: Vec<Op>, rev: u64, tx_id: &str) -> (Project, CommittedTx) {
    let mut t = tx(ops);
    t.tx_id = tx_id.into();
    t.base_rev = rev - 1;
    let a = apply_tx(p, &t, &mut MemIds::after(p)).unwrap_or_else(|e| panic!("{tx_id}: {e}"));
    (
        a.project,
        CommittedTx {
            rev,
            tx: t,
            diff: a.diff,
            bindings: a.bindings,
            link: None,
        },
    )
}

fn rebase_ops(base: &Project, theirs: Vec<Op>, ours: Vec<Op>) -> Result<Project, CoreError> {
    let (head, c) = commit(base, theirs, 1, "tx-a");
    let mut t = tx(ours);
    t.tx_id = "tx-b".into();
    rebase_tx(base, &head, &t, &[c], &mut MemIds::after(&head)).map(|r| r.applied.project)
}

fn conflict(e: &CoreError) -> (String, Option<String>) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    (
        e.details["reason"].as_str().unwrap().to_string(),
        e.details["againstTx"].as_str().map(str::to_string),
    )
}

#[test]
fn same_param_conflicts_disjoint_params_merge() {
    let p = library();
    let same = ("SAME_PARAM".to_string(), Some("tx-a".to_string()));
    let e = rebase_ops(
        &p,
        vec![set("c1", "opacity", num(0.5))],
        vec![set("c1", "opacity", num(0.2))],
    )
    .unwrap_err();
    assert_eq!(conflict(&e), same);
    let e = rebase_ops(
        &p,
        vec![set("c1", "effects.fx9.radius", num(3.0))],
        vec![set("c1", "effects.fx9.radius", num(4.0))],
    )
    .unwrap_err();
    assert_eq!(conflict(&e), same);
    // Disjoint parameters merge, effect params of different effects too.
    let got = rebase_ops(
        &p,
        vec![
            set("c1", "opacity", num(0.5)),
            set("c1", "effects.fx9.radius", num(3.0)),
        ],
        vec![
            set("c1", "transform.rotation", num(90.0)),
            set("c1", "effects.fx10.intensity", num(0.5)),
        ],
    )
    .unwrap();
    let c1 = item(&got, "c1");
    assert_eq!(c1.common.opacity, 0.5);
    assert_eq!(c1.common.transform.rotation, 90.0);
    assert_eq!(
        params::get(c1, "effects.fx9.radius").unwrap(),
        Some(ParamValue::Number(3.0))
    );
    // A set on a deleted item is REMOVED.
    let e = rebase_ops(&p, vec![delete("c1")], vec![set("c1", "opacity", num(0.2))]).unwrap_err();
    assert_eq!(conflict(&e).0, "REMOVED");
}

// ---- graphic slots ----

#[test]
fn graphic_slots_validated_against_template() {
    let p = library();
    let slots = |k: &str, v: ParamValue| BTreeMap::from([(k.to_string(), v)]);
    let e = try_apply(
        &p,
        vec![graphic(
            "$g",
            12,
            slots("nope", ParamValue::Text("x".into())),
        )],
    )
    .unwrap_err();
    assert_eq!(e.code, "SLOT_UNKNOWN");
    assert_eq!(e.message, "template a2 has no slot nope");
    assert_eq!(e.details["slots"], json!(["title", "tint", "logo", "size"]));
    assert_eq!(
        e.fix.as_deref(),
        Some("use one of the template's slots: title, tint, logo, size")
    );
    let e = try_apply(
        &p,
        vec![graphic(
            "$g",
            12,
            slots("size", ParamValue::Text("x".into())),
        )],
    )
    .unwrap_err();
    assert_eq!(e.code, "PARAM_TYPE");
    assert_eq!(e.details["path"], "slots.size");
    let e = try_apply(
        &p,
        vec![graphic(
            "$g",
            12,
            slots("tint", ParamValue::Text("red".into())),
        )],
    )
    .unwrap_err();
    assert_eq!(e.code, "PARAM_TYPE");
    // Image slots name an image asset, by placeholder too.
    let ok = graphic("$g", 12, slots("logo", asset_ref("$img")));
    ids_cover_every_id_in_json(&ok);
    let a = apply(&p, vec![library_asset("$img", AssetKind::Image), ok]);
    let g = item(&a.project, &a.bindings["$g"]);
    let ItemBody::Graphic { slots: got, .. } = &g.body else {
        panic!()
    };
    assert_eq!(got["logo"], asset_ref(&a.bindings["$img"]));
    let e = try_apply(&p, vec![graphic("$g", 12, slots("logo", asset_ref("a4")))]).unwrap_err();
    assert_eq!(e.code, "PARAM_TYPE");
    // param.set checks against the template too.
    assert_eq!(
        err(&p, set("g1", "slots.nope", text("x"))).code,
        "SLOT_UNKNOWN"
    );
    assert_eq!(
        err(&p, set("g1", "slots.size", text("x"))).code,
        "PARAM_TYPE"
    );
    assert_eq!(
        err(&p, set("g1", "slots.tint", text("#ABCDEF"))).code,
        "PARAM_TYPE"
    );
    apply(&p, vec![set("g1", "slots.tint", text("#abcdef"))]);
}

// ---- the registry ----

#[test]
fn specs_serialise_deterministically() {
    let a = serde_json::to_string(specs()).unwrap();
    let b = serde_json::to_string(specs()).unwrap();
    assert_eq!(a, b);
    let opacity = specs().iter().find(|s| s.path == "opacity").unwrap();
    assert_eq!(
        serde_json::to_value(opacity).unwrap(),
        json!({
            "path": "opacity",
            "scope": { "type": "item", "kinds": ["clip", "caption", "graphic", "image", "adjustment", "nested"] },
            "type": { "type": "number", "min": 0.0, "max": 1.0 },
            "animatable": true,
            "default": 1.0,
            "unit": null,
            "fix": "use an opacity from {min} to {max}",
        })
    );
    // Paths are unique per scope (slot specs share theirs by slot kind).
    let mut keys: Vec<String> = specs()
        .iter()
        .map(|s| format!("{:?}|{}|{:?}", s.scope, s.path, s.ty))
        .collect();
    keys.sort();
    let n = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), n);
    // The built-in effects (Decision 27) and the scopes Task 11 lists.
    let paths: Vec<&str> = specs().iter().map(|s| s.path).collect();
    for want in [
        "colorAdjust.exposure",
        "colorAdjust.contrast",
        "colorAdjust.saturation",
        "colorAdjust.temperature",
        "colorAdjust.tint",
        "lut.asset",
        "lut.intensity",
        "blur.radius",
        "name",
        "mute",
        "solo",
        "lock",
        "gain",
        "resolution",
        "rate",
        "label",
        "color",
        "at",
        "duration",
    ] {
        assert!(paths.contains(&want), "{want}");
    }
    assert_eq!(params::effect_kinds(), ["colorAdjust", "lut", "blur"]);
    // The schema names ParamSpec.
    assert!(fin_core::json_schema()["$defs"]["ParamSpec"].is_object());
}

#[test]
fn find_by_scope() {
    let p = library();
    let c1 = item(&p, "c1");
    let scope = ScopeOf::Item {
        item: c1,
        template: None,
    };
    assert_eq!(
        find(&scope, "effects.fx9.radius").unwrap().path,
        "blur.radius"
    );
    assert_eq!(find(&scope, "mask.feather").unwrap().path, "mask.feather");
    let mut bare = c1.clone();
    bare.common.mask = None;
    let scope = ScopeOf::Item {
        item: &bare,
        template: None,
    };
    let e = find(&scope, "mask.feather").unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert_eq!(e.message, "item c1 has no mask");
    assert_eq!(find(&ScopeOf::Track, "gain").unwrap().unit, Some("dB"));
    assert_eq!(
        find(&ScopeOf::Effect { kind: "lut" }, "intensity")
            .unwrap()
            .path,
        "lut.intensity"
    );
    assert_eq!(
        find(&ScopeOf::Marker, "nope").unwrap_err().code,
        "PARAM_UNKNOWN"
    );
    let e = find(&ScopeOf::Effect { kind: "glow" }, "x").unwrap_err();
    assert_eq!(e.code, "PARAM_UNKNOWN");
    assert_eq!(e.message, "no effect kind glow");
    assert_eq!(
        e.details["alternatives"],
        json!(["colorAdjust", "lut", "blur"])
    );
}

/// A size's bounds are its range (policy), not its shape: whole non-negative pixels of any
/// size are well-formed, so a stored size past today's maximum still replays.
#[test]
fn size_bounds_are_a_range() {
    let spec = find(&ScopeOf::Timeline, "resolution").unwrap();
    let ParamType::Size { max, .. } = spec.ty else {
        panic!("{spec:?}")
    };
    let big = ParamValue::Vec2([f64::from(max) + 2.0, 2.0]);
    params::check_shape(spec, "resolution", &big).unwrap();
    let e = params::check_range(spec, "resolution", &big).unwrap_err();
    assert_eq!(e.code, "INVALID_RANGE");
    for bad in [[1.5, 2.0], [-2.0, 2.0], [f64::from(u32::MAX) + 1.0, 2.0]] {
        let e = params::check_shape(spec, "resolution", &ParamValue::Vec2(bad)).unwrap_err();
        assert_eq!(e.code, "PARAM_TYPE", "{bad:?}");
    }
}
