//! `eval_audio` (plan 4C Task 16) over projects built literally from model types.

mod eval_common;

use eval_common::*;
use fin_core::eval::{AudioItem, AudioPlan, GainSegment, PanSegment, Shape, ShapeKind, eval_audio};
use fin_core::{Curve, Interp, Item, ItemBody, Project, Rational, TrackKind, TransitionKind};
use proptest::prelude::*;

fn plan(p: &Project, from: &str, to: &str) -> AudioPlan {
    eval_audio(p, "tl1", r(from), r(to)).unwrap()
}

fn only(p: &Project, from: &str, to: &str) -> AudioItem {
    let mut items = plan(p, from, to).items;
    assert_eq!(items.len(), 1, "{items:?}");
    items.remove(0)
}

fn edges(segs: &[GainSegment]) -> Vec<Rational> {
    let mut v: Vec<Rational> = segs.iter().map(|s| s.from).collect();
    v.extend(segs.last().map(|s| s.to));
    v
}

fn shape(kind: ShapeKind, curve: Curve, p: [f64; 2]) -> Shape {
    Shape { kind, curve, p }
}

fn constant(from: Rational, to: Rational, db: f64, shapes: Vec<Shape>) -> GainSegment {
    GainSegment {
        from,
        to,
        db: [db, db],
        interp: Interp::Hold,
        u: [0.0, 1.0],
        shapes,
    }
}

fn audio_project(tracks: Vec<fin_core::Track>) -> Project {
    project_with(vec![timeline("tl1", tracks)])
}

#[test]
fn segments_break_at_every_edge() {
    let c1 = with_audio(
        clip("c1", secs(0), secs(0), secs(10)),
        0.0,
        0.0,
        secs(1),
        secs(2),
    );
    let c1 = keyed(
        c1,
        "audio.gain",
        vec![
            kf(secs(2), num(-6.0), Interp::Linear),
            kf(secs(6), num(0.0), Interp::Hold),
        ],
    );
    let c1 = keyed(
        c1,
        "audio.pan",
        vec![kf(secs(5), num(-1.0), Interp::Linear)],
    );
    let mut a1 = track(
        "t2",
        TrackKind::Audio,
        vec![c1, clip("c2", secs(10), secs(0), secs(4))],
    );
    a1.transitions
        .push(crossfade("tr1", "c1", "c2", secs(1), Curve::Linear));
    let p = audio_project(vec![a1]);
    let items = plan(&p, "0", "20").items;
    let c1 = &items[0];
    // Extent [0, 21/2): fade-in end 1, keyframes 2 and 6, fade-out start 17/2, window 19/2.
    let want: Vec<Rational> = ["0", "1", "2", "6", "17/2", "19/2", "21/2"].map(r).into();
    assert_eq!(edges(&c1.gain), want);
    assert_eq!((c1.from, c1.to), (secs(0), r("21/2")));
    // Before the first keyframe the first value holds; the ramp runs 2 → 6; then 0 dB.
    assert_eq!(c1.gain[1], constant(secs(1), secs(2), -6.0, vec![]));
    assert_eq!(
        c1.gain[2],
        GainSegment {
            from: secs(2),
            to: secs(6),
            db: [-6.0, 0.0],
            interp: Interp::Linear,
            u: [0.0, 1.0],
            shapes: vec![]
        }
    );
    assert_eq!(c1.gain[3], constant(secs(6), r("17/2"), 0.0, vec![]));
    let pans: Vec<(Rational, Rational, [f64; 2])> =
        c1.pan.iter().map(|s| (s.from, s.to, s.pan)).collect();
    assert_eq!(
        pans,
        [
            (secs(0), secs(5), [-1.0, -1.0]),
            (secs(5), r("21/2"), [-1.0, -1.0])
        ]
    );
    // c2 starts early at 19/2 and breaks where the window ends.
    assert_eq!(edges(&items[1].gain), ["19/2", "21/2", "14"].map(r));
}

