//! The multi-op builders `fin` uses (plan 4C Decisions 17, 21 and 22).

mod common;

use common::moves::*;
use common::*;
use fin_core::{InsertMode, ItemBody, Op, TrackKind, compose};
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
