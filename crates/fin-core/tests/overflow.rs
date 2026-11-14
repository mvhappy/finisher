//! Measurement in the edit path and eval (plan 4C Task 20, Decision 36): the caption overflow
//! check runs once at the end of a transaction with a measurer, on what the transaction
//! changed; stale commits, merges and undos re-check at the head; replay never measures; an
//! unpinned or byte-less caption is unmeasured and never blocks.
//!
//! Captions use project style `st1` (the default: Inter 700 at 64 px, the generic 9:16 zone,
//! 972 px wide on 1080×1920, two lines). [`FakeMeasurer`] gives every character 0.5 em, so a
//! 12-character word is 384 px at 64 px and 1200 px at 200 px.

mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use common::memlog::MemLog;
use common::stale::conflict_reason;
use common::*;
use fin_core::eval::{UnmeasuredReason, eval_frame};
use fin_core::layout::TextMeasurer;
use fin_core::layout::testing::{CountingMeasurer, FakeMeasurer, PanicMeasurer};
use fin_core::{
    Applied, ApplyEnv, ApplyMode, CaptionBlock, CaptionSetStyle, CaptionSetText, CoreError, Edge,
    FontFormat, FontPin, FontPinOp, FontRepin, FontSource, InsertMode, ItemBody, ItemInsert,
    ItemMove, MemIds, Op, ParamValue, Project, Rational, StyleAdd, StyleSet, TimelineSet, Track,
    TrackKind, Word, apply_tx_env,
};

const INTER: &str = "Inter|700|normal";
const LONG: &str = "abcdefghijkl";

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 64).collect()
}

fn inter(sha256: &str) -> FontPin {
    FontPin {
        family: "Inter".into(),
        weight: 700,
        italic: false,
        face: 0,
        sha256: sha256.into(),
        size: 1000,
        format: FontFormat::Ttf,
        scripts: vec!["Cyrl".into(), "Grek".into(), "Latn".into()],
        source: FontSource::Catalog {
            entry: "inter".into(),
            style: "bold".into(),
            catalog: "2026.11.1".into(),
        },
    }
}

fn caption(id: &str, at: i64, text: &str) -> Op {
    ItemInsert {
        id: id.into(),
        track: "t3".into(),
        at: secs(at),
        duration: secs(2),
        mode: InsertMode::Overwrite,
        body: ItemBody::Caption {
            block: CaptionBlock {
                words: text
                    .split_whitespace()
                    .enumerate()
                    .map(|(i, t)| Word {
                        text: t.into(),
                        start: Rational::new(i as i64, 10).unwrap(),
                        end: Rational::new(i as i64 + 1, 10).unwrap(),
                    })
                    .collect(),
                style: "st1".into(),
                overrides: BTreeMap::new(),
                source: None,
            },
        },
    }
    .into()
}

fn set_text(item: &str, text: &str) -> Op {
    CaptionSetText {
        item: item.into(),
        text: text.into(),
    }
    .into()
}

fn size(v: f64) -> Op {
    StyleSet {
        style: "st1".into(),
        path: "text.size".into(),
        value: Some(ParamValue::Number(v)),
    }
    .into()
}

/// Caption track `t3`, style `st1`, and (when `pinned`) Inter 700 pinned.
fn base(pinned: bool) -> Project {
    let mut p = project();
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Caption, "C1"));
    let mut ops: Vec<Op> = vec![
        StyleAdd {
            id: "$st".into(),
            name: "Mine".into(),
            from: None,
            set: BTreeMap::new(),
        }
        .into(),
    ];
    if pinned {
        ops.push(
            FontPinOp {
                key: INTER.into(),
                pin: inter(&sha('a')),
                fallback: None,
            }
            .into(),
        );
    }
    apply(&p, ops).project
}

/// [`base`] with `cap1` "hi there" at 0 and `cap2` `LONG` at 3.
fn captioned() -> Project {
    apply(
        &base(true),
        vec![caption("$a", 0, "hi there"), caption("$b", 3, LONG)],
    )
    .project
}

fn measured(p: &Project, ops: Vec<Op>, m: &dyn TextMeasurer) -> Result<Applied, CoreError> {
    apply_tx_env(
        p,
        &tx(ops),
        &mut MemIds::after(p),
        ApplyMode::Client,
        ApplyEnv::measuring(Some(m)),
    )
}

