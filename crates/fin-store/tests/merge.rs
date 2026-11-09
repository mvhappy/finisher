//! Merge through the store (plan 4B Task 13): `merge` records appended in one transaction,
//! conflicts listed in full, `--skip-conflicting`, retries and re-merges, crashes, dry runs on
//! a follower, merging both ways, undoing a merge unit, and followers applying a merge, a
//! prefix of it first.

mod integration_support;

use std::fs;
use std::path::PathBuf;

use fin_core::branch::BranchState;
use fin_core::{
    CommittedTx, CoreError, Edge, Link, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo,
    apply_committed,
};
use fin_store::{BranchCreate, HistoryReq, MAIN, MergeReport, MergeReq, Store, StoreError};
use integration_support as is;
use rusqlite::Connection;
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

struct Env {
    _tmp: TempDir,
    data: PathBuf,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    Env {
        data: tmp.path().join("server-data"),
        dir: tmp.path().join("p"),
        _tmp: tmp,
    }
}

fn asset() -> Op {
    fin_core::AssetAdd::media(
        "$a",
        "/media/a.mov",
        secs(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        None,
        "fp",
    )
    .into()
}

fn insert(id: &str, at: i64, duration: i64) -> Op {
    fin_core::ItemInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        duration: secs(duration),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: "a1".into(),
            source_in: secs(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into()
}

fn delete(clip: &str) -> Op {
    fin_core::ItemDelete {
        item: clip.into(),
        mode: fin_core::DeleteMode::Ripple,
    }
    .into()
}

fn trim(clip: &str, delta: i64) -> Op {
    fin_core::ItemTrim {
        item: clip.into(),
        edge: Edge::End,
        delta: secs(delta),
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    }
    .into()
}

fn edit(s: &mut Store, branch: &str, id: &str, author: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: author.into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
}

fn req(id: &str, from: &str, into: &str) -> MergeReq {
    MergeReq {
        tx_id: id.into(),
        author: "carol".into(),
        from: from.into(),
        into: into.into(),
        skip_conflicting: false,
        dry_run: false,
        args_hash: None,
    }
}

fn merge(s: &mut Store, id: &str, from: &str, into: &str) -> MergeReport {
    s.merge(req(id, from, into)).unwrap()
}

fn code(e: StoreError) -> CoreError {
    e.into()
}

fn state(s: &Store, branch: &str) -> Project {
    s.snapshot(branch).unwrap().1
}

fn starts(p: &Project) -> Vec<(String, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start))
        .collect()
}

fn at(list: &[(&str, i64)]) -> Vec<(String, Rational)> {
    list.iter()
        .map(|(c, s)| (c.to_string(), secs(*s)))
        .collect()
}

/// Everything a write could leave behind, for "nothing was written" checks.
fn rows(s: &Store) -> Vec<String> {
    let c = Connection::open(s.db_path()).unwrap();
    let mut out = vec![];
    for q in [
        "SELECT branch || ':' || seq || ':' || tx_id FROM txs ORDER BY branch, seq",
        "SELECT branch || ':' || seq FROM snapshots ORDER BY branch, seq",
        "SELECT kind || '=' || next FROM ids ORDER BY kind",
        "SELECT tx_id || ':' || kind || ':' || coalesce(merge_id, '') FROM record_links ORDER BY tx_id",
    ] {
        let mut stmt = c.prepare(q).unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(r.map(Result::unwrap));
    }
    out
}

/// Main at rev 2: the asset `a1`, then `c1` [0,10), `c2` [10,20), `c3` [20,30).
fn setup(s: &mut Store) {
    edit(s, MAIN, "s1", "setup", vec![asset()]);
    let clips = vec![
        insert("$x", 0, 10),
        insert("$y", 10, 10),
        insert("$z", 20, 10),
    ];
    edit(s, MAIN, "s2", "setup", clips);
}

