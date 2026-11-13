//! Review #55: a split keeps the item's gain envelope. Each fade is restricted to each half
//! (spec §5.13): a half its window does not reach drops it, a half it
//! reaches keeps its length with `fadeInOffset`/`fadeOutOffset` advanced by how far the half's
//! edge lies from the original edge. So the amplitude at every source/timeline time is what it
//! was, and the split's inverse restores the left part's audio fields exactly.

mod common;

use common::*;
use fin_core::eval::{ShapeKind, eval_audio};
use fin_core::{
    AudioParams, Curve, Edge, ItemSplit, ItemTrim, Op, ParamSet, ParamValue, Project, Rational,
    TransitionKind, TransitionSet, TrimMode,
};

/// Asset `a2` (audio, 40 s) and clip `c1` on `t2` at 0, `len` seconds, with the given fades
/// (`None`: no audio params set).
fn clip_with_fades(len: i64, fades: Option<(&str, &str)>) -> Project {
    let mut ops = vec![
        audio_asset("$b", secs(40)),
        insert("$c", "t2", secs(0), "$b", secs(0), secs(len)),
    ];
    if let Some((fade_in, fade_out)) = fades {
        ops.push(set("$c", "audio.fadeIn", fade_in));
        ops.push(set("$c", "audio.fadeOut", fade_out));
    }
    let a = apply(&project(), ops);
    assert_eq!(a.bindings["$c"], "c1");
    a.project
}

fn set(item: &str, path: &str, v: &str) -> Op {
    ParamSet {
        item: item.into(),
        path: path.into(),
        value: Some(ParamValue::Text(v.into())),
    }
    .into()
}

fn split(item: &str, offset: Rational) -> Op {
    ItemSplit {
        item: item.into(),
        offset,
        id: "$r".into(),
    }
    .into()
}

/// What sounds at timeline time `t`: (source time, linear amplitude factor of the fades).
fn sound_at(p: &Project, t: Rational) -> (Rational, f64) {
    let plan = eval_audio(p, "tl1", secs(0), secs(40)).unwrap();
    let it = plan
        .items
        .iter()
        .find(|i| i.from <= t && t < i.to)
        .unwrap_or_else(|| panic!("nothing sounds at {t}"));
    let source = it
        .source_from
        .checked_add(
            t.checked_sub(it.from)
                .unwrap()
                .checked_mul(it.speed)
                .unwrap(),
        )
        .unwrap();
    let seg = it.gain.iter().find(|s| s.from <= t && t < s.to).unwrap();
    let x =
        t.checked_sub(seg.from).unwrap().to_f64() / seg.to.checked_sub(seg.from).unwrap().to_f64();
    let factor = seg
        .shapes
        .iter()
        .map(|s| {
            let p = s.p[0] + (s.p[1] - s.p[0]) * x;
            match s.kind {
                ShapeKind::FadeIn | ShapeKind::CrossIn => p,
                ShapeKind::FadeOut | ShapeKind::CrossOut => 1.0 - p,
            }
        })
        .product();
    (source, factor)
}

/// Every quarter second over `[0, len)`.
fn samples(len: i64) -> impl Iterator<Item = Rational> {
    (0..len * 4).map(|q| Rational::new(q, 4).unwrap())
}

/// The envelope (and source mapping) of `before` and `after` agree at every sample.
fn same_envelope(before: &Project, after: &Project, len: i64) {
    for t in samples(len) {
        let (s0, f0) = sound_at(before, t);
        let (s1, f1) = sound_at(after, t);
        assert_eq!(s0, s1, "source time at {t}");
        assert!(
            (f0 - f1).abs() < 1e-12,
            "factor at {t}: {f0} before, {f1} after"
        );
    }
}

fn audio(p: &Project, id: &str) -> Option<AudioParams> {
    p.timelines[0]
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap()
        .common
        .audio
        .clone()
}

fn fades(fade_in: &str, in_offset: &str, fade_out: &str, out_offset: &str) -> AudioParams {
    AudioParams {
        fade_in: r(fade_in),
        fade_in_offset: r(in_offset),
        fade_out: r(fade_out),
        fade_out_offset: r(out_offset),
        ..AudioParams::default()
    }
}