#[test]
fn client_insert_overflow_refused() {
    let p = base(true);
    let m = FakeMeasurer::default();
    let e = measured(
        &p,
        vec![caption("$a", 0, "supercalifragilisticexpialidocious")],
        &m,
    )
    .unwrap_err();
    assert_eq!(e.code, "CAPTION_OVERFLOW");
    let d = &e.details;
    assert_eq!(
        (
            d["item"].as_str(),
            d["lines"].as_u64(),
            d["maxLines"].as_u64()
        ),
        (Some("cap1"), Some(1), Some(2))
    );
    assert_eq!(
        (d["width"].as_f64(), d["maxWidth"].as_f64()),
        (Some(1088.0), Some(972.0))
    );
    assert_eq!(d["opIndex"], 0);
    assert!(e.fix.is_some());
    // Too many lines overflows too; a text that fits is accepted.
    let three = format!("{LONG} {LONG} {LONG} {LONG} {LONG} {LONG}");
    let e = measured(&p, vec![caption("$a", 0, &three)], &m).unwrap_err();
    assert_eq!(e.details["lines"], 3);
    measured(&p, vec![caption("$a", 0, "hi there")], &m).unwrap();
    // Without a measurer nothing is measured.
    try_apply(
        &p,
        vec![caption("$a", 0, "supercalifragilisticexpialidocious")],
    )
    .unwrap();
}

#[test]
fn unpinned_or_missing_bytes_never_block() {
    let long = caption("$a", 0, "supercalifragilisticexpialidocious");
    // No pin for the style's face: unmeasured, accepted.
    let m = CountingMeasurer::default();
    measured(&base(false), vec![long.clone()], &m).unwrap();
    assert_eq!(m.calls(), 0);
    // A pin whose bytes the measurer lacks: unmeasured, accepted.
    let lacking = FakeMeasurer::lacking(&[&sha('a')]);
    let a = measured(&base(true), vec![long.clone()], &lacking).unwrap();
    // The result is the one made without a measurer.
    assert_eq!(a, apply(&base(true), vec![long]));
}

#[test]
fn overflow_checked_once_at_tx_end() {
    let p = captioned();
    let m = CountingMeasurer::default();
    // An intermediate state may overflow: only the end of the tx is checked, once.
    let long = format!("{LONG}{LONG}{LONG}");
    measured(
        &p,
        vec![set_text("cap1", &long), set_text("cap1", "ok")],
        &m,
    )
    .unwrap();
    assert_eq!(m.calls(), 1);
    let e = measured(
        &p,
        vec![set_text("cap1", "ok"), set_text("cap1", &long)],
        &m,
    )
    .unwrap_err();
    assert_eq!(
        (e.code.as_str(), &e.details["opIndex"]),
        ("CAPTION_OVERFLOW", &1.into())
    );
    // A clip edit measures nothing.
    let m = CountingMeasurer::default();
    let q = apply(&p, vec![av_asset("$v", secs(10))]).project;
    measured(
        &q,
        vec![insert("$c", "t1", secs(0), "a1", secs(0), secs(1))],
        &m,
    )
    .unwrap();
    assert_eq!(m.calls(), 0);
}

#[test]
fn style_set_rechecks_every_user() {
    let p = captioned();
    let m = CountingMeasurer::default();
    measured(&p, vec![size(80.0)], &m).unwrap();
    assert_eq!(m.calls(), 2);
    // cap2's long word no longer fits at 200 px.
    let e = measured(&p, vec![size(200.0)], &m).unwrap_err();
    assert_eq!(e.details["item"], "cap2");
}

#[test]
fn resolution_change_rechecks_timeline() {
    let p = captioned();
    let resize = |w: f64, h: f64| -> Op {
        TimelineSet {
            timeline: "tl1".into(),
            path: "resolution".into(),
            value: Some(ParamValue::Vec2([w, h])),
        }
        .into()
    };
    let m = CountingMeasurer::default();
    measured(&p, vec![resize(1080.0, 1350.0)], &m).unwrap();
    assert_eq!(m.calls(), 2);
    // 400 px wide: the zone is 360 px, under cap2's 384.
    let e = measured(&p, vec![resize(400.0, 712.0)], &m).unwrap_err();
    assert_eq!(e.details["item"], "cap2");
}

#[test]
fn repin_rechecks_users() {
    let p = captioned();
    let m = CountingMeasurer::default();
    let repin: Op = FontRepin {
        key: INTER.into(),
        pin: inter(&sha('b')),
    }
    .into();
    measured(&p, vec![repin.clone()], &m).unwrap();
    assert_eq!(m.calls(), 2);
    // The new file's bytes are missing: unmeasured, accepted.
    measured(&p, vec![repin], &FakeMeasurer::lacking(&[&sha('b')])).unwrap();
}