/// `br1` ("feature") forked from main at its head.
fn branch(s: &mut Store) {
    s.create_branch(BranchCreate {
        tx_id: "b1".into(),
        author: "alice".into(),
        name: Some("feature".into()),
        from: MAIN.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
}

fn project(e: &Env) -> Store {
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    setup(&mut s);
    branch(&mut s);
    s
}

fn tx_ids(c: &[CommittedTx]) -> Vec<&str> {
    c.iter().map(|c| c.tx.tx_id.as_str()).collect()
}

#[test]
fn merge_child_without_target_edits_applies_all() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c1", -1)]);
    edit(&mut s, "br1", "e3", "bob", vec![delete("c2")]);
    let r = merge(&mut s, "m", "feature", MAIN);
    assert_eq!(r.merged, ["e1", "e2", "e3"]);
    assert!(r.skipped.is_empty() && r.excluded.is_empty() && !r.dry_run);
    assert_eq!(
        r.source_state,
        BranchState::Merged,
        "nothing of br1 is left"
    );
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
    assert_eq!(s.head(MAIN), 3 + 3, "after the branch record");
    let v = s.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn merge_rebases_over_target_edits_on_same_track() {
    let e = env();
    let mut s = project(&e);
    // br1 appends P at 30; main inserts W (5 s) at 0, so P lands at 35, keeping its id.
    let p = edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    let r = merge(&mut s, "m", "br1", MAIN);
    let c = &r.committed[0];
    assert_eq!(c.bindings, p.bindings);
    let Op::ItemInsert(i) = &c.tx.ops[0] else {
        panic!("{:?}", c.tx.ops)
    };
    assert_eq!(i.at, secs(35));
    assert_eq!(
        starts(&state(&s, MAIN)),
        at(&[("c5", 0), ("c1", 5), ("c2", 15), ("c3", 25), ("c4", 35)])
    );
}

#[test]
fn merge_conflict_returns_full_list_and_commits_nothing() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![trim("c3", -2)]);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c2", -1)]);
    edit(&mut s, "br1", "e3", "alice", vec![insert("$p", 0, 1)]);
    edit(&mut s, MAIN, "t1", "bob", vec![delete("c3")]);
    edit(&mut s, MAIN, "t2", "bob", vec![delete("c2")]);
    let before = rows(&s);
    let err = code(s.merge(req("m", "br1", MAIN)).unwrap_err());
    assert_eq!(err.code, "CONFLICT");
    let all: Vec<(&str, &str, Option<&str>)> = err.details["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["txId"].as_str().unwrap(),
                c["reason"].as_str().unwrap(),
                c["againstTx"].as_str(),
            )
        })
        .collect();
    assert_eq!(
        all,
        [("e1", "REMOVED", Some("t1")), ("e2", "REMOVED", Some("t2"))]
    );
    // The 4A detail fields come from the first conflict.
    assert_eq!(err.details["txId"], "e1");
    assert_eq!(rows(&s), before, "nothing written");
}

