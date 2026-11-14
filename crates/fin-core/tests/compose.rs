//! The multi-op builders `fin` uses (plan 4C Decisions 17, 21 and 22).

mod common;

use common::moves::*;
use common::*;
use fin_core::{
    Curve, InsertMode, ItemBody, Op, Project, TrackKind, TransitionKind, TransitionSet, compose,
};
use serde_json::json;

#[test]
fn clip_with_audio_links_video_and_audio() {
    let p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let c = compose::clip_with_audio(
        &p,
        "t1",
        secs(0),
        secs(5),
        "a1",
        secs(2),
        secs(1),
        InsertMode::Ripple,
        None,
    )
    .unwrap();
    let names: Vec<&str> = c.ops.iter().map(Op::name).collect();
    assert_eq!(names, ["item.insert", "item.insert", "item.link"]);
    assert_eq!(c.ops[2], link(&["$clip", "$audio"], "$link"));
    assert_eq!(c.echo["audio"], json!({ "track": "t2", "id": "$audio" }));
    let a = apply(&p, c.ops);
    let (v, s) = (clip(&a.project, "c1"), clip(&a.project, "c2"));
    assert_eq!(
        (v.start, v.duration, v.source_in),
        (secs(0), secs(5), secs(2))
    );
    assert_eq!(
        (s.start, s.duration, s.source_in),
        (secs(0), secs(5), secs(2))
    );
    let links: Vec<_> = a.project.timelines[0]
        .tracks
        .iter()
        .map(|t| t.items[0].common.link.clone())
        .collect();
    assert_eq!(links, [Some("lk1".to_string()), Some("lk1".to_string())]);
    // A video-only asset, or a clip on an audio track, is one insert.
    let q = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let c = compose::clip_with_audio(
        &q,
        "t2",
        secs(0),
        secs(5),
        "a1",
        secs(0),
        secs(1),
        InsertMode::Ripple,
        None,
    )
    .unwrap();
    assert_eq!(c.ops.len(), 1);
    assert_eq!(c.echo["audio"], json!(null));
}

#[test]
fn clip_with_audio_needs_an_audio_track() {
    let mut p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    p.timelines[0].tracks.retain(|t| t.kind == TrackKind::Video);
    let e = compose::clip_with_audio(
        &p,
        "t1",
        secs(0),
        secs(5),
        "a1",
        secs(0),
        secs(1),
        InsertMode::Ripple,
        None,
    )
    .unwrap_err();
    assert_eq!(e.code, "USAGE");
    assert!(e.fix.unwrap().contains("--no-audio"));
    // A named track must be an audio track.
    let p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let e = compose::clip_with_audio(
        &p,
        "t1",
        secs(0),
        secs(5),
        "a1",
        secs(0),
        secs(1),
        InsertMode::Ripple,
        Some("t1"),
    )
    .unwrap_err();
    assert_eq!(e.code, "TRACK_KIND_MISMATCH");
}

/// A named audio track that would place no audio is a usage error, not silently ignored: the
/// clip goes on an audio track, or the asset has no audio.
#[test]
fn clip_with_audio_refuses_an_audio_track_it_would_not_use() {
    let p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    let mut video_only = p.clone();
    video_only.assets.get_mut("a1").unwrap().audio = None;
    for (p, track, why) in [(&p, "t2", "audio track"), (&video_only, "t1", "no audio")] {
        let e = compose::clip_with_audio(
            p,
            track,
            secs(0),
            secs(5),
            "a1",
            secs(0),
            secs(1),
            InsertMode::Ripple,
            Some("t2"),
        )
        .unwrap_err();
        assert_eq!(e.code, "USAGE", "{track}");
        assert!(e.message.contains(why), "{}", e.message);
        assert_eq!(e.details["audioTrack"], "t2");
        assert_eq!(e.fix.as_deref(), Some("drop --audio-track"));
    }
}