#[test]
fn gain_sums_track_and_item_db() {
    let c1 = with_audio(
        clip("c1", secs(0), secs(0), secs(4)),
        -6.0,
        0.25,
        secs(0),
        secs(0),
    );
    let c2 = keyed(
        clip("c2", secs(4), secs(0), secs(4)),
        "audio.gain",
        vec![
            kf(secs(0), num(-12.0), Interp::Linear),
            kf(secs(4), num(0.0), Interp::Linear),
        ],
    );
    let mut a1 = track("t2", TrackKind::Audio, vec![c1, c2]);
    a1.gain = -3.0;
    let p = audio_project(vec![a1]);
    let items = plan(&p, "0", "8").items;
    assert_eq!(items[0].gain, [constant(secs(0), secs(4), -9.0, vec![])]);
    assert_eq!(
        items[0].pan,
        [PanSegment {
            from: secs(0),
            to: secs(4),
            pan: [0.25, 0.25],
            interp: Interp::Hold,
            u: [0.0, 1.0]
        }]
    );
    // No audio params: 0 dB and centre pan, plus the track's gain.
    assert_eq!(items[1].gain[0].db, [-15.0, -3.0]);
    assert_eq!(
        (items[1].gain[0].interp, items[1].gain[0].u),
        (Interp::Linear, [0.0, 1.0])
    );
    assert_eq!(items[1].pan[0].pan, [0.0, 0.0]);
}

#[test]
fn fades_are_shapes() {
    let c1 = with_audio(
        clip("c1", secs(0), secs(0), secs(4)),
        0.0,
        0.0,
        secs(2),
        secs(1),
    );
    let p = audio_project(vec![track("t2", TrackKind::Audio, vec![c1])]);
    let c = only(&p, "0", "4");
    assert_eq!(
        c.gain,
        [
            constant(
                secs(0),
                secs(2),
                0.0,
                vec![shape(ShapeKind::FadeIn, Curve::Linear, [0.0, 1.0])]
            ),
            constant(secs(2), secs(3), 0.0, vec![]),
            constant(
                secs(3),
                secs(4),
                0.0,
                vec![shape(ShapeKind::FadeOut, Curve::Linear, [0.0, 1.0])]
            ),
        ]
    );
    // A range starting inside the fade gives its progress there.
    let c = only(&p, "1", "4");
    assert_eq!(
        c.gain[0].shapes,
        [shape(ShapeKind::FadeIn, Curve::Linear, [0.5, 1.0])]
    );
    // Fades longer than the item overlap and multiply.
    let long = with_audio(
        clip("c1", secs(0), secs(0), secs(2)),
        0.0,
        0.0,
        secs(4),
        secs(4),
    );
    let c = only(
        &audio_project(vec![track("t2", TrackKind::Audio, vec![long])]),
        "0",
        "2",
    );
    assert_eq!(
        c.gain[0].shapes,
        [
            shape(ShapeKind::FadeIn, Curve::Linear, [0.0, 0.5]),
            shape(ShapeKind::FadeOut, Curve::Linear, [0.5, 1.0])
        ]
    );
}

#[test]
fn crossfade_shapes_with_curve() {
    let mut a1 = track(
        "t2",
        TrackKind::Audio,
        vec![
            clip("c1", secs(0), secs(0), secs(4)),
            clip("c2", secs(4), secs(10), secs(4)),
        ],
    );
    let mut tr = crossfade("tr1", "c1", "c2", secs(2), Curve::EqualPower);
    tr.kind = TransitionKind::Dissolve;
    a1.transitions.push(tr);
    let p = audio_project(vec![a1]);
    let items = plan(&p, "0", "8").items;
    let (a, b) = (&items[0], &items[1]);
    assert_eq!((a.from, a.to, a.source_from), (secs(0), secs(5), secs(0)));
    assert_eq!(edges(&a.gain), ["0", "3", "5"].map(r));
    assert_eq!(
        a.gain[1].shapes,
        [shape(ShapeKind::CrossOut, Curve::EqualPower, [0.0, 1.0])]
    );
    // B starts one second early, reading its handle.
    assert_eq!((b.from, b.to, b.source_from), (secs(3), secs(8), secs(9)));
    assert_eq!(
        b.gain[0].shapes,
        [shape(ShapeKind::CrossIn, Curve::EqualPower, [0.0, 1.0])]
    );
    assert!(b.gain[1].shapes.is_empty());
    // Inside the window both sound, each with its side's progress.
    let mid = plan(&p, "4", "9/2").items;
    assert_eq!(mid.len(), 2);
    assert_eq!(mid[0].gain[0].shapes[0].p, [0.5, 0.75]);
    assert_eq!(
        (mid[1].gain[0].shapes[0].kind, mid[1].gain[0].shapes[0].p),
        (ShapeKind::CrossIn, [0.5, 0.75])
    );
}

