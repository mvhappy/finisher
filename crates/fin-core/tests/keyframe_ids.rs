//! Stable keyframe ids (sub-project 4 spec §5.12, review finding #1): a stale keyframe edit
//! follows the key it met in its own frame, not its time, so trims around another author's
//! edit of that key cannot hide the conflict.

mod common;

use common::memlog::MemLog;
use common::*;
use fin_core::{
    CoreError, Edge, EffectAdd, Interp, Item, KeyframeRemove, KeyframeSet, Op, ParamValue, Project,
    Rational,
};
use proptest::prelude::*;
use serde_json::json;

const OPACITY: &str = "opacity";
const RADIUS: &str = "effects.fx1.radius";

fn key(item: &str, path: &str, at: Rational, v: f64) -> Op {
    KeyframeSet::timed(item, path, at, ParamValue::Number(v), Interp::Linear).into()
}

fn key_id(item: &str, path: &str, key: &str, at: Option<Rational>, v: f64) -> Op {
    KeyframeSet {
        item: item.into(),
        path: path.into(),
        key: Some(key.into()),
        at,
        value: ParamValue::Number(v),
        interp: Interp::Linear,
    }
    .into()
}

fn unkey(item: &str, path: &str, at: Rational) -> Op {
    KeyframeRemove::timed(item, path, at).into()
}

fn half() -> Rational {
    r("1/2")
}

fn item<'a>(p: &'a Project, id: &str) -> &'a Item {
    p.timelines
        .iter()
        .flat_map(|t| &t.tracks)
        .flat_map(|t| &t.items)
        .find(|i| i.id == id)
        .unwrap_or_else(|| panic!("no item {id}"))
}