/// 4C Tasks 9–10 review: the audio landing inside an item says how to avoid it, and a named
/// audio track must be on the clip's timeline.
#[test]
fn clip_with_audio_explains_audio_inside_an_item() {
    // t1 is empty where the clip goes; t2 has an audio clip [0,4).
    let p = apply(
        &project(),
        vec![
            av_asset("$a", secs(100)),
            insert("$s", "t2", secs(0), "$a", secs(0), secs(4)),
        ],
    )
    .project;
    let audio = |p: &Project, track: Option<&str>| {
        compose::clip_with_audio(
            p,
            "t1",
            secs(2),
            secs(5),
            "a1",
            secs(0),
            secs(1),
            InsertMode::Ripple,
            track,
        )
    };
    let e = audio(&p, None).unwrap_err();
    assert_eq!(e.code, "INSERT_INSIDE_CLIP");
    assert_eq!(e.details["track"], "t2");
    assert_eq!(e.details["item"], "c1");
    let fix = e.fix.unwrap();
    assert!(
        fix.contains("--no-audio") && fix.contains("--audio-track"),
        "{fix}"
    );
    // Another timeline's audio track is refused.
    let mut q = p.clone();
    let mut tl2 = q.timelines[0].clone();
    tl2.id = "tl2".into();
    tl2.tracks = vec![fin_core::Track::new("t9", TrackKind::Audio, "A1")];
    q.timelines.push(tl2);
    let e = audio(&q, Some("t9")).unwrap_err();
    assert_eq!(e.code, "USAGE");
    assert!(e.fix.unwrap().contains("own timeline"));
}

#[test]
fn clip_with_audio_overwrites_inside_items_on_both_tracks() {
    let p = linked(10);
    let c = compose::clip_with_audio(
        &p,
        "t1",
        secs(1),
        secs(2),
        "a1",
        secs(0),
        secs(1),
        InsertMode::Overwrite,
        None,
    )
    .unwrap();
    let names: Vec<&str> = c.ops.iter().map(Op::name).collect();
    assert_eq!(
        names,
        [
            "item.split",
            "item.insert",
            "item.split",
            "item.insert",
            "item.link"
        ]
    );
    let a = apply(&p, c.ops);
    assert_eq!(
        spans_on(&a.project, 0),
        [
            sp("c1", 0, 1),
            sp("c6", 1, 2),
            sp("c5", 3, 1),
            sp("c3", 4, 4)
        ]
    );
    assert_eq!(
        spans_on(&a.project, 1),
        [
            sp("c2", 0, 1),
            sp("c8", 1, 2),
            sp("c7", 3, 1),
            sp("c4", 4, 4)
        ]
    );
}

#[test]
fn split_linked_splits_members_and_links_rights() {
    let p = linked(10);
    let c = compose::split_linked(&p, "c1", secs(1)).unwrap();
    let names: Vec<&str> = c.ops.iter().map(Op::name).collect();
    assert_eq!(names, ["item.split", "item.split", "item.link"]);
    let a = apply(&p, c.ops);
    let link_of = |id: &str| {
        a.project.timelines[0]
            .tracks
            .iter()
            .flat_map(|t| &t.items)
            .find(|c| c.id == id)
            .unwrap()
            .common
            .link
            .clone()
    };
    assert_eq!(
        spans_on(&a.project, 0)[..2],
        [sp("c1", 0, 1), sp("c5", 1, 3)]
    );
    assert_eq!(
        spans_on(&a.project, 1)[..2],
        [sp("c2", 0, 1), sp("c6", 1, 3)]
    );
    assert_eq!(link_of("c1"), Some("lk1".into()));
    assert_eq!(link_of("c2"), Some("lk1".into()));
    assert_eq!(link_of("c5"), Some("lk2".into()));
    assert_eq!(link_of("c6"), Some("lk2".into()));
    // An unlinked item splits alone and links nothing.
    let c = compose::split_linked(&p, "c3", secs(5)).unwrap();
    assert_eq!(c.ops.len(), 1);
    assert_eq!(c.echo["link"], json!(null));
    let _ = ItemBody::Adjustment {};
}