#[test]
fn skip_conflicting_merges_rest_and_reoffers_skipped() {
    let e = env();
    let mut s = project(&e);
    edit(
        &mut s,
        "br1",
        "e1",
        "alice",
        vec![trim("c3", -2), insert("$p", 28, 2)],
    );
    edit(&mut s, "br1", "e2", "alice", vec![trim("c4", -1)]);
    edit(&mut s, "br1", "e3", "alice", vec![delete("c1")]);
    edit(&mut s, MAIN, "t1", "bob", vec![delete("c3")]);
    let r = s
        .merge(MergeReq {
            skip_conflicting: true,
            ..req("m", "br1", MAIN)
        })
        .unwrap();
    assert_eq!(r.merged, ["e3"]);
    let skipped: Vec<(&str, &str)> = r
        .skipped
        .iter()
        .map(|c| (c.tx_id.as_str(), c.reason.as_str()))
        .collect();
    assert_eq!(skipped, [("e1", "REMOVED"), ("e2", "DEPENDS_ON_SKIPPED")]);
    assert_eq!(starts(&state(&s, MAIN)), at(&[("c2", 0)]));
    // A later merge offers the skipped records again (and only them).
    let err = code(s.merge(req("m2", "br1", MAIN)).unwrap_err());
    let again: Vec<&str> = err.details["conflicts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["txId"].as_str().unwrap())
        .collect();
    assert_eq!(again, ["e1", "e2"]);
}

#[test]
fn merge_records_share_merge_id_and_txids_are_derived() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "bob", vec![insert("$q", 32, 2)]);
    let r = merge(&mut s, "m", "br1", MAIN);
    assert_eq!(r.merge_id, "m");
    assert_eq!(tx_ids(&r.committed), ["m-1", "m-2"]);
    for (c, origin) in r.committed.iter().zip(["e1", "e2"]) {
        assert_eq!(
            c.link.as_deref(),
            Some(&Link::Merge {
                merge_id: "m".into(),
                from: "br1".into(),
                origin: origin.into(),
            })
        );
        assert_eq!(
            (c.tx.author.as_str(), c.tx.branch.as_str()),
            ("carol", MAIN)
        );
    }
    assert_eq!(s.ops_since(MAIN, 3).unwrap(), r.committed);
    assert!(rows(&s).contains(&"m-2:merge:m".to_string()));
    // The group is one unit of the merging author on the target.
    let st = s.undo_stacks(MAIN, "carol").unwrap();
    assert_eq!(st.undo.len(), 1);
    assert_eq!(st.undo[0].records, ["m-1", "m-2"]);
}

#[test]
fn merge_retry_same_merge_id_is_idempotent() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    let h = |c: char| Some(format!("blake3:{}", c.to_string().repeat(64)));
    let first = s
        .merge(MergeReq {
            args_hash: h('a'),
            ..req("m", "br1", MAIN)
        })
        .unwrap();
    assert_eq!(first.committed[0].tx.args_hash, h('a'));
    let before = rows(&s);
    // The retry answers from the log, also as a dry run, and writes nothing.
    for dry_run in [false, true] {
        let again = s
            .merge(MergeReq {
                dry_run,
                args_hash: h('a'),
                ..req("m", "br1", MAIN)
            })
            .unwrap();
        assert_eq!(again, first);
    }
    assert_eq!(rows(&s), before);
    let err = code(
        s.merge(MergeReq {
            args_hash: h('b'),
            ..req("m", "br1", MAIN)
        })
        .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    // A new mergeId with nothing left merges nothing.
    let none = merge(&mut s, "m2", "br1", MAIN);
    assert!(none.committed.is_empty() && none.merged.is_empty());
    assert_eq!(rows(&s), before);
    // A mergeId that is a record's txId, or whose `<id>-1` is, is refused.
    for key in ["e1", "b1"] {
        let err = code(s.merge(req(key, "br1", MAIN)).unwrap_err());
        assert_eq!(err.code, "IDEM_KEY_REUSED", "{key}");
    }
    edit(&mut s, "br1", "x-1", "alice", vec![insert("$q", 32, 1)]);
    edit(&mut s, "br1", "y-2", "alice", vec![insert("$r", 33, 1)]);
    let err = code(s.merge(req("x", "br1", MAIN)).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED", "x-1 holds an edit");
    let err = code(s.merge(req("y", "br1", MAIN)).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED", "y-2 would be written");
    assert_eq!(err.details["txId"], "y-2");
    assert_eq!(
        code(s.merge(req(" ", "br1", MAIN)).unwrap_err()).code,
        "INVALID_OP"
    );
    assert_eq!(
        code(s.merge(req("z", "br1", "br1")).unwrap_err()).code,
        "INVALID_OP"
    );
    assert_eq!(
        code(s.merge(req("z", "nope", MAIN)).unwrap_err()).code,
        "BRANCH_NOT_FOUND"
    );
}

#[test]
fn merge_is_all_or_nothing_on_crash() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![insert("$q", 32, 2)]);
    edit(&mut s, "br1", "e3", "alice", vec![delete("c1")]);
    let before = rows(&s);
    let main_before = state(&s, MAIN);
    s.fail_next_commit_after_apply();
    let err = code(s.merge(req("m", "br1", MAIN)).unwrap_err());
    assert_eq!(err.code, "TEST_FAULT");
    assert_eq!(rows(&s), before, "no prefix of the merge is kept");
    assert_eq!(state(&s, MAIN), main_before);
    // A reopened store sees nothing either, and the retry merges all three.
    drop(s);
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(rows(&s), before);
    let r = merge(&mut s, "m", "br1", MAIN);
    assert_eq!(r.merged, ["e1", "e2", "e3"]);
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
}