#[test]
fn split_outside_both_fades_keeps_the_envelope() {
    let p = clip_with_fades(10, Some(("4", "4")));
    assert_eq!(sound_at(&p, secs(4)), (secs(4), 1.0));
    assert_eq!(sound_at(&p, secs(6)), (secs(6), 1.0));
    let a = apply(&p, vec![split("c1", secs(5))]);
    assert_eq!(
        sound_at(&a.project, secs(4)),
        (secs(4), 1.0),
        "the repro at 4"
    );
    assert_eq!(
        sound_at(&a.project, secs(6)),
        (secs(6), 1.0),
        "the repro at 6"
    );
    same_envelope(&p, &a.project, 10);
    let right = &a.bindings["$r"];
    assert_eq!(audio(&a.project, "c1"), Some(fades("4", "0", "0", "0")));
    assert_eq!(audio(&a.project, right), Some(fades("0", "0", "4", "0")));
    assert_eq!(undo(&a), p, "apply then undo is the original project");
}

#[test]
fn cut_inside_the_fade_in_keeps_its_progress() {
    let p = clip_with_fades(10, Some(("4", "4")));
    let a = apply(&p, vec![split("c1", secs(2))]);
    same_envelope(&p, &a.project, 10);
    assert_eq!(sound_at(&a.project, secs(1)).1, 0.25);
    assert_eq!(sound_at(&a.project, secs(2)).1, 0.5);
    let right = &a.bindings["$r"];
    assert_eq!(audio(&a.project, "c1"), Some(fades("4", "0", "0", "0")));
    assert_eq!(audio(&a.project, right), Some(fades("4", "2", "4", "0")));
    assert_eq!(undo(&a), p);
}

#[test]
fn cut_inside_the_fade_out_keeps_its_progress() {
    let p = clip_with_fades(10, Some(("4", "4")));
    let a = apply(&p, vec![split("c1", secs(7))]);
    same_envelope(&p, &a.project, 10);
    let right = &a.bindings["$r"];
    assert_eq!(audio(&a.project, "c1"), Some(fades("4", "0", "4", "3")));
    assert_eq!(audio(&a.project, right), Some(fades("0", "0", "4", "0")));
    assert_eq!(undo(&a), p);
}

/// Fades whose sum passes the length overlap and multiply; each is restricted on its own.
#[test]
fn overlapping_fades_are_restricted_independently() {
    let p = clip_with_fades(4, Some(("3", "3")));
    for cut in ["1", "2", "5/2", "3"] {
        let a = apply(&p, vec![split("c1", r(cut))]);
        same_envelope(&p, &a.project, 4);
        assert_eq!(undo(&a), p, "cut {cut}");
    }
}

/// Splitting the halves again (and frame-off cuts) keeps the envelope too.
#[test]
fn repeated_splits_keep_the_envelope() {
    let p = clip_with_fades(10, Some(("4", "5")));
    let a = apply(&p, vec![split("c1", r("3/2"))]);
    let right = a.bindings["$r"].clone();
    let b = apply(&a.project, vec![split(&right, r("11/2"))]);
    let c = apply(&b.project, vec![split("c1", r("1/2"))]);
    same_envelope(&p, &c.project, 10);
    assert_eq!(undo(&c), b.project);
    assert_eq!(undo(&b), a.project);
}

/// The no-fade control: source mapping and amplitude are kept, and no audio params appear.
#[test]
fn control_without_fades() {
    let p = clip_with_fades(10, None);
    let a = apply(&p, vec![split("c1", secs(5))]);
    same_envelope(&p, &a.project, 10);
    assert_eq!(audio(&a.project, "c1"), None);
    assert_eq!(audio(&a.project, &a.bindings["$r"]), None);
    assert_eq!(a.inverse.len(), 2, "remove and trim only");
    assert_eq!(undo(&a), p);
}

/// Offsets read as part of their fade: a window reaching back before the start plays from its
/// progress there; one that does not reach the item has no effect.
#[test]
fn offsets_shift_the_fade_windows() {
    let p = clip_with_fades(10, Some(("4", "0")));
    let a = apply(&p, vec![set("c1", "audio.fadeInOffset", "2")]);
    assert_eq!(sound_at(&a.project, secs(0)).1, 0.5);
    assert_eq!(sound_at(&a.project, secs(1)).1, 0.75);
    assert_eq!(sound_at(&a.project, secs(2)).1, 1.0);
    let b = apply(&a.project, vec![set("c1", "audio.fadeInOffset", "4")]);
    assert_eq!(sound_at(&b.project, secs(0)).1, 1.0);
    let c = apply(
        &p,
        vec![
            set("c1", "audio.fadeOut", "2"),
            set("c1", "audio.fadeOutOffset", "1"),
        ],
    );
    // The fade-out window is [9, 11): it starts at 9 and is a quarter through at the end.
    assert_eq!(sound_at(&c.project, secs(9)).1, 1.0);
    assert_eq!(sound_at(&c.project, r("19/2")).1, 0.75);
    assert_eq!(undo(&c), p);
}