#[test]
fn replay_never_calls_the_measurer() {
    let p = base(true);
    let ops = ops_again();
    let a = apply(&p, ops.clone());
    let replayed = apply_tx_env(
        &p,
        &tx(ops),
        &mut MemIds::after(&p),
        ApplyMode::Replay,
        ApplyEnv::measuring(Some(&PanicMeasurer)),
    )
    .unwrap();
    assert_eq!(replayed.project, a.project);
    // `apply_committed` takes no measurer at all.
    let back = fin_core::apply_committed(&p, &tx(ops_again()), &a.bindings).unwrap();
    assert_eq!(back.project, a.project);
}

fn ops_again() -> Vec<Op> {
    vec![
        caption("$a", 0, "supercalifragilisticexpialidocious"),
        size(500.0),
    ]
}

#[test]
fn eval_reports_unmeasured_with_reason() {
    let unpinned = apply(&base(false), vec![caption("$a", 0, "hi")]).project;
    let m = FakeMeasurer::default();
    let caption_of = |p: &Project, m: &dyn TextMeasurer| {
        let plan = eval_frame(p, "tl1", secs(1), Some(m)).unwrap();
        plan.layers[0].caption.clone().unwrap()
    };
    let c = caption_of(&unpinned, &m);
    let why = c.unmeasured.unwrap();
    // The face's key, then each script no chosen pin covers.
    assert_eq!(
        (why.reason, why.keys),
        (
            UnmeasuredReason::FontUnpinned,
            vec![INTER.to_string(), "script:Latn".into()]
        )
    );
    // A script no pin covers.
    let hebrew = apply(&base(true), vec![caption("$a", 0, "שלום")]).project;
    let why = caption_of(&hebrew, &m).unmeasured.unwrap();
    assert_eq!(why.keys, ["script:Hebr"]);
    // Pinned but the bytes are missing.
    let p = apply(&base(true), vec![caption("$a", 0, "hi")]).project;
    let why = caption_of(&p, &FakeMeasurer::lacking(&[&sha('a')]))
        .unmeasured
        .unwrap();
    assert_eq!(
        (why.reason, why.keys),
        (UnmeasuredReason::FontMissing, vec![INTER.to_string()])
    );
    // No measurer.
    let plan = eval_frame(&p, "tl1", secs(1), None).unwrap();
    let c = plan.layers[0].caption.clone().unwrap();
    assert_eq!(c.unmeasured.unwrap().reason, UnmeasuredReason::NoMeasurer);
    assert!(plan.text_engine.is_none());
}

#[test]
fn measurement_resumes_when_bytes_arrive() {
    let p = apply(&base(true), vec![caption("$a", 0, "hi there")]).project;
    let before = eval_frame(
        &p,
        "tl1",
        secs(1),
        Some(&FakeMeasurer::lacking(&[&sha('a')])),
    )
    .unwrap();
    assert!(before.layers[0].caption.as_ref().unwrap().layout.is_none());
    let after = eval_frame(&p, "tl1", secs(1), Some(&FakeMeasurer::default())).unwrap();
    let c = after.layers[0].caption.as_ref().unwrap();
    let layout = c.layout.as_ref().unwrap();
    assert!(c.unmeasured.is_none() && !layout.overflow);
    assert_eq!(layout.faces, [sha('a')]);
    assert_eq!(after.text_engine.as_deref(), Some("fake/1"));
}

fn log() -> MemLog {
    let mut log = MemLog::new(captioned());
    log.measurer = Some(Arc::new(FakeMeasurer::default()));
    log
}

#[test]
fn stale_commit_rechecks_overflow() {
    let mut log = log();
    // Theirs: 200 px, which "hi" and "ok" still fit.
    log.commit("main", "t1", "a", vec![set_text("cap2", "ok"), size(200.0)]);
    // Ours, written at 0: a long word that fits at 64 px, not at the head's 200.
    let e = log
        .commit_at("main", "t2", "b", 0, vec![set_text("cap1", LONG)])
        .unwrap_err();
    assert_eq!(conflict_reason(&e), "CAPTION_OVERFLOW");
    // Without a measurer the same commit merges.
    let mut plain = MemLog::new(captioned());
    plain.commit("main", "t1", "a", vec![set_text("cap2", "ok"), size(200.0)]);
    plain
        .commit_at("main", "t2", "b", 0, vec![set_text("cap1", LONG)])
        .unwrap();
}

