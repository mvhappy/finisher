//! `apply_committed`: replay re-applies committed history with structural checks only.

mod common;

use std::collections::BTreeMap;

use common::*;
use fin_core::{
    BoundIds, IdAlloc, IdKind, MemIds, Op, Rational, apply_committed, apply_tx, resolve_ops,
};

fn bindings(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn replay_matches_the_live_apply() {
    let p = with_clips(&[3, 4]);
    let intent = tx(vec![
        av_asset("$b", secs(50)),
        insert("$x", "t1", secs(3), "$b", secs(1), secs(2)),
        insert("$y", "t2", secs(0), "a1", secs(0), secs(6)),
    ]);
    let live = apply_tx(&p, &intent, &mut MemIds::after(&p)).unwrap();
    let replayed = apply_committed(&p, &intent, &live.bindings).unwrap();
    assert_eq!(replayed, live);
}

#[test]
fn replay_with_bindings_equals_applying_resolved_ops() {
    let p = with_clips(&[3]);
    let intent = tx(vec![insert("$x", "t1", secs(3), "a1", secs(0), secs(2))]);
    let b = bindings(&[("$x", "c7")]);
    let replayed = apply_committed(&p, &intent, &b).unwrap();
    assert_eq!(clip(&replayed.project, "c7").start, secs(3));
    let resolved = resolve_ops(&intent.ops, &b).unwrap();
    let direct = fin_core::apply_internal(&p, &tx(resolved), &mut MemIds::after(&p)).unwrap();
    assert_eq!(replayed.project, direct.project);
}

/// `INSERT_INSIDE_CLIP` cannot be the policy-only example: with ripple, an insert strictly
/// inside a clip always overlaps it, which replay rejects structurally (see the next test).
/// The one-frame minimum and the asset source range are policy-only.
#[test]
fn replay_ignores_policy_rules() {
    let p = with_clips(&[3]);
    let half_frame: Rational = r("1/60");
    let short = tx(vec![insert("$x", "t1", secs(3), "a1", secs(0), half_frame)]);
    assert_eq!(
        apply_tx(&p, &short, &mut MemIds::after(&p))
            .unwrap_err()
            .code,
        "INVALID_RANGE"
    );
    let replayed = apply_committed(&p, &short, &bindings(&[("$x", "c2")])).unwrap();
    assert_eq!(clip(&replayed.project, "c2").duration, half_frame);

    let past_end = tx(vec![insert("$x", "t1", secs(3), "a1", secs(99), secs(5))]);
    assert_eq!(
        apply_tx(&p, &past_end, &mut MemIds::after(&p))
            .unwrap_err()
            .code,
        "INVALID_RANGE"
    );
    apply_committed(&p, &past_end, &bindings(&[("$x", "c2")])).unwrap();

    let p = with_clips(&[3]);
    let trim = tx(vec![Op::ClipTrim {
        clip: "c1".into(),
        edge: fin_core::Edge::End,
        delta: secs(500),
    }]);
    assert_eq!(
        apply_tx(&p, &trim, &mut MemIds::after(&p))
            .unwrap_err()
            .code,
        "INVALID_RANGE"
    );
    let trimmed = apply_committed(&p, &trim, &BTreeMap::new()).unwrap();
    assert_eq!(clip(&trimmed.project, "c1").duration, secs(503));
}

#[test]
fn replay_rejects_insert_inside_a_clip_as_overlap() {
    let p = with_clips(&[4]);
    let inside = tx(vec![insert("$x", "t1", secs(2), "a1", secs(0), secs(1))]);
    let live = apply_tx(&p, &inside, &mut MemIds::after(&p)).unwrap_err();
    assert_eq!(live.code, "INSERT_INSIDE_CLIP");
    let e = apply_committed(&p, &inside, &bindings(&[("$x", "c2")])).unwrap_err();
    assert_eq!(e.code, "REPLAY_FAILED");
    assert_eq!(e.details["cause"]["code"], "CLIP_OVERLAP");
}

#[test]
fn replay_rejects_structural() {
    let p = with_clips(&[3]);
    let intent = tx(vec![Op::ClipDelete { clip: "c9".into() }]);
    let e = apply_committed(&p, &intent, &BTreeMap::new()).unwrap_err();
    assert_eq!(e.code, "REPLAY_FAILED");
    assert_eq!(e.details["txId"], "tx-test");
    assert_eq!(e.details["cause"]["code"], "NOT_FOUND");
    assert_eq!(e.details["cause"]["details"]["opIndex"], 0);
}

#[test]
fn replay_rejects_wrong_kind_on_track() {
    let p = with_clips(&[3]);
    let intent = tx(vec![
        audio_asset("$s", secs(10)),
        insert("$x", "t1", secs(3), "$s", secs(0), secs(2)),
    ]);
    let b = bindings(&[("$s", "a2"), ("$x", "c2")]);
    let e = apply_committed(&p, &intent, &b).unwrap_err();
    assert_eq!(e.details["cause"]["code"], "TRACK_KIND_MISMATCH");
}

#[test]
fn replay_rejects_bindings_that_do_not_match() {
    let p = with_clips(&[3]);
    let intent = tx(vec![insert("$x", "t1", secs(3), "a1", secs(0), secs(2))]);

    let e = apply_committed(&p, &intent, &BTreeMap::new()).unwrap_err();
    assert_eq!(e.code, "REPLAY_FAILED");
    assert_eq!(e.details["cause"]["code"], "UNBOUND_PLACEHOLDER");

    let e = apply_committed(&p, &intent, &bindings(&[("$x", "a5")])).unwrap_err();
    assert_eq!(e.details["cause"]["code"], "BINDINGS_MISMATCH");

    let extra = bindings(&[("$x", "c2"), ("$y", "c3")]);
    let e = apply_committed(&p, &intent, &extra).unwrap_err();
    assert_eq!(e.details["cause"]["code"], "BINDINGS_MISMATCH");

    // A bound id that already exists is a duplicate, not a silent overwrite.
    let e = apply_committed(&p, &intent, &bindings(&[("$x", "c1")])).unwrap_err();
    assert_eq!(e.details["cause"]["code"], "DUPLICATE_ID");
}

#[test]
fn bound_ids_hand_out_bindings_in_placeholder_order() {
    let ops = vec![
        av_asset("$a", secs(10)),
        insert("$c", "t1", secs(0), "$a", secs(0), secs(1)),
    ];
    let b = bindings(&[("$a", "a4"), ("$c", "c9")]);
    let mut ids = BoundIds::new(&ops, &b).unwrap();
    assert_eq!(ids.next(IdKind::Asset), "a4");
    assert_eq!(ids.next(IdKind::Clip), "c9");
    assert!(ids.finished());
}

fn cause(p: &fin_core::Project, ops: Vec<Op>, b: &BTreeMap<String, String>) -> String {
    let e = apply_committed(p, &tx(ops), b).unwrap_err();
    assert_eq!(e.code, "REPLAY_FAILED");
    e.details["cause"]["code"].as_str().unwrap().to_string()
}

#[test]
fn replay_rejects_non_positive_asset_duration() {
    let ops = vec![
        av_asset("$a", secs(-5)),
        insert("$x", "t1", secs(0), "$a", secs(0), secs(2)),
    ];
    let b = bindings(&[("$a", "a1"), ("$x", "c1")]);
    assert_eq!(cause(&project(), ops, &b), "INVALID_RANGE");
    let zero = vec![av_asset("$a", secs(0))];
    assert_eq!(
        cause(&project(), zero, &bindings(&[("$a", "a1")])),
        "INVALID_RANGE"
    );
}

#[test]
fn replay_rejects_negative_start_and_source_in() {
    let p = with_clips(&[3]);
    let b = bindings(&[("$x", "c2")]);
    let at = vec![insert("$x", "t1", secs(-10), "a1", secs(0), secs(2))];
    assert_eq!(cause(&p, at, &b), "CLIP_NEGATIVE");
    let source = vec![insert("$x", "t1", secs(3), "a1", secs(-3), secs(2))];
    assert_eq!(cause(&p, source, &b), "CLIP_NEGATIVE");
}

#[test]
fn replay_rejects_a_trim_to_nothing() {
    let p = with_clips(&[3]);
    let trim = vec![Op::ClipTrim {
        clip: "c1".into(),
        edge: fin_core::Edge::End,
        delta: secs(-3),
    }];
    assert_eq!(cause(&p, trim, &BTreeMap::new()), "CLIP_EMPTY");
}

#[test]
fn replay_rejects_removing_an_asset_in_use() {
    let p = with_clips(&[3]);
    let ops = vec![Op::AssetRemove { id: "a1".into() }];
    assert_eq!(cause(&p, ops, &BTreeMap::new()), "ASSET_IN_USE");
}

#[test]
fn replay_rejects_a_restore_without_ripple_that_overlaps() {
    let p = with_clips(&[4]);
    let mut c = clip(&p, "c1").clone();
    c.id = "c5".into();
    c.start = secs(2);
    let ops = vec![Op::ClipRestore {
        clip: Box::new(c),
        track: "t1".into(),
        ripple: false,
    }];
    assert_eq!(cause(&p, ops, &BTreeMap::new()), "CLIP_OVERLAP");
}

#[test]
fn replay_rejects_a_duplicate_placeholder() {
    let p = with_clips(&[3]);
    let ops = vec![
        insert("$x", "t1", secs(3), "a1", secs(0), secs(1)),
        insert("$x", "t1", secs(4), "a1", secs(0), secs(1)),
    ];
    assert_eq!(
        cause(&p, ops, &bindings(&[("$x", "c2")])),
        "DUPLICATE_PLACEHOLDER"
    );
}

#[test]
fn replay_rejects_literal_ids_that_do_not_fit_their_kind() {
    let p = with_clips(&[3]);
    let wrong = vec![av_asset("c9", secs(10))];
    assert_eq!(cause(&p, wrong, &BTreeMap::new()), "INVALID_ID");
    let right = vec![av_asset("a9", secs(10))];
    apply_committed(&p, &tx(right), &BTreeMap::new()).unwrap();

    let mut c = clip(&p, "c1").clone();
    c.id = "$r".into();
    c.start = secs(3);
    let restore = |clip| Op::ClipRestore {
        clip: Box::new(clip),
        track: "t1".into(),
        ripple: true,
    };
    // A placeholder restore (spec §4.3) needs its recorded binding.
    assert_eq!(
        cause(&p, vec![restore(c.clone())], &BTreeMap::new()),
        "UNBOUND_PLACEHOLDER"
    );
    c.id = "x5".into();
    assert_eq!(
        cause(&p, vec![restore(c.clone())], &BTreeMap::new()),
        "INVALID_ID"
    );
    c.id = "c5".into();
    apply_committed(&p, &tx(vec![restore(c)]), &BTreeMap::new()).unwrap();
}

#[test]
fn internal_mode_still_accepts_any_literal_id() {
    let p = with_clips(&[3]);
    let ops = vec![av_asset("c9", secs(10))];
    fin_core::apply_internal(&p, &tx(ops), &mut MemIds::after(&p)).unwrap();
}