#[test]
fn negative_offsets_are_refused() {
    let p = clip_with_fades(10, Some(("4", "4")));
    for path in ["audio.fadeInOffset", "audio.fadeOutOffset"] {
        let e = try_apply(&p, vec![set("c1", path, "-1")]).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE", "{path}");
    }
}

/// A fade is edge-anchored and trims leave audio fields alone: the offset moves with its edge.
#[test]
fn trims_keep_the_fade_fields() {
    let p = clip_with_fades(10, Some(("4", "4")));
    let a = apply(&p, vec![split("c1", secs(7))]);
    let b = apply(
        &a.project,
        vec![
            ItemTrim {
                item: "c1".into(),
                edge: Edge::End,
                delta: secs(-1),
                mode: TrimMode::Ripple,
                alone: false,
            }
            .into(),
        ],
    );
    assert_eq!(audio(&b.project, "c1"), audio(&a.project, "c1"));
    // The left part now ends at 6 and its fade-out window is [5, 9): 0.75 at 6⁻ (t = 5.5 → 7/8).
    assert_eq!(sound_at(&b.project, r("11/2")).1, 0.875);
}

/// The split writes the left part's fade params it changed, so a concurrent edit of the same
/// fade conflicts; other params merge.
#[test]
fn concurrent_fade_edit_conflicts_with_the_split() {
    let p = clip_with_fades(10, Some(("4", "4")));
    for path in ["audio.fadeOut", "audio.fadeOutOffset"] {
        let e = stale::rebase_over(&p, vec![split("c1", secs(5))], vec![set("c1", path, "1")])
            .unwrap_err();
        assert_eq!(stale::conflict_reason(&e), "SAME_PARAM", "{path}");
    }
    // The left part keeps its fade-in, so the split did not write it.
    let ok = stale::rebase_over(
        &p,
        vec![split("c1", secs(5))],
        vec![set("c1", "audio.fadeIn", "1")],
    );
    assert!(ok.is_ok(), "{ok:?}");
    let gain = ParamSet {
        item: "c1".into(),
        path: "audio.gain".into(),
        value: Some(ParamValue::Number(-6.0)),
    };
    let ok = stale::rebase_over(&p, vec![split("c1", secs(5))], vec![gain.into()]);
    assert!(ok.is_ok(), "{ok:?}");
    // Without fades the split writes no audio param.
    let plain = clip_with_fades(10, None);
    let ok = stale::rebase_over(
        &plain,
        vec![split("c1", secs(5))],
        vec![set("c1", "audio.fadeOut", "1")],
    );
    assert!(ok.is_ok(), "{ok:?}");
}

/// Review of #55 (S3): a cut inside the fade-in leaves the left part's fade-in as it was, but
/// the split divided that fade between the parts, so a concurrent edit of it conflicts (the
/// mirror of the fade-out case) instead of leaving a gain step at the cut. The inverse still
/// restores only what changed on the left part.
#[test]
fn concurrent_edit_of_a_divided_fade_in_conflicts() {
    let p = clip_with_fades(10, Some(("4", "4")));
    for path in ["audio.fadeIn", "audio.fadeInOffset"] {
        let e = stale::rebase_over(&p, vec![split("c1", secs(2))], vec![set("c1", path, "1")])
            .unwrap_err();
        assert_eq!(stale::conflict_reason(&e), "SAME_PARAM", "{path}");
    }
    // The left part's fade-out was cleared: that conflicts too.
    let e = stale::rebase_over(
        &p,
        vec![split("c1", secs(2))],
        vec![set("c1", "audio.fadeOut", "1")],
    )
    .unwrap_err();
    assert_eq!(stale::conflict_reason(&e), "SAME_PARAM");
    // An unrelated param still merges.
    let gain = ParamSet {
        item: "c1".into(),
        path: "audio.gain".into(),
        value: Some(ParamValue::Number(-6.0)),
    };
    let ok = stale::rebase_over(&p, vec![split("c1", secs(2))], vec![gain.into()]);
    assert!(ok.is_ok(), "{ok:?}");
    // Only the left part's cleared fade-out is put back: remove, trim and its two paths.
    let a = apply(&p, vec![split("c1", secs(2))]);
    assert_eq!(a.inverse.len(), 4, "{:?}", a.inverse);
    assert_eq!(undo(&a), p);
    // The same edit the other way round: a stale split over the fade-in edit conflicts.
    let e = stale::rebase_over(
        &p,
        vec![set("c1", "audio.fadeIn", "1")],
        vec![split("c1", secs(2))],
    )
    .unwrap_err();
    assert_eq!(stale::conflict_reason(&e), "SAME_PARAM");
}