#[test]
fn update_branch_from_main() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    edit(&mut s, "br1", "e1", "alice", vec![trim("c3", -2)]);
    let r = merge(&mut s, "m", MAIN, "feature");
    assert_eq!(r.merged, ["t1"]);
    assert_eq!(r.committed[0].tx.branch, "br1");
    assert_eq!(
        starts(&state(&s, "br1")),
        at(&[("c4", 0), ("c1", 5), ("c2", 15), ("c3", 25)])
    );
    // A merge into a descendant (br1 is main's child) drops nothing: t1 stays undoable.
    assert_eq!(s.undo_stacks(MAIN, "bob").unwrap().undo[0].id, "t1");
}

#[test]
fn merge_both_directions() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    edit(&mut s, "br1", "e1", "alice", vec![trim("c3", -2)]);
    merge(&mut s, "m1", MAIN, "br1");
    edit(&mut s, "br1", "e2", "alice", vec![insert("$p", 33, 1)]);
    let r = merge(&mut s, "m2", "br1", MAIN);
    assert_eq!(r.merged, ["e1", "e2"], "the copy of t1 is not merged back");
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
    // Each record is applied once on each branch: one clip each.
    let count = |p: &Project| p.timelines[0].tracks[0].items.len();
    assert_eq!(count(&state(&s, MAIN)), 5);
    // Nothing is left either way.
    assert!(merge(&mut s, "m3", "br1", MAIN).committed.is_empty());
    assert!(merge(&mut s, "m4", MAIN, "br1").committed.is_empty());
    let v = s.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn merge_then_undo_of_the_merge_unit() {
    let e = env();
    let mut s = project(&e);
    let before = state(&s, MAIN);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, "br1", "e2", "alice", vec![delete("c1")]);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 30, 1)]);
    let after_t1 = state(&s, MAIN);
    merge(&mut s, "m", "br1", MAIN);
    let u = s
        .undo(HistoryReq {
            tx_id: "u1".into(),
            author: "carol".into(),
            branch: MAIN.into(),
            dry_run: false,
            args_hash: None,
            to: None,
            after_seq: None,
        })
        .unwrap();
    assert_eq!(u.undone, ["m"]);
    assert_eq!(state(&s, MAIN), after_t1, "the whole group undone, t1 kept");
    assert_ne!(state(&s, MAIN), before);
    // Alice's br1 stack was dropped by the merge into main (its parent).
    assert!(s.undo_stacks("br1", "alice").unwrap().undo.is_empty());
}

#[test]
fn merge_dry_run_on_follower_plans() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut s, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    let before = rows(&s);
    let dry = |r: MergeReq| MergeReq { dry_run: true, ..r };
    let planned = s.merge(dry(req("m", "br1", MAIN))).unwrap();
    assert!(planned.dry_run);
    assert_eq!(planned.merged, ["e1"]);
    assert_eq!(rows(&s), before);
    let mut f = Store::open(&e.data, &e.dir).unwrap();
    assert!(f.read_only().is_some());
    let on_f = f.merge(dry(req("m", "br1", MAIN))).unwrap();
    assert_eq!(on_f.committed, planned.committed);
    assert_eq!(rows(&f), before);
    assert_eq!(
        code(f.merge(req("m", "br1", MAIN)).unwrap_err()).code,
        "PROJECT_READ_ONLY"
    );
    let real = merge(&mut s, "m", "br1", MAIN);
    assert_eq!(real.committed, planned.committed);
    assert!(!real.dry_run);
}

