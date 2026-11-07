//! Builders for the move, slide and link tests (plan 4C Task 9).
#![allow(dead_code)]

use fin_core::{
    InsertMode, ItemLink, ItemMove, ItemSlide, ItemUnlink, Op, Project, Rational, Track, TrackKind,
};

use super::{apply, audio_asset, av_asset, insert, project, secs};

pub fn move_to(item: &str, track: &str, at: Rational, mode: InsertMode) -> Op {
    ItemMove {
        item: item.into(),
        track: track.into(),
        at,
        mode,
    }
    .into()
}

pub fn slide(item: &str, delta: Rational) -> Op {
    ItemSlide {
        item: item.into(),
        delta,
        sides: None,
    }
    .into()
}

pub fn link(items: &[&str], id: &str) -> Op {
    ItemLink {
        items: items.iter().map(|s| s.to_string()).collect(),
        id: id.into(),
    }
    .into()
}

pub fn unlink(group: &str) -> Op {
    ItemUnlink {
        group: group.into(),
    }
    .into()
}

/// `(id, start, duration)` of every item on track `track` (index in timeline 0).
pub fn spans_on(p: &Project, track: usize) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[track]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

pub fn sp(id: &str, start: i64, duration: i64) -> (String, Rational, Rational) {
    (id.to_string(), secs(start), secs(duration))
}

/// `p` with a second video track `t3` (`V2`) on top.
pub fn with_v2(mut p: Project) -> Project {
    p.timelines[0]
        .tracks
        .push(Track::new("t3", TrackKind::Video, "V2"));
    p
}

/// Asset `a1` (100 s, A/V) and `a2` (audio, `audio_len` s). On `t1`: `c1` [0,4) linked (lk1)
/// with `c2` [0,4) on `t2` (from `a2`), then `c3` [4,8) on `t1` and `c4` [4,8) on `t2`, and
/// `t3` (video) empty.
pub fn linked(audio_len: i64) -> Project {
    let ops = vec![
        av_asset("$a", secs(100)),
        audio_asset("$b", secs(audio_len)),
        insert("$v", "t1", secs(0), "$a", secs(0), secs(4)),
        insert("$s", "t2", secs(0), "$b", secs(0), secs(4)),
        insert("$x", "t1", secs(4), "$a", secs(0), secs(4)),
        insert("$y", "t2", secs(4), "$a", secs(0), secs(4)),
        link(&["$v", "$s"], "$lk"),
    ];
    with_v2(apply(&project(), ops).project)
}