#[test]
fn mute_and_solo() {
    let a = |id: &str, c: &str| {
        track(
            id,
            TrackKind::Audio,
            vec![clip(c, secs(0), secs(0), secs(4))],
        )
    };
    let (mut t2, mut t3, t4) = (a("t2", "c2"), a("t3", "c3"), a("t4", "c4"));
    t2.mute = true;
    let ids = |p: &Project| {
        plan(p, "0", "4")
            .items
            .into_iter()
            .map(|i| i.item)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        ids(&audio_project(vec![t2.clone(), t3.clone(), t4.clone()])),
        ["c3", "c4"]
    );
    t3.solo = true;
    assert_eq!(
        ids(&audio_project(vec![t2.clone(), t3.clone(), t4.clone()])),
        ["c3"]
    );
    // A muted soloed track is silent, and still silences the others.
    t3.mute = true;
    assert!(ids(&audio_project(vec![t2.clone(), t3.clone(), t4.clone()])).is_empty());
    // A soloed video track does not affect audio.
    let mut v = track(
        "t1",
        TrackKind::Video,
        vec![clip("c1", secs(0), secs(0), secs(4))],
    );
    v.solo = true;
    assert_eq!(ids(&audio_project(vec![v, t4])), ["c4"]);
}

#[test]
fn range_clipping() {
    let [x1, y1, x2, y2] = fin_core::eval::PRESETS[0].1;
    let ease = Interp::Bezier { x1, y1, x2, y2 };
    let c = with_speed(clip("c1", secs(2), secs(10), secs(8)), secs(2));
    let c = keyed(
        c,
        "audio.gain",
        vec![
            kf(secs(0), num(-20.0), ease),
            kf(secs(4), num(0.0), Interp::Linear),
        ],
    );
    let p = audio_project(vec![track("t2", TrackKind::Audio, vec![c])]);
    let c = only(&p, "3", "5");
    assert_eq!(
        (c.from, c.to, c.source_from, c.speed),
        (secs(3), secs(5), secs(12), secs(2))
    );
    // The bézier span [2, 6) is cut: its end values stay, `u` says which part plays.
    assert_eq!(
        c.gain,
        [GainSegment {
            from: secs(3),
            to: secs(5),
            db: [-20.0, 0.0],
            interp: ease,
            u: [0.25, 0.75],
            shapes: vec![]
        }]
    );
    // Outside every item, or at its exclusive end, nothing plays.
    assert!(plan(&p, "0", "2").items.is_empty());
    assert!(plan(&p, "10", "11").items.is_empty());
    assert_eq!(only(&p, "9", "100").to, secs(10));
    for (from, to) in [("1", "1"), ("2", "1")] {
        let e = eval_audio(&p, "tl1", r(from), r(to)).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
    }
    assert_eq!(
        eval_audio(&p, "tl9", secs(0), secs(1)).unwrap_err().code,
        "NOT_FOUND"
    );
}

#[test]
fn video_tracks_are_silent() {
    let v = track(
        "t1",
        TrackKind::Video,
        vec![clip("c1", secs(0), secs(0), secs(4))],
    );
    // Only clips sound: other kinds on an audio track are silent in 4C.
    let a = track(
        "t2",
        TrackKind::Audio,
        vec![Item::new(
            "img1",
            secs(0),
            secs(4),
            ItemBody::Image { asset: "a2".into() },
        )],
    );
    let c = track(
        "t4",
        TrackKind::Caption,
        vec![caption("cap1", secs(0), secs(4), &[("Hi", "0", "1")])],
    );
    assert!(
        plan(&audio_project(vec![v, a, c]), "0", "4")
            .items
            .is_empty()
    );
}

#[test]
fn plan_json_omits_empty_shapes() {
    let c1 = with_audio(
        clip("c1", secs(0), secs(0), secs(2)),
        0.0,
        0.0,
        secs(1),
        secs(0),
    );
    let p = audio_project(vec![track("t2", TrackKind::Audio, vec![c1])]);
    let json = serde_json::to_value(plan(&p, "0", "2")).unwrap();
    let want = serde_json::json!({
        "timeline": "tl1", "from": "0", "to": "2",
        "items": [{
            "item": "c1", "track": "t2", "asset": "a1", "from": "0", "to": "2", "sourceFrom": "0", "speed": "1",
            "gain": [
                {"from": "0", "to": "1", "db": [0.0, 0.0], "interp": {"type": "hold"}, "u": [0.0, 1.0],
                 "shapes": [{"kind": "fadeIn", "curve": "linear", "p": [0.0, 1.0]}]},
                {"from": "1", "to": "2", "db": [0.0, 0.0], "interp": {"type": "hold"}, "u": [0.0, 1.0]}
            ],
            "pan": [{"from": "0", "to": "2", "pan": [0.0, 0.0], "interp": {"type": "hold"}, "u": [0.0, 1.0]}]
        }]
    });
    assert_eq!(json, want);
}