#[test]
fn follower_applies_partial_merge_prefix() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    setup(&mut a);
    branch(&mut a);
    edit(&mut a, "br1", "e1", "alice", vec![insert("$p", 30, 2)]);
    edit(&mut a, "br1", "e2", "alice", vec![insert("$q", 32, 2)]);
    edit(&mut a, "br1", "e3", "alice", vec![delete("c1")]);
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.head(MAIN), a.head(MAIN));
    let pre = a.head(MAIN);
    let pre_state = state(&a, MAIN);

    // Two records per segment: the three merge records span two segments.
    fin_store::segment::set_max_records_for_test(2);
    let r = merge(&mut a, "m", "br1", MAIN);
    assert_eq!(r.committed.len(), 3);
    e.flush(&mut a);
    let segs = is::segments(&e.dir);
    let (last_path, last_name, _) = segs.last().unwrap();
    assert_eq!(last_name.first_seq(), pre + 3, "the third record alone");
    let held = e.root.join("held.fseg");
    fs::rename(last_path, &held).unwrap();

    // The follower applies the prefix it has: two of the three copies.
    e.poll(&mut b);
    assert_eq!(b.head(MAIN), pre + 2);
    let mut expect = pre_state;
    for c in &r.committed[..2] {
        expect = apply_committed(&expect, &c.tx, &c.bindings)
            .unwrap()
            .project;
    }
    assert_eq!(state(&b, MAIN), expect);
    assert_eq!(b.ops_since(MAIN, pre).unwrap(), r.committed[..2]);

    // The rest arrives: the same state as the leader's.
    fs::rename(&held, last_path).unwrap();
    e.poll(&mut b);
    assert_eq!(b.head(MAIN), a.head(MAIN));
    assert_eq!(state(&b, MAIN), state(&a, MAIN));
    let v = b.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn merges_replay_identically_on_follower() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    setup(&mut a);
    branch(&mut a);
    edit(&mut a, MAIN, "t1", "bob", vec![insert("$w", 0, 5)]);
    edit(&mut a, "br1", "e1", "alice", vec![trim("c3", -2)]);
    merge(&mut a, "m1", MAIN, "br1");
    edit(&mut a, "br1", "e2", "alice", vec![insert("$p", 33, 1)]);
    merge(&mut a, "m2", "br1", MAIN);
    e.flush(&mut a);
    e.poll(&mut b);
    for br in [MAIN, "br1"] {
        assert_eq!(b.head(br), a.head(br), "{br}");
        assert_eq!(state(&b, br), state(&a, br), "{br}");
        assert_eq!(
            b.ops_since(br, 0).unwrap(),
            a.ops_since(br, 0).unwrap(),
            "{br}"
        );
    }
    assert_eq!(
        b.undo_stacks(MAIN, "carol").unwrap(),
        a.undo_stacks(MAIN, "carol").unwrap()
    );
    // A follower's dry run of another merge plans what the leader commits.
    edit(&mut a, "br1", "e3", "alice", vec![delete("c1")]);
    e.flush(&mut a);
    e.poll(&mut b);
    let planned = b
        .merge(MergeReq {
            dry_run: true,
            ..req("m3", "br1", MAIN)
        })
        .unwrap();
    let real = merge(&mut a, "m3", "br1", MAIN);
    assert_eq!(planned.committed, real.committed);
    let v = b.verify();
    assert!(v.ok, "{v:?}");
}

