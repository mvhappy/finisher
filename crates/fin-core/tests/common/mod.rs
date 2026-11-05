//! Shared fixtures for the op tests.
#![allow(dead_code)]

use fin_core::{
    Applied, AudioInfo, CoreError, MemIds, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo,
    apply_internal, apply_tx, new_project,
};

pub fn r(s: &str) -> Rational {
    s.parse().unwrap()
}

pub fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

/// A 30 fps project with `t1` (video) and `t2` (audio).
pub fn project() -> Project {
    new_project("p1".into(), "Demo".into(), secs(30)).unwrap()
}

pub fn tx(ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: "tx-test".into(),
        author: "test".into(),
        branch: "main".into(),
        base_rev: 0,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

fn asset(id: &str, duration: Rational, video: bool, audio: bool) -> Op {
    Op::AssetAdd {
        id: id.into(),
        path: format!("/media/{}.mov", id.trim_start_matches('$')),
        duration,
        video: video.then(|| VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        audio: audio.then_some(AudioInfo {
            sample_rate: 48_000,
            channels: 2,
        }),
        fingerprint: "fp".into(),
    }
}

/// An asset with video and audio streams.
pub fn av_asset(id: &str, duration: Rational) -> Op {
    asset(id, duration, true, true)
}

pub fn audio_asset(id: &str, duration: Rational) -> Op {
    asset(id, duration, false, true)
}

pub fn insert(
    id: &str,
    track: &str,
    at: Rational,
    asset: &str,
    source_in: Rational,
    duration: Rational,
) -> Op {
    Op::ClipInsert {
        id: id.into(),
        track: track.into(),
        at,
        asset: asset.into(),
        source_in,
        duration,
    }
}

pub fn try_apply(p: &Project, ops: Vec<Op>) -> Result<Applied, CoreError> {
    apply_tx(p, &tx(ops), &mut MemIds::after(p))
}

pub fn apply(p: &Project, ops: Vec<Op>) -> Applied {
    try_apply(p, ops).unwrap()
}

/// Applies `applied.inverse` (an internal transaction) to `applied.project`.
pub fn undo(applied: &Applied) -> Project {
    let p = &applied.project;
    apply_internal(p, &tx(applied.inverse.clone()), &mut MemIds::after(p))
        .unwrap()
        .project
}

/// A project with asset `a1` (100 s, video+audio) and back-to-back clips of `durations` on `t1`.
pub fn with_clips(durations: &[i64]) -> Project {
    let mut ops = vec![av_asset("$a", secs(100))];
    let mut at = 0;
    for (i, d) in durations.iter().enumerate() {
        ops.push(insert(
            &format!("$c{i}"),
            "t1",
            secs(at),
            "$a",
            Rational::zero(),
            secs(*d),
        ));
        at += d;
    }
    apply(&project(), ops).project
}

/// Sorted by start, non-overlapping, non-negative, positive durations, on every track.
pub fn assert_well_formed(p: &Project) {
    for track in &p.timeline.tracks {
        for c in &track.items {
            assert!(c.start >= Rational::zero(), "negative start {c:?}");
            assert!(c.source_in >= Rational::zero(), "negative sourceIn {c:?}");
            assert!(c.duration.is_positive(), "non-positive duration {c:?}");
            let asset = &p.assets[&c.asset];
            assert!(
                (c.source_in + c.duration).unwrap() <= asset.duration,
                "past asset end {c:?}"
            );
        }
        for w in track.items.windows(2) {
            let end = (w[0].start + w[0].duration).unwrap();
            assert!(
                end <= w[1].start,
                "overlap or unsorted on {}: {:?} then {:?}",
                track.id,
                w[0],
                w[1]
            );
        }
    }
}

pub fn clip<'a>(p: &'a Project, id: &str) -> &'a fin_core::Clip {
    p.timeline
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no clip {id}"))
}

pub fn starts(p: &Project, track: usize) -> Vec<(String, Rational)> {
    p.timeline.tracks[track]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start))
        .collect()
}