prop_compose! {
    fn interp()(k in 0..3u8, x1 in 0.0..=1.0f64, y1 in -2.0..3.0f64, x2 in 0.0..=1.0f64, y2 in -2.0..3.0f64) -> Interp {
        match k { 0 => Interp::Hold, 1 => Interp::Linear, _ => Interp::Bezier { x1, y1, x2, y2 } }
    }
}

prop_compose! {
    fn keys()(ats in proptest::collection::vec(-20..200i64, 0..5), vals in proptest::collection::vec(-60.0..12.0f64, 5), ints in proptest::collection::vec(interp(), 5)) -> Vec<fin_core::Keyframe> {
        let mut ats = ats;
        ats.sort();
        ats.iter().enumerate().map(|(i, a)| kf(Rational::new(*a, 30).unwrap(), num(vals[i]), ints[i])).collect()
    }
}

prop_compose! {
    fn arb_project()(lens in proptest::collection::vec(1..90i64, 1..5), gaps in proptest::collection::vec(0..2i64, 4), fades in proptest::collection::vec((0..60i64, 0..60i64), 4),
                     gain in keys(), pan in keys(), tr in proptest::collection::vec(0..60i64, 4), track_gain in -30.0..6.0f64) -> Project {
        let mut start = 0i64;
        let mut items = Vec::new();
        for (i, len) in lens.iter().enumerate() {
            start += gaps[i] * 15;
            let c = clip(&format!("c{}", i + 1), Rational::new(start, 30).unwrap(), secs(1), Rational::new(*len, 30).unwrap());
            let c = with_audio(c, -3.0, 0.0, Rational::new(fades[i].0, 30).unwrap(), Rational::new(fades[i].1, 30).unwrap());
            let c = keyed(keyed(c, "audio.gain", gain.clone()), "audio.pan", pan.clone());
            items.push(c);
            start += len;
        }
        let mut a1 = track("t2", TrackKind::Audio, items.clone());
        a1.gain = track_gain;
        for (i, pair) in items.windows(2).enumerate() {
            if pair[0].end().unwrap() == pair[1].start && tr[i] > 0 {
                a1.transitions.push(crossfade(&format!("tr{i}"), &pair[0].id, &pair[1].id, Rational::new(tr[i], 30).unwrap(), Curve::EqualPower));
            }
        }
        audio_project(vec![a1])
    }
}

proptest! {
    #![proptest_config(prop_config(64))]
    #[test]
    fn no_nan(p in arb_project(), from in -30..300i64, len in 1..300i64) {
        let plan = eval_audio(&p, "tl1", Rational::new(from, 30).unwrap(), Rational::new(from + len, 30).unwrap()).unwrap();
        for item in &plan.items {
            prop_assert!(item.from < item.to);
            // Segments tile the item's span in order.
            let mut at = item.from;
            for s in &item.gain {
                prop_assert_eq!(s.from, at);
                prop_assert!(s.from < s.to);
                at = s.to;
                prop_assert!(s.db.iter().chain(&s.u).all(|x| x.is_finite()));
                prop_assert!(s.u[0] <= s.u[1] && (0.0..=1.0).contains(&s.u[0]) && (0.0..=1.0).contains(&s.u[1]));
                for sh in &s.shapes {
                    prop_assert!(sh.p[0] <= sh.p[1] && (0.0..=1.0).contains(&sh.p[0]) && (0.0..=1.0).contains(&sh.p[1]));
                }
            }
            prop_assert_eq!(at, item.to);
            let mut at = item.from;
            for s in &item.pan {
                prop_assert_eq!(s.from, at);
                at = s.to;
                prop_assert!(s.pan.iter().chain(&s.u).all(|x| x.is_finite()));
            }
            prop_assert_eq!(at, item.to);
        }
    }
}