/// Everything that sounds at `t`: (source time, factor) per item, sorted.
fn sounds_at(p: &Project, t: Rational) -> Vec<(Rational, f64)> {
    let plan = eval_audio(p, "tl1", secs(0), secs(40)).unwrap();
    let mut out: Vec<(Rational, f64)> = plan
        .items
        .iter()
        .filter(|i| i.from <= t && t < i.to)
        .map(|it| {
            let source = it
                .source_from
                .checked_add(
                    t.checked_sub(it.from)
                        .unwrap()
                        .checked_mul(it.speed)
                        .unwrap(),
                )
                .unwrap();
            let seg = it.gain.iter().find(|s| s.from <= t && t < s.to).unwrap();
            let x = t.checked_sub(seg.from).unwrap().to_f64()
                / seg.to.checked_sub(seg.from).unwrap().to_f64();
            let factor = seg
                .shapes
                .iter()
                .map(|s| {
                    let p = s.p[0] + (s.p[1] - s.p[0]) * x;
                    match s.kind {
                        ShapeKind::FadeIn | ShapeKind::CrossIn => p,
                        ShapeKind::FadeOut | ShapeKind::CrossOut => 1.0 - p,
                    }
                })
                .product();
            (source, factor)
        })
        .collect();
    out.sort_by_key(|a| a.0);
    out
}

/// `x1` [0, 10) from source 0 and `b1` [10, 20) from source 20 on `t2`, a 2 s crossfade
/// between them, `fades` on each (`(item, path, value)`).
fn crossfaded(fades: &[(&str, &str, &str)]) -> Project {
    let mut ops = vec![
        audio_asset("$b", secs(40)),
        insert("$x", "t2", secs(0), "$b", secs(0), secs(10)),
        insert("$y", "t2", secs(10), "$b", secs(20), secs(10)),
        TransitionSet {
            id: "$tr".into(),
            track: "t2".into(),
            between: ["$x".into(), "$y".into()],
            kind: TransitionKind::Crossfade,
            duration: secs(2),
            curve: Curve::Linear,
        }
        .into(),
    ];
    for (item, path, v) in fades {
        ops.push(set(item, path, v));
    }
    let a = apply(&project(), ops);
    assert_eq!(
        (a.bindings["$x"].as_str(), a.bindings["$y"].as_str()),
        ("c1", "c2")
    );
    a.project
}

/// Review of #55 (S2): an incoming crossfade suppresses the item's fade-in, so the right part
/// of a split does not inherit an audible share of it.
#[test]
fn split_after_an_incoming_crossfade_keeps_the_fade_in_silent() {
    let p = crossfaded(&[("$y", "audio.fadeIn", "4")]);
    let a = apply(&p, vec![split("c2", secs(2))]);
    for q in 0..80 {
        let t = Rational::new(q, 4).unwrap();
        let (before, after) = (sounds_at(&p, t), sounds_at(&a.project, t));
        assert_eq!(before.len(), after.len(), "at {t}");
        for ((s0, f0), (s1, f1)) in before.iter().zip(&after) {
            assert_eq!(s0, s1, "source at {t}");
            assert!(
                (f0 - f1).abs() < 1e-12,
                "factor at {t}: {f0} before, {f1} after"
            );
        }
    }
    assert_eq!(audio(&a.project, "c2"), Some(fades("4", "0", "0", "0")));
    assert_eq!(audio(&a.project, &a.bindings["$r"]), None);
    assert_eq!(undo(&a), p);
}

/// The mirror: an outgoing crossfade suppresses the fade-out, so the left part drops its
/// share. The prune pass removes the transition, whose outgoing item no longer reaches the
/// cut (behaviour older than #55): the right part's fade-out then sounds, as it did before.
#[test]
fn split_before_an_outgoing_crossfade_drops_the_left_fade_out() {
    let p = crossfaded(&[("$x", "audio.fadeOut", "4")]);
    let a = apply(&p, vec![split("c1", secs(5))]);
    assert_eq!(audio(&a.project, "c1"), None);
    assert_eq!(
        audio(&a.project, &a.bindings["$r"]),
        Some(fades("0", "0", "4", "0"))
    );
    assert!(
        a.project.timelines[0].tracks[1].transitions.is_empty(),
        "pruned"
    );
    for q in 0..20 {
        let t = Rational::new(q, 4).unwrap();
        assert_eq!(
            sounds_at(&p, t),
            sounds_at(&a.project, t),
            "the left part at {t}"
        );
    }
    assert_eq!(undo(&a), p);
}