/// `(at, value)` of every key of `path` on `id`.
fn keys(p: &Project, id: &str, path: &str) -> Vec<(Rational, f64)> {
    item(p, id)
        .common
        .keyframes
        .get(path)
        .map(|k| {
            k.iter()
                .map(|k| match k.value {
                    ParamValue::Number(v) => (k.at, v),
                    ref other => panic!("not a number: {other:?}"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `(id, at)` of every key of `path` on `id`.
fn ids(p: &Project, id: &str, path: &str) -> Vec<(String, Rational)> {
    item(p, id).common.keyframes[path]
        .iter()
        .map(|k| (k.id.clone(), k.at))
        .collect()
}

/// `c1` (10 s from source 10) with blur `fx1`; keys at 2 (1.0) and 6 (0.0) on opacity
/// (`kf1`, `kf2`) and at 2 (0) and 6 (20) on the blur radius (`kf3`, `kf4`).
fn genesis() -> Project {
    let p = with_sourced(&[(10, 10)]);
    apply(
        &p,
        vec![
            EffectAdd {
                item: "c1".into(),
                id: "$b".into(),
                kind: "blur".into(),
                at: None,
                params: Default::default(),
            }
            .into(),
            key("c1", OPACITY, secs(2), 1.0),
            key("c1", OPACITY, secs(6), 0.0),
            key("c1", RADIUS, secs(2), 0.0),
            key("c1", RADIUS, secs(6), 20.0),
        ],
    )
    .project
}

/// Bob trims the start of `c1` by 1/2, sets the key of `path` now at `edit_at` to `v`, and
/// trims another 1/2 (seqs 1–3; the edit is `tx-bob-edit`).
fn bob(path: &str, edit_at: Rational, v: f64) -> MemLog {
    let mut log = MemLog::new(genesis());
    log.commit(
        "main",
        "tx-bob-1",
        "bob",
        vec![trim("c1", Edge::Start, half())],
    );
    log.commit(
        "main",
        "tx-bob-edit",
        "bob",
        vec![key("c1", path, edit_at, v)],
    );
    log.commit(
        "main",
        "tx-bob-2",
        "bob",
        vec![trim("c1", Edge::Start, half())],
    );
    log
}

fn conflict(e: &CoreError) -> (String, Option<String>) {
    assert_eq!(e.code, "CONFLICT", "{e:?}");
    (
        e.details["reason"].as_str().unwrap().to_string(),
        e.details["againstTx"].as_str().map(str::to_string),
    )
}

fn same_param_against_bob() -> (String, Option<String>) {
    ("SAME_PARAM".into(), Some("tx-bob-edit".into()))
}

/// The validated repro: Alice's stale set and stale remove of the key at 2 meet Bob's edit of
/// that key, which two trims moved to 1. Before ids, both returned `Ok` and lost Bob's value.
#[test]
fn stale_set_and_remove_after_trim_edit_trim_are_same_param() {
    for path in [OPACITY, RADIUS] {
        let alice = [key("c1", path, secs(2), 0.25), unkey("c1", path, secs(2))];
        for op in alice {
            let mut log = bob(path, r("3/2"), 0.75);
            let before = keys(&log.state("main"), "c1", path);
            let e = log
                .commit_at("main", "tx-alice", "alice", 0, vec![op.clone()])
                .unwrap_err();
            assert_eq!(conflict(&e), same_param_against_bob(), "{path} {op:?}");
            assert_eq!(keys(&log.state("main"), "c1", path), before);
        }
    }
    let log = bob(OPACITY, r("3/2"), 0.75);
    assert_eq!(
        keys(&log.state("main"), "c1", OPACITY),
        [(secs(1), 0.75), (secs(5), 0.0)]
    );
}

/// The control: Bob edits the other key; Alice's stale edit of the key at 2 merges, and both
/// values hold.
#[test]
fn edits_of_independent_keys_merge_across_trims() {
    for path in [OPACITY, RADIUS] {
        let mut log = bob(path, r("11/2"), 0.75);
        log.commit_at(
            "main",
            "tx-alice",
            "alice",
            0,
            vec![key("c1", path, secs(2), 0.25)],
        )
        .unwrap();
        let got = keys(&log.state("main"), "c1", path);
        assert_eq!(got, [(secs(1), 0.25), (secs(5), 0.75)], "{path}");
    }
}

/// A stale time-form edit is stored as the key form of the key it met at its base.
#[test]
fn stale_time_form_is_stored_as_key_form() {
    let mut log = bob(OPACITY, r("11/2"), 0.75);
    let rec = log
        .commit_at(
            "main",
            "tx-alice",
            "alice",
            0,
            vec![key("c1", OPACITY, secs(2), 0.25)],
        )
        .unwrap();
    assert_eq!(
        rec.record.intent.ops,
        [key_id("c1", OPACITY, "kf1", Some(secs(1)), 0.25)]
    );
    assert_eq!(
        ids(&log.state("main"), "c1", OPACITY),
        [("kf1".to_string(), secs(1)), ("kf2".to_string(), secs(5))]
    );
}

/// Concurrent inserts at one time: the stale one is `KEYFRAME_EXISTS`, blamed on the first;
/// inserts at different times merge.
#[test]
fn concurrent_inserts_at_one_time_conflict() {
    let mut log = MemLog::new(genesis());
    log.commit(
        "main",
        "tx-bob-1",
        "bob",
        vec![trim("c1", Edge::Start, half())],
    );
    log.commit(
        "main",
        "tx-bob-ins",
        "bob",
        vec![key("c1", OPACITY, r("7/2"), 0.5)],
    );
    let e = log
        .commit_at(
            "main",
            "tx-alice",
            "alice",
            0,
            vec![key("c1", OPACITY, secs(4), 0.25)],
        )
        .unwrap_err();
    assert_eq!(
        conflict(&e),
        (
            "KEYFRAME_EXISTS".to_string(),
            Some("tx-bob-ins".to_string())
        )
    );
    log.commit_at(
        "main",
        "tx-alice-2",
        "alice",
        0,
        vec![key("c1", OPACITY, secs(3), 0.25)],
    )
    .unwrap();
    assert_eq!(
        keys(&log.state("main"), "c1", OPACITY),
        [
            (r("3/2"), 1.0),
            (r("5/2"), 0.25),
            (r("7/2"), 0.5),
            (r("11/2"), 0.0)
        ]
    );
}

/// A stale move of a key onto a time another record filled since is refused the same way.
#[test]
fn stale_move_onto_a_filled_time_conflicts() {
    let mut log = MemLog::new(genesis());
    log.commit(
        "main",
        "tx-bob-ins",
        "bob",
        vec![key("c1", OPACITY, secs(4), 0.5)],
    );
    let e = log
        .commit_at(
            "main",
            "tx-alice",
            "alice",
            0,
            vec![key_id("c1", OPACITY, "kf1", Some(secs(4)), 0.25)],
        )
        .unwrap_err();
    assert_eq!(
        conflict(&e),
        (
            "KEYFRAME_EXISTS".to_string(),
            Some("tx-bob-ins".to_string())
        )
    );
}

/// Merging two branches that inserted a key at the same time conflicts, blamed on the
/// target's insert; inserts at different times merge.
#[test]
fn merged_inserts_at_one_time_conflict() {
    let mut log = MemLog::new(genesis());
    log.fork("b", "main", 0);
    log.commit(
        "b",
        "tx-alice",
        "alice",
        vec![key("c1", OPACITY, secs(4), 0.25)],
    );
    log.commit(
        "main",
        "tx-bob",
        "bob",
        vec![key("c1", OPACITY, secs(4), 0.5)],
    );
    let e = log.merge("b", "main", "alice", "mg-1", false).unwrap_err();
    assert_eq!(
        conflict(&e),
        ("KEYFRAME_EXISTS".to_string(), Some("tx-bob".to_string()))
    );
    let mut log = MemLog::new(genesis());
    log.fork("b", "main", 0);
    log.commit(
        "b",
        "tx-alice",
        "alice",
        vec![key("c1", OPACITY, secs(3), 0.25)],
    );
    log.commit(
        "main",
        "tx-bob",
        "bob",
        vec![key("c1", OPACITY, secs(4), 0.5)],
    );
    log.merge("b", "main", "alice", "mg-1", false).unwrap();
    // The copy keeps the id the source minted.
    let got = ids(&log.state("main"), "c1", OPACITY);
    let source = ids(&log.state("b"), "c1", OPACITY);
    let copied = source.iter().find(|(_, at)| *at == secs(3)).unwrap();
    assert!(got.contains(copied), "{got:?} lacks {copied:?}");
}

// ---- ops ----

fn code(p: &Project, ops: Vec<Op>) -> String {
    try_apply(p, ops).unwrap_err().code
}

/// Every form of `keyframe.set`: the time form edits the key at `at` (keeping its id) or
/// inserts one; the key form edits, moves or (by placeholder) inserts; each inverse is in key
/// form and undoes it exactly.
#[test]
fn keyframe_set_forms_and_inverses() {
    let p = genesis();
    // Time form on a held time: an edit of kf1.
    let a = golden_apply(&p, vec![key("c1", OPACITY, secs(2), 0.5)]);
    assert_eq!(ids(&a.project, "c1", OPACITY)[0], ("kf1".into(), secs(2)));
    assert_eq!(
        a.inverse,
        [key_id("c1", OPACITY, "kf1", Some(secs(2)), 1.0)]
    );
    // Time form on a free time: an insert under a new id; its inverse removes it by id.
    let a = golden_apply(&p, vec![key("c1", OPACITY, secs(4), 0.5)]);
    assert_eq!(ids(&a.project, "c1", OPACITY)[1], ("kf5".into(), secs(4)));
    assert_eq!(a.bindings["$kf0"], "kf5");
    assert_eq!(
        a.inverse,
        [KeyframeRemove::keyed("c1", OPACITY, "kf5").into()]
    );
    // Key form without `at`: an edit in place.
    let a = golden_apply(&p, vec![key_id("c1", OPACITY, "kf2", None, 0.5)]);
    assert_eq!(
        keys(&a.project, "c1", OPACITY),
        [(secs(2), 1.0), (secs(6), 0.5)]
    );
    // Key form with another `at`: a move, which re-sorts the list.
    let a = golden_apply(&p, vec![key_id("c1", OPACITY, "kf1", Some(secs(8)), 0.5)]);
    assert_eq!(
        ids(&a.project, "c1", OPACITY),
        [("kf2".into(), secs(6)), ("kf1".into(), secs(8))]
    );
    assert_eq!(
        a.inverse,
        [key_id("c1", OPACITY, "kf1", Some(secs(2)), 1.0)]
    );
    // A placeholder inserts and binds; a later op of the tx names the new key by it.
    let a = golden_apply(
        &p,
        vec![
            key_id("c1", OPACITY, "$k", Some(secs(3)), 0.5),
            key_id("c1", OPACITY, "$k", Some(secs(4)), 0.25),
            KeyframeRemove::keyed("c1", OPACITY, "$k").into(),
        ],
    );
    assert_eq!(a.bindings["$k"], "kf5");
    assert_eq!(ids(&a.project, "c1", OPACITY).len(), 2);
}

/// The refusals: an unknown key (`NOT_FOUND`), a time another key holds (`KEYFRAME_EXISTS`,
/// naming it), a new key without a time, a removal naming both or neither of key and time
/// (`INVALID_OP`), and a removal of a time with no key (`NOT_FOUND`).
#[test]
fn keyframe_op_errors() {
    let p = genesis();
    assert_eq!(
        code(&p, vec![key_id("c1", OPACITY, "kf9", None, 0.5)]),
        "NOT_FOUND"
    );
    // kf3 is the radius's: keys are per path.
    assert_eq!(
        code(&p, vec![key_id("c1", OPACITY, "kf3", None, 0.5)]),
        "NOT_FOUND"
    );
    let e = try_apply(&p, vec![key_id("c1", OPACITY, "kf1", Some(secs(6)), 0.5)]).unwrap_err();
    assert_eq!(e.code, "KEYFRAME_EXISTS");
    assert_eq!(
        (&e.details["key"], &e.details["existing"], &e.details["at"]),
        (&json!("kf1"), &json!("kf2"), &json!("6"))
    );
    assert_eq!(
        code(&p, vec![key_id("c1", OPACITY, "$k", Some(secs(6)), 0.5)]),
        "KEYFRAME_EXISTS"
    );
    assert_eq!(
        code(&p, vec![key_id("c1", OPACITY, "$k", None, 0.5)]),
        "INVALID_OP"
    );
    let both = KeyframeRemove {
        at: Some(secs(2)),
        ..KeyframeRemove::keyed("c1", OPACITY, "kf1")
    };
    let neither = KeyframeRemove {
        key: None,
        ..KeyframeRemove::keyed("c1", OPACITY, "kf1")
    };
    assert_eq!(code(&p, vec![both.into()]), "INVALID_OP");
    assert_eq!(code(&p, vec![neither.into()]), "INVALID_OP");
    assert_eq!(code(&p, vec![unkey("c1", OPACITY, secs(3))]), "NOT_FOUND");
}

/// A generated placeholder never takes a name the client's transaction uses, before or after
/// the time-form op: each insert stays its own key.
#[test]
fn generated_placeholders_avoid_the_clients() {
    let p = genesis();
    for ops in [
        vec![
            key_id("c1", OPACITY, "$kf1", Some(secs(3)), 0.5),
            key("c1", OPACITY, secs(4), 0.5),
        ],
        vec![
            key("c1", OPACITY, secs(1), 0.5),
            key_id("c1", OPACITY, "$kf0", Some(secs(3)), 0.7),
        ],
    ] {
        let t = fin_core::canonicalize(&p, &tx(ops.clone())).unwrap();
        let a = golden_apply(&p, ops);
        assert_eq!(keys(&a.project, "c1", OPACITY).len(), 4, "{:?}", t.ops);
        let placeholders: Vec<&str> = t
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::KeyframeSet(k) => k.key.as_deref(),
                _ => None,
            })
            .collect();
        assert_eq!(placeholders.len(), 2);
        assert_ne!(placeholders[0], placeholders[1]);
    }
}

/// `keyframe.restore` (a removal's inverse) puts the key back under its id; it is internal,
/// refuses a held time and an id the item holds, and replay checks its id's shape.
#[test]
fn keyframe_restore_round_trip_and_refusals() {
    let p = genesis();
    let a = golden_apply(&p, vec![unkey("c1", OPACITY, secs(2))]);
    let Op::KeyframeRestore(back) = &a.inverse[0] else {
        panic!("{:?}", a.inverse)
    };
    assert_eq!((back.key.as_str(), back.at), ("kf1", secs(2)));
    // Clients cannot send it.
    assert_eq!(code(&a.project, vec![a.inverse[0].clone()]), "INVALID_OP");
    let restore = |key: &str, at: Rational| -> Op {
        fin_core::KeyframeRestore {
            key: key.into(),
            at,
            ..(**back).clone()
        }
        .into()
    };
    let internal = |p: &Project, op: Op| {
        fin_core::apply_internal(p, &tx(vec![op]), &mut fin_core::MemIds::after(p))
    };
    assert_eq!(
        internal(&a.project, restore("kf1", secs(6)))
            .unwrap_err()
            .code,
        "KEYFRAME_EXISTS"
    );
    // kf3 is held by another path of the item.
    assert_eq!(
        internal(&a.project, restore("kf3", secs(3)))
            .unwrap_err()
            .code,
        "DUPLICATE_ID"
    );
    let e = fin_core::apply_tx_mode(
        &a.project,
        &tx(vec![restore("k1", secs(3))]),
        &mut fin_core::MemIds::after(&a.project),
        fin_core::ApplyMode::Replay,
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_ID");
    // Replay refuses the time form: history names keys.
    let e = fin_core::apply_tx_mode(
        &p,
        &tx(vec![key("c1", OPACITY, secs(2), 0.5)]),
        &mut fin_core::MemIds::after(&p),
        fin_core::ApplyMode::Replay,
    )
    .unwrap_err();
    assert_eq!(e.code, "INVALID_OP");
}

/// Canonical form: the committed intent names keys (a key the tx made by its placeholder),
/// and replays to the same state.
#[test]
fn canonical_form_names_keys() {
    let p = genesis();
    let t = tx(vec![
        key("c1", OPACITY, secs(4), 0.5),
        key("c1", OPACITY, secs(4), 0.25),
        unkey("c1", OPACITY, secs(6)),
    ]);
    let c = fin_core::canonicalize(&p, &t).unwrap();
    assert_eq!(
        c.ops,
        [
            key_id("c1", OPACITY, "$kf0", Some(secs(4)), 0.5),
            key_id("c1", OPACITY, "$kf0", Some(secs(4)), 0.25),
            KeyframeRemove::keyed("c1", OPACITY, "kf2").into(),
        ]
    );
    let a = fin_core::apply_tx(&p, &c, &mut fin_core::MemIds::after(&p)).unwrap();
    assert_eq!(
        a.project,
        fin_core::apply_tx(&p, &t, &mut fin_core::MemIds::after(&p))
            .unwrap()
            .project
    );
    let rp = fin_core::apply_committed(&p, &c, &a.bindings).unwrap();
    assert_eq!(rp.project, a.project);
    assert_eq!(
        keys(&a.project, "c1", OPACITY),
        [(secs(2), 1.0), (secs(4), 0.25)]
    );
}

/// A split copies the keys to the right part with their ids; an effect removal and its undo
/// keep the effect's key ids.
#[test]
fn split_and_effect_removal_keep_key_ids() {
    let p = genesis();
    let a = apply(
        &p,
        vec![
            fin_core::ItemSplit {
                item: "c1".into(),
                offset: secs(4),
                id: "$r".into(),
            }
            .into(),
        ],
    );
    let right = a.bindings["$r"].clone();
    assert_eq!(
        ids(&a.project, &right, OPACITY)
            .iter()
            .map(|k| k.0.as_str())
            .collect::<Vec<_>>(),
        ["kf1", "kf2"]
    );
    let a = golden_apply(
        &p,
        vec![
            fin_core::EffectRemove {
                item: "c1".into(),
                effect: "fx1".into(),
            }
            .into(),
        ],
    );
    let back = undo(&a);
    assert_eq!(ids(&back, "c1", RADIUS), ids(&p, "c1", RADIUS));
}

/// An undone removal comes back under its id at its mapped time, or is `KEYFRAME_EXISTS`
/// when another author's key holds that time.
#[test]
fn undo_of_a_removal_restores_the_id_or_conflicts() {
    let mut log = MemLog::new(genesis());
    log.commit(
        "main",
        "tx-rm",
        "alice",
        vec![unkey("c1", OPACITY, secs(2))],
    );
    log.commit(
        "main",
        "tx-trim",
        "bob",
        vec![trim("c1", Edge::Start, half())],
    );
    log.undo("main", "alice", "tx-undo").unwrap();
    assert_eq!(
        ids(&log.state("main"), "c1", OPACITY),
        [
            ("kf1".to_string(), r("3/2")),
            ("kf2".to_string(), r("11/2"))
        ]
    );
    let mut log = MemLog::new(genesis());
    log.commit(
        "main",
        "tx-rm",
        "alice",
        vec![unkey("c1", OPACITY, secs(2))],
    );
    log.commit(
        "main",
        "tx-ins",
        "bob",
        vec![key("c1", OPACITY, secs(2), 0.5)],
    );
    let e = log.undo("main", "alice", "tx-undo").unwrap_err();
    assert_eq!(
        conflict(&e),
        ("KEYFRAME_EXISTS".to_string(), Some("tx-ins".to_string()))
    );
}

/// Put-backs hold key lists with their ids: those ids are unique within the item and, in
/// replay, `kf<n>`; their times strictly increase.
#[test]
fn put_backs_check_key_ids() {
    let p = genesis();
    let removed = apply(
        &p,
        vec![
            fin_core::EffectRemove {
                item: "c1".into(),
                effect: "fx1".into(),
            }
            .into(),
        ],
    );
    let Op::EffectRestore(back) = &removed.inverse[0] else {
        panic!("{:?}", removed.inverse)
    };
    let internal = |op: Op, mode: fin_core::ApplyMode| {
        fin_core::apply_tx_mode(
            &removed.project,
            &tx(vec![op]),
            &mut fin_core::MemIds::after(&removed.project),
            mode,
        )
    };
    // An effect key under an id the item's opacity keys hold.
    let mut dup = (**back).clone();
    dup.keyframes.get_mut(RADIUS).unwrap()[0].id = "kf1".into();
    let e = internal(dup.into(), fin_core::ApplyMode::Internal).unwrap_err();
    assert_eq!(e.code, "DUPLICATE_ID");
    let mut bad = (**back).clone();
    bad.keyframes.get_mut(RADIUS).unwrap()[0].id = "k9".into();
    let e = internal(bad.into(), fin_core::ApplyMode::Replay).unwrap_err();
    assert_eq!(e.code, "INVALID_ID");
    // An item put back with two keys under one id.
    let deleted = apply(&p, vec![delete("c1")]);
    let Op::ItemRestore(r) = deleted.inverse.last().unwrap().clone() else {
        panic!("{:?}", deleted.inverse)
    };
    let mut r = r;
    r.item.common.keyframes.get_mut(OPACITY).unwrap()[1].id = "kf1".into();
    let e = fin_core::apply_internal(
        &deleted.project,
        &tx(vec![(*r).into()]),
        &mut fin_core::MemIds::after(&deleted.project),
    )
    .unwrap_err();
    assert_eq!(e.code, "DUPLICATE_ID");
}

// ---- property: conflicts follow the key, whatever trims did ----

/// One of Bob's records: a start trim by `n`/2 (`n` in {-1, 1}) or an edit of key `kf<k>`
/// (of `path`'s two) by id.
#[derive(Debug, Clone)]
enum Bob {
    Trim(i64),
    Edit(usize, f64),
}

fn bob_step() -> impl Strategy<Value = Bob> {
    prop_oneof![
        prop_oneof![Just(-1i64), Just(1)].prop_map(Bob::Trim),
        (0usize..2, 0u8..10).prop_map(|(k, v)| Bob::Edit(k, f64::from(v) / 10.0)),
    ]
}

proptest! {
    #![proptest_config(prop_config(256))]
    /// Bob's trims and key edits, then Alice's stale edit or removal (by time, at the base) of
    /// one key: `SAME_PARAM` exactly when Bob edited that key; otherwise it merges, and every
    /// key holds the value its last author gave it, by id.
    #[test]
    fn stale_keyframe_ops_meet_edits_of_the_same_key(
        effect in any::<bool>(),
        steps in prop::collection::vec(bob_step(), 0..8),
        target in 0usize..2,
        remove in any::<bool>(),
    ) {
        let path = if effect { RADIUS } else { OPACITY };
        let p = genesis();
        let ids0 = ids(&p, "c1", path);
        let mut log = MemLog::new(p);
        let mut want: Vec<f64> = keys(&log.state("main"), "c1", path).iter().map(|k| k.1).collect();
        let mut edited = [false, false];
        for (i, s) in steps.iter().enumerate() {
            let op = match s {
                Bob::Trim(n) => trim("c1", Edge::Start, Rational::new(*n, 2).unwrap()),
                Bob::Edit(k, v) => {
                    edited[*k] = true;
                    want[*k] = *v;
                    key_id("c1", path, &ids0[*k].0, None, *v)
                }
            };
            log.commit("main", &format!("tx-bob-{i}"), "bob", vec![op]);
        }
        let at = ids0[target].1;
        let op = if remove { unkey("c1", path, at) } else { key("c1", path, at, 0.25) };
        // Where trims moved the key: an edit there outside the item is refused by policy.
        let head = log.state("main");
        let moved = ids(&head, "c1", path)[target].1;
        let outside = moved < Rational::zero() || moved > item(&head, "c1").duration;
        let got = log.commit_at("main", "tx-alice", "alice", 0, vec![op]);
        if edited[target] {
            let e = got.unwrap_err();
            prop_assert_eq!(conflict(&e).0, "SAME_PARAM");
        } else if outside && !remove {
            prop_assert_eq!(conflict(&got.unwrap_err()).0, "INVALID_RANGE");
        } else {
            got.unwrap();
            let after = log.state("main");
            let held: Vec<(String, f64)> = item(&after, "c1").common.keyframes[path]
                .iter()
                .map(|k| match k.value {
                    ParamValue::Number(v) => (k.id.clone(), v),
                    _ => unreachable!(),
                })
                .collect();
            let mut expect = Vec::new();
            for (k, (id, _)) in ids0.iter().enumerate() {
                if k == target {
                    if !remove {
                        expect.push((id.clone(), 0.25));
                    }
                } else {
                    expect.push((id.clone(), want[k]));
                }
            }
            prop_assert_eq!(held, expect);
        }
    }
}