/// Two A/V clips back to back, each with its linked audio: `c1`|`c3` on `t1` (video, from 0
/// and 10), `c2`|`c4` on `t2` (audio), so every cut has handles.
fn linked_pair() -> Project {
    let mut p = apply(&project(), vec![av_asset("$a", secs(100))]).project;
    for (at, from) in [(0, 0), (4, 10)] {
        let c = compose::clip_with_audio(
            &p,
            "t1",
            secs(at),
            secs(4),
            "a1",
            secs(from),
            secs(1),
            InsertMode::Ripple,
            None,
        )
        .unwrap();
        p = apply(&p, c.ops).project;
    }
    p
}

fn transitions_on(p: &Project, track: usize) -> Vec<(String, [String; 2], TransitionKind)> {
    p.timelines[0].tracks[track]
        .transitions
        .iter()
        .map(|t| (t.id.clone(), t.between.clone(), t.kind))
        .collect()
}

#[test]
fn transition_with_links_crossfades_audio_partners() {
    let p = linked_pair();
    let c = compose::transition_with_links(
        &p,
        "c1",
        "c3",
        TransitionKind::Dissolve,
        secs(1),
        Curve::Linear,
        true,
    )
    .unwrap();
    let want: Vec<Op> = [
        ("$transition", "t1", "c1", "c3", TransitionKind::Dissolve),
        ("$transition2", "t2", "c2", "c4", TransitionKind::Crossfade),
    ]
    .into_iter()
    .map(|(id, track, a, b, kind)| {
        TransitionSet {
            id: id.into(),
            track: track.into(),
            between: [a.into(), b.into()],
            kind,
            duration: secs(1),
            curve: Curve::Linear,
        }
        .into()
    })
    .collect();
    assert_eq!(c.ops, want);
    assert_eq!(
        c.echo,
        json!({
            "transitions": [
                { "id": "$transition", "track": "t1", "between": ["c1", "c3"] },
                { "id": "$transition2", "track": "t2", "between": ["c2", "c4"] },
            ],
            "skipped": [],
        })
    );
    let a = apply(&p, c.ops);
    let pair = |id: &str, x: &str, y: &str, kind| (id.to_string(), [x.into(), y.into()], kind);
    assert_eq!(
        transitions_on(&a.project, 0),
        [pair("tr1", "c1", "c3", TransitionKind::Dissolve)]
    );
    assert_eq!(
        transitions_on(&a.project, 1),
        [pair("tr2", "c2", "c4", TransitionKind::Crossfade)]
    );
    // `--no-linked`: the one transition.
    let c = compose::transition_with_links(
        &p,
        "c1",
        "c3",
        TransitionKind::Dissolve,
        secs(1),
        Curve::Linear,
        false,
    )
    .unwrap();
    assert_eq!(c.ops.len(), 1);
    // Unknown items are NOT_FOUND; a cut that is not one is the op's own NOT_ADJACENT.
    let e = compose::transition_with_links(
        &p,
        "c9",
        "c3",
        TransitionKind::Dissolve,
        secs(1),
        Curve::Linear,
        true,
    )
    .unwrap_err();
    assert_eq!(e.code, "NOT_FOUND");
}

#[test]
fn transition_with_links_skips_partners_that_do_not_touch() {
    let mut p = linked_pair();
    // The first audio partner ends a second early: no cut on t2.
    p.timelines[0].tracks[1].items[0].duration = secs(3);
    let c = compose::transition_with_links(
        &p,
        "c1",
        "c3",
        TransitionKind::Dissolve,
        secs(1),
        Curve::Linear,
        true,
    )
    .unwrap();
    assert_eq!(c.ops.len(), 1);
    assert_eq!(
        c.echo["skipped"],
        json!([{ "track": "t2", "between": ["c2", "c4"] }])
    );
    let a = apply(&p, c.ops);
    assert_eq!(transitions_on(&a.project, 0).len(), 1);
    assert!(transitions_on(&a.project, 1).is_empty());
}