fn new_branch(s: &mut Store, key: &str, name: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: key.into(),
        author: "alice".into(),
        name: Some(name.into()),
        from: MAIN.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn branch_state(s: &Store, id: &str) -> BranchState {
    s.branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap()
        .state
}

#[test]
fn transitive_merge_and_merged_state() {
    // br2 → br1 → main brings br2's clip and br1's trim of it (Decision 34); both branches
    // are merged then, br2 through br1.
    let e = env();
    let mut s = project(&e);
    let br2 = new_branch(&mut s, "b2", "other");
    edit(&mut s, &br2, "e1", "bob", vec![insert("$p", 30, 5)]);
    merge(&mut s, "ma", &br2, "br1");
    assert_eq!(branch_state(&s, &br2), BranchState::Merged);
    edit(&mut s, "br1", "e2", "alice", vec![trim("c4", -1)]);
    let r = merge(&mut s, "mb", "br1", MAIN);
    assert_eq!(r.merged, ["ma-1", "e2"]);
    assert_eq!(r.source_state, BranchState::Merged);
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
    assert_eq!(branch_state(&s, &br2), BranchState::Merged);
    // A merge into br1 gives it something to merge on: active again, merged once it does.
    let br3 = new_branch(&mut s, "b3", "third");
    edit(&mut s, &br3, "e3", "bob", vec![insert("$q", 0, 1)]);
    merge(&mut s, "mc", &br3, "br1");
    assert_eq!(branch_state(&s, "br1"), BranchState::Active);
    merge(&mut s, "md", "br1", MAIN);
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    assert_eq!(state(&s, MAIN), state(&s, "br1"));
    // An update from main into a merged branch copies nothing that is left to merge back.
    edit(&mut s, MAIN, "t1", "carol", vec![delete("c1")]);
    merge(&mut s, "me", MAIN, "br1");
    assert_eq!(branch_state(&s, "br1"), BranchState::Merged);
    let v = s.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn txid_equal_to_a_merge_id_is_refused() {
    let e = env();
    let mut s = project(&e);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 1)]);
    merge(&mut s, "m", "br1", MAIN);
    let before = rows(&s);
    let base = s.head(MAIN);
    let err = code(
        s.commit(Tx {
            tx_id: "m".into(),
            author: "carol".into(),
            branch: MAIN.into(),
            base_rev: base,
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![insert("$q", 0, 1)],
        })
        .unwrap_err(),
    );
    assert_eq!(
        (err.code.as_str(), err.details["mergeId"].as_str()),
        ("IDEM_KEY_REUSED", Some("m"))
    );
    let hist = HistoryReq {
        tx_id: "m".into(),
        author: "carol".into(),
        branch: MAIN.into(),
        dry_run: false,
        args_hash: None,
        to: None,
        after_seq: None,
    };
    assert_eq!(
        code(s.undo(hist.clone()).unwrap_err()).code,
        "IDEM_KEY_REUSED"
    );
    assert_eq!(code(s.redo(hist).unwrap_err()).code, "IDEM_KEY_REUSED");
    let err = code(
        s.create_branch(BranchCreate {
            tx_id: "m".into(),
            author: "alice".into(),
            name: Some("x".into()),
            from: MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    assert_eq!(rows(&s), before);
    // The merge unit stays one unit: undo takes the group.
    let u = s
        .undo(HistoryReq {
            tx_id: "u1".into(),
            author: "carol".into(),
            branch: MAIN.into(),
            dry_run: false,
            args_hash: None,
            to: None,
            after_seq: None,
        })
        .unwrap();
    assert_eq!(u.undone, ["m"]);
}

#[test]
fn empty_merge_leaves_no_trace() {
    // Nothing to merge writes nothing, so the same key merges what arrives later.
    let e = env();
    let mut s = project(&e);
    let before = rows(&s);
    assert!(merge(&mut s, "k", "br1", MAIN).committed.is_empty());
    assert_eq!(rows(&s), before);
    edit(&mut s, "br1", "e1", "alice", vec![insert("$p", 30, 1)]);
    assert_eq!(merge(&mut s, "k", "br1", MAIN).merged, ["e1"]);
}
