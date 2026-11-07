//! Shared fixtures for the op tests.
#![allow(dead_code)]

pub mod ids_json;
pub mod memlog;
pub mod moves;

use fin_core::{
    Applied, AssetAdd, AudioInfo, CoreError, DeleteMode, Edge, InsertMode, ItemBody, ItemDelete,
    ItemInsert, ItemTrim, MemIds, Op, Project, Rational, SCHEMA_VERSION, TrimMode, Tx, VideoInfo,
    apply_internal, apply_tx, new_project,
};

/// A proptest config with `default` cases; `PROPTEST_CASES` overrides it (plan 4B Global
/// Constraints: every block sets its cases and honours the override).
pub fn prop_config(default: u32) -> proptest::test_runner::Config {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    proptest::test_runner::Config {
        cases,
        ..proptest::test_runner::Config::default()
    }
}

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
    AssetAdd::media(
        id,
        format!("/media/{}.mov", id.trim_start_matches('$')),
        duration,
        video.then(|| VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        audio.then_some(AudioInfo {
            sample_rate: 48_000,
            channels: 2,
        }),
        "fp",
    )
    .into()
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
    ItemInsert {
        id: id.into(),
        track: track.into(),
        at,
        duration,
        mode: InsertMode::Ripple,
        body: ItemBody::Clip {
            asset: asset.into(),
            source_in,
            speed: Rational::from_int(1),
        },
    }
    .into()
}

/// `item.trim` in ripple mode.
pub fn trim(item: &str, edge: Edge, delta: Rational) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode: TrimMode::Ripple,
        alone: false,
    }
    .into()
}

/// The inverse of a ripple trim: `item.trim` back in ripple mode, of `item` alone.
pub fn trim_back(item: &str, edge: Edge, delta: Rational) -> Op {
    ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode: TrimMode::Ripple,
        alone: true,
    }
    .into()
}

/// `item.delete` in ripple mode.
pub fn delete(item: &str) -> Op {
    ItemDelete {
        item: item.into(),
        mode: DeleteMode::Ripple,
    }
    .into()
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
    for track in &p.timelines[0].tracks {
        for c in &track.items {
            assert!(c.start >= Rational::zero(), "negative start {c:?}");
            let source_in = c.source_in().unwrap();
            assert!(source_in >= Rational::zero(), "negative sourceIn {c:?}");
            assert!(c.duration.is_positive(), "non-positive duration {c:?}");
            let asset = &p.assets[c.asset().unwrap()];
            assert!(
                (source_in + c.duration).unwrap() <= asset.duration.unwrap(),
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

/// A clip item's fields, flat.
#[derive(Debug, Clone, PartialEq)]
pub struct ClipView {
    pub id: String,
    pub asset: String,
    pub start: Rational,
    pub source_in: Rational,
    pub duration: Rational,
    pub speed: Rational,
}

/// Clip item `id`, flat.
pub fn clip(p: &Project, id: &str) -> ClipView {
    let item = p.timelines[0]
        .tracks
        .iter()
        .flat_map(|t| &t.items)
        .find(|c| c.id == id)
        .unwrap_or_else(|| panic!("no item {id}"));
    let ItemBody::Clip {
        asset,
        source_in,
        speed,
    } = &item.body
    else {
        panic!("{id} is not a clip");
    };
    ClipView {
        id: item.id.clone(),
        asset: asset.clone(),
        start: item.start,
        source_in: *source_in,
        duration: item.duration,
        speed: *speed,
    }
}

pub fn starts(p: &Project, track: usize) -> Vec<(String, Rational)> {
    p.timelines[0].tracks[track]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start))
        .collect()
}

/// Decision 23: the inverse records the op's shifts inverted, in exact reverse order.
pub fn inverse_shifts_mirror(applied: &Applied) {
    let p = &applied.project;
    let back = apply_internal(p, &tx(applied.inverse.clone()), &mut MemIds::after(p)).unwrap();
    let want = fin_core::rebase::StepMap::from_diff(&applied.diff)
        .invert()
        .unwrap();
    let strip = |s: &[fin_core::Shift]| -> Vec<(String, Rational, Rational)> {
        s.iter().map(|s| (s.track.clone(), s.from, s.by)).collect()
    };
    assert_eq!(
        strip(&back.diff.shifts),
        strip(want.shifts()),
        "{applied:?}"
    );
}

/// Applies `ops`, checks the inverse restores `p` exactly and mirrors the shifts.
pub fn golden_apply(p: &Project, ops: Vec<Op>) -> Applied {
    let a = apply(p, ops);
    assert_eq!(undo(&a), *p, "inverse restores the project");
    inverse_shifts_mirror(&a);
    a
}

/// A project with asset `a1` (100 s, video+audio) and back-to-back clips on `t1`, each
/// `(duration, sourceIn)`.
pub fn with_sourced(clips: &[(i64, i64)]) -> Project {
    let mut ops = vec![av_asset("$a", secs(100))];
    let mut at = 0;
    for (i, (d, s)) in clips.iter().enumerate() {
        ops.push(insert(
            &format!("$c{i}"),
            "t1",
            secs(at),
            "$a",
            secs(*s),
            secs(*d),
        ));
        at += d;
    }
    apply(&project(), ops).project
}