#[test]
fn merge_rechecks_overflow() {
    let mut log = log();
    log.fork("b1", "main", 0);
    log.commit("b1", "t1", "a", vec![set_text("cap1", LONG)]);
    log.commit("main", "t2", "a", vec![set_text("cap2", "ok"), size(200.0)]);
    let e = log.merge("b1", "main", "a", "m1", false).unwrap_err();
    assert_eq!(conflict_reason(&e), "CAPTION_OVERFLOW", "{e:?}");
}

#[test]
fn undo_rechecks_overflow() {
    let mut log = log();
    log.commit("main", "t1", "a", vec![set_text("cap2", "ok")]);
    log.commit("main", "t2", "b", vec![size(200.0)]);
    // Undoing t1 brings back cap2's long word, which no longer fits.
    let e = log.undo("main", "a", "u1").unwrap_err();
    assert_eq!(conflict_reason(&e), "CAPTION_OVERFLOW", "{e:?}");
}

fn set(path: &str, value: ParamValue) -> Op {
    StyleSet {
        style: "st1".into(),
        path: path.into(),
        value: Some(value),
    }
    .into()
}

fn restyle(item: &str, style: &str, overrides: &[(&str, ParamValue)]) -> Op {
    CaptionSetStyle {
        item: item.into(),
        style: style.into(),
        overrides: Some(
            overrides
                .iter()
                .map(|(k, v)| ((*k).to_string(), v.clone()))
                .collect(),
        ),
    }
    .into()
}

#[test]
fn edits_that_cannot_change_layout_measure_nothing() {
    let p = captioned();
    let m = CountingMeasurer::default();
    let none = |ops: Vec<Op>| {
        measured(&p, ops, &m).unwrap();
        assert_eq!(m.calls(), 0);
    };
    // A move, an end trim that shows the same words, a colour, a stroke, a colour override.
    none(vec![
        ItemMove {
            item: "cap2".into(),
            track: "t3".into(),
            at: secs(8),
            mode: InsertMode::Overwrite,
        }
        .into(),
    ]);
    none(vec![trim("cap2", Edge::End, secs(-1))]);
    none(vec![set("color", ParamValue::Text("#ff0000".into()))]);
    none(vec![set("stroke.width", ParamValue::Number(4.0))]);
    none(vec![restyle(
        "cap2",
        "st1",
        &[("color", ParamValue::Text("#00ff00".into()))],
    )]);
    // An undo of a delete brings back what was measured before: even at a size its long
    // word no longer fits, it is not re-measured.
    let gone = apply(&p, vec![delete("cap2")]);
    let big = apply(&gone.project, vec![size(200.0)]);
    let undo = apply_tx_env(
        &big.project,
        &tx(gone.inverse.clone()),
        &mut MemIds::after(&big.project),
        ApplyMode::Internal,
        ApplyEnv::measuring(Some(&m)),
    )
    .unwrap();
    assert_eq!(m.calls(), 0);
    assert!(
        undo.project.timelines[0].tracks[2]
            .items
            .iter()
            .any(|i| i.id == "cap2")
    );
}

#[test]
fn edits_that_can_change_layout_are_measured() {
    let p = captioned();
    let m = FakeMeasurer::default();
    let overflows = |ops: Vec<Op>| {
        let e = measured(&p, ops, &m).unwrap_err();
        assert_eq!(
            (e.code.as_str(), &e.details["item"]),
            ("CAPTION_OVERFLOW", &"cap2".into())
        );
    };
    // Words.
    overflows(vec![set_text("cap2", &format!("{LONG}{LONG}{LONG}"))]);
    // A style reference: to a style at 200 px.
    overflows(vec![
        StyleAdd {
            id: "$st".into(),
            name: "Big".into(),
            from: Some("st1".into()),
            set: [("text.size".to_string(), ParamValue::Number(200.0))].into(),
        }
        .into(),
        restyle("cap2", "$st", &[]),
    ]);
    // A layout override, and the style's own layout paths.
    overflows(vec![restyle(
        "cap2",
        "st1",
        &[("text.size", ParamValue::Number(200.0))],
    )]);
    overflows(vec![size(200.0)]);
    overflows(vec![set("background.padding", ParamValue::Number(400.0))]);
    // cap2 on two lines, then one line at most.
    let q = apply(&p, vec![set_text("cap2", &format!("{LONG} {LONG} {LONG}"))]).project;
    let e = measured(&q, vec![set("maxLines", ParamValue::Number(1.0))], &m).unwrap_err();
    assert_eq!(e.details["item"], "cap2");
}
