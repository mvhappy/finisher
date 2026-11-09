//! Durable rejections through the store (sub-project 5 spec §8.3; plan 5E Task 2): `Store::drop`
//! validates what it names, is idempotent and guarded; the `drops` index is maintained by every
//! write and import and rebuilt from the log; the write-time `merged` state agrees with the
//! log's; a rejection survives a move and a recovery; and, as properties, no merge ever copies a
//! dropped origin, and a writer and a follower that imported in another order agree.

mod integration_support;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use fin_core::branch::BranchState;
use fin_core::{CommittedTx, CoreError, Link, Op, SCHEMA_VERSION, Tx};
use fin_store::{
    BranchCreate, BranchSet, CherryPickReq, DropReq, MAIN, MergeReport, MergeReq, RecoverReq,
    RecoverSel, Store, StoreError, WriteGuard, WriteOutcome,
};
use integration_support::diverge::{Split, all_segments, split, winner};
use integration_support::{self as is, Env, asset, insert};
use proptest::prelude::*;
use rusqlite::Connection;

fn edit(s: &mut Store, branch: &str, id: &str, ops: Vec<Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: "alice".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
}

fn new_branch(s: &mut Store, key: &str, name: &str, from: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: key.into(),
        author: "alice".into(),
        name: Some(name.into()),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn req(id: &str, from: &str, into: &str, drops: &[&str], undrops: &[&str]) -> DropReq {
    let ids = |l: &[&str]| l.iter().map(|s| s.to_string()).collect();
    DropReq {
        tx_id: id.into(),
        author: "rev".into(),
        into: into.into(),
        from: from.into(),
        drops: ids(drops),
        undrops: ids(undrops),
        dry_run: false,
        args_hash: None,
    }
}

fn merge_req(id: &str, from: &str, into: &str) -> MergeReq {
    MergeReq {
        tx_id: id.into(),
        author: "carol".into(),
        from: from.into(),
        into: into.into(),
        skip_conflicting: true,
        dry_run: false,
        args_hash: None,
    }
}

fn merge(s: &mut Store, id: &str, from: &str, into: &str) -> MergeReport {
    s.merge(merge_req(id, from, into)).unwrap()
}

fn code<T: std::fmt::Debug>(r: Result<T, StoreError>) -> CoreError {
    r.unwrap_err().into()
}

fn dropped(s: &Store, branch: &str) -> Vec<String> {
    s.dropped(branch).unwrap().into_iter().collect()
}

fn state(s: &Store, id: &str) -> BranchState {
    s.branches(true)
        .unwrap()
        .into_iter()
        .find(|b| b.id == id)
        .unwrap()
        .state
}

fn assert_verifies(s: &Store) {
    let v = s.verify();
    assert!(v.ok, "{v:#?}");
}

/// The `drops` rows, in order.
fn index_rows(s: &Store) -> Vec<(String, String, i64, String, bool)> {
    let c = Connection::open(s.db_path()).unwrap();
    let mut stmt = c
        .prepare(
            "SELECT tx_id, branch, seq, origin, dropped FROM drops ORDER BY branch, seq, origin",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn tx_count(s: &Store) -> i64 {
    let c = Connection::open(s.db_path()).unwrap();
    c.query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))
        .unwrap()
}

/// Main at rev 3 (`t0` an asset, `m0` and `m1` clips), `br1` ("feature") forked there with
/// `e1`, `e2`, `e3`; then `m2` on main.
fn setup(s: &mut Store) {
    edit(s, MAIN, "t0", vec![asset("/m/a.mov")]);
    edit(s, MAIN, "m0", vec![insert(0, "a1")]);
    edit(s, MAIN, "m1", vec![insert(0, "a1")]);
    new_branch(s, "b1", "feature", MAIN);
    for id in ["e1", "e2", "e3"] {
        edit(s, "br1", id, vec![insert(0, "a1")]);
    }
    edit(s, MAIN, "m2", vec![insert(0, "a1")]);
}

fn writer(e: &Env) -> Store {
    let mut s = e.create("a");
    setup(&mut s);
    s
}

#[test]
fn drop_validates_origins_on_source() {
    let e = is::env();
    let mut s = writer(&e);
    let before = tx_count(&s);
    for (bad, from) in [
        ("m2", "br1"), // the target's own record
        ("m1", "br1"), // common history
        ("zz", "br1"), // unknown
        ("e1", MAIN),  // a branch into itself
    ] {
        let into = MAIN;
        let err = code(s.drop(req("d", from, into, &[bad], &[])));
        assert_eq!(err.code, "INVALID_OP", "{bad}: {err:?}");
        assert_eq!(err.details["reason"], "notOnSource", "{bad}");
        assert_eq!(err.details["txId"], bad);
    }
    // An undrop is checked too; so is every id of a list.
    let err = code(s.drop(req("d", "br1", MAIN, &["e1"], &["m2"])));
    assert_eq!(
        (err.code.as_str(), &err.details["txId"]),
        ("INVALID_OP", &"m2".into())
    );
    // The lists' own rules (Link::new_drop).
    assert_eq!(
        code(s.drop(req("d", "br1", MAIN, &[], &[]))).code,
        "INVALID_OP"
    );
    let both = code(s.drop(req("d", "br1", MAIN, &["e1"], &["e1"])));
    assert_eq!(both.details["txId"], "e1");
    assert_eq!(
        code(s.drop(req("d", "nope", MAIN, &["e1"], &[]))).code,
        "BRANCH_NOT_FOUND"
    );
    assert_eq!(tx_count(&s), before, "nothing written");

    let c = s
        .drop(req("d", "br1", MAIN, &["e2", "e1", "e2"], &[]))
        .unwrap();
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::Drop {
            from: "br1".into(),
            drops: vec!["e1".into(), "e2".into()],
            undrops: vec![],
            recovers: None,
        })
    );
    assert!(c.tx.ops.is_empty());
    assert_eq!((c.tx.branch.as_str(), c.tx.author.as_str()), (MAIN, "rev"));
    assert_eq!(dropped(&s, MAIN), ["e1", "e2"]);
    assert!(dropped(&s, "br1").is_empty(), "a drop binds its own branch");

    // A copy's txId is not a root; its root is, on a branch that brought it.
    let br2 = new_branch(&mut s, "b2", "side", MAIN);
    merge(&mut s, "mg", "br1", &br2);
    let err = code(s.drop(req("d2", &br2, MAIN, &["mg-3"], &[])));
    assert_eq!(err.details["reason"], "notOnSource");
    s.drop(req("d2", &br2, MAIN, &["e3"], &[])).unwrap();
    assert_eq!(dropped(&s, MAIN), ["e1", "e2", "e3"]);
    // A plain merge leaves them out: DROPPED, not a conflict.
    let r = merge(&mut s, "m", "br1", MAIN);
    assert!(r.merged.is_empty(), "{r:?}");
    let excluded: Vec<(&str, &str)> = r
        .excluded
        .iter()
        .map(|c| (c.tx_id.as_str(), c.reason.as_str()))
        .collect();
    assert_eq!(
        excluded,
        [("e1", "DROPPED"), ("e2", "DROPPED"), ("e3", "DROPPED")]
    );
    assert_verifies(&s);
}

/// Decision 15: a rejection is of an entry not yet merged. An origin `into` already holds (it
/// originates there, or a merge or pick copied it there) is no drop: `notOnSource` when it
/// originates on `into`, `alreadyPresent` when it was copied there. An undrop names an origin
/// dropped for `into` now (`notDropped` otherwise), whatever became of it on the source since.
#[test]
fn drop_only_of_unmerged_origins() {
    let e = is::env();
    let mut s = writer(&e);
    s.drop(req("d1", "br1", MAIN, &["e2"], &[])).unwrap();
    let not_dropped = code(s.drop(req("d", "br1", MAIN, &[], &["e1"])));
    assert_eq!(
        (not_dropped.code.as_str(), &not_dropped.details["reason"]),
        ("INVALID_OP", &"notDropped".into())
    );
    assert_eq!(not_dropped.details["txId"], "e1");
    // main merged into br1: its copy of m2 is on br1's path, but m2 is main's own.
    merge(&mut s, "mx", MAIN, "br1");
    let own = code(s.drop(req("d", "br1", MAIN, &["m2"], &[])));
    assert_eq!(
        (
            own.code.as_str(),
            &own.details["reason"],
            &own.details["txId"]
        ),
        ("INVALID_OP", &"notOnSource".into(), &"m2".into())
    );
    // br1 merged into main: e1 and e3 are there now.
    assert_eq!(merge(&mut s, "mA", "br1", MAIN).merged, ["e1", "e3"]);
    for (id, from) in [("e1", "br1"), ("e3", "br1")] {
        let err = code(s.drop(req("d", from, MAIN, &[id], &[])));
        assert_eq!(
            (
                err.code.as_str(),
                &err.details["reason"],
                &err.details["txId"]
            ),
            ("INVALID_OP", &"alreadyPresent".into(), &id.into()),
            "{id}"
        );
    }
    // Through a child of br1 that also holds e1: the root is what counts.
    let br2 = new_branch(&mut s, "b2", "side", "br1");
    let err = code(s.drop(req("d", &br2, MAIN, &["e1"], &[])));
    assert_eq!(err.details["reason"], "alreadyPresent");
    // e2, still dropped, is picked into main and br1 is archived: the undrop stays possible.
    s.cherry_pick(CherryPickReq {
        tx_id: "p1".into(),
        author: "rev".into(),
        origin: "e2".into(),
        into: MAIN.into(),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    s.set_branch(BranchSet {
        tx_id: "a1".into(),
        author: "rev".into(),
        branch: "br1".into(),
        name: None,
        archived: Some(true),
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
    let again = code(s.drop(req("d", "br1", MAIN, &["e2"], &[])));
    assert_eq!(again.details["reason"], "alreadyPresent");
    s.drop(req("d2", "br1", MAIN, &[], &["e2"])).unwrap();
    assert!(dropped(&s, MAIN).is_empty());
    let twice = code(s.drop(req("d3", "br1", MAIN, &[], &["e2"])));
    assert_eq!(twice.details["reason"], "notDropped");
    assert_verifies(&s);
}

#[test]
fn drop_is_idempotent() {
    let e = is::env();
    let mut s = writer(&e);
    let before = (tx_count(&s), s.head(MAIN));
    let planned = s
        .drop(DropReq {
            dry_run: true,
            ..req("d1", "br1", MAIN, &["e1"], &[])
        })
        .unwrap();
    assert_eq!(s.take_write_outcome(), Some(WriteOutcome::Planned));
    assert_eq!(
        (tx_count(&s), s.head(MAIN)),
        before,
        "a dry run keeps nothing"
    );
    assert!(dropped(&s, MAIN).is_empty() && index_rows(&s).is_empty());

    let hash = format!("blake3:{}", "ab".repeat(32));
    let first = s
        .drop(DropReq {
            args_hash: Some(hash.clone()),
            ..req("d1", "br1", MAIN, &["e1"], &[])
        })
        .unwrap();
    assert_eq!(first.link, planned.link);
    assert_eq!(first.rev, planned.rev);
    let again = s
        .drop(DropReq {
            args_hash: Some(hash),
            ..req("d1", "br1", MAIN, &["e1"], &[])
        })
        .unwrap();
    assert_eq!(s.take_write_outcome(), Some(WriteOutcome::Deduped));
    assert_eq!(again, first);
    assert_eq!(s.head(MAIN), first.rev, "one record");
    let other = format!("blake3:{}", "cd".repeat(32));
    let err = code(s.drop(DropReq {
        args_hash: Some(other),
        ..req("d1", "br1", MAIN, &["e1"], &[])
    }));
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    // The txId of another kind of record.
    assert_eq!(
        code(s.drop(req("e2", "br1", MAIN, &["e1"], &[]))).code,
        "IDEM_KEY_REUSED"
    );
    // A merge group's id.
    let br2 = new_branch(&mut s, "b2", "side", MAIN);
    merge(&mut s, "mg", "br1", &br2);
    assert_eq!(
        code(s.drop(req("mg", "br1", MAIN, &["e2"], &[]))).code,
        "IDEM_KEY_REUSED"
    );

    // Undropped, it is a candidate again.
    s.drop(req("d2", "br1", MAIN, &[], &["e1"])).unwrap();
    assert!(dropped(&s, MAIN).is_empty());
    assert_eq!(merge(&mut s, "m", "br1", MAIN).merged, ["e1", "e2", "e3"]);
    // A drop is no unit on the undo stacks.
    let stacks = s.undo_stacks(MAIN, "rev").unwrap();
    assert!(stacks.undo.is_empty(), "{stacks:?}");
    assert_verifies(&s);
}

fn guard(allow: Option<&[&str]>, deny: &[&str]) -> WriteGuard {
    WriteGuard {
        deny_branches: deny.iter().map(|s| s.to_string()).collect(),
        allow_branches: allow.map(|a| a.iter().map(|s| s.to_string()).collect()),
        credential: None,
    }
}

#[test]
fn drop_respects_guard() {
    let e = is::env();
    let mut s = writer(&e);
    let before = tx_count(&s);
    let r = s.with_guard(guard(Some(&["br1"]), &[]), |s| {
        s.drop(req("d", "br1", MAIN, &["e1"], &[]))
    });
    assert_eq!(code(r).code, "BRANCH_NOT_ALLOWED");
    let r = s.with_guard(guard(None, &[MAIN]), |s| {
        s.drop(req("d", "br1", MAIN, &["e1"], &[]))
    });
    assert_eq!(code(r).code, "BRANCH_PROTECTED");
    // A dry run is checked too.
    let r = s.with_guard(guard(Some(&["br1"]), &[]), |s| {
        s.drop(DropReq {
            dry_run: true,
            ..req("d", "br1", MAIN, &["e1"], &[])
        })
    });
    assert_eq!(code(r).code, "BRANCH_NOT_ALLOWED");
    assert_eq!(tx_count(&s), before);
    // The source is only read: not a target of the guard.
    s.with_guard(guard(Some(&[MAIN]), &["br1"]), |s| {
        s.drop(req("d", "br1", MAIN, &["e1"], &[]))
    })
    .unwrap();
    assert_eq!(dropped(&s, MAIN), ["e1"]);
}

#[test]
fn index_rebuilt_equals_maintained() {
    let e = is::env();
    let mut s = writer(&e);
    s.drop(req("d1", "br1", MAIN, &["e1", "e2"], &[])).unwrap();
    let br2 = new_branch(&mut s, "b2", "child", "br1");
    edit(&mut s, &br2, "x1", vec![insert(0, "a1")]);
    s.drop(req("d2", &br2, "br1", &["x1"], &[])).unwrap();
    s.drop(req("d3", "br1", MAIN, &["e3"], &["e1"])).unwrap();
    let rows = index_rows(&s);
    assert_eq!(rows.len(), 5, "{rows:?}");
    assert_eq!(dropped(&s, MAIN), ["e2", "e3"]);
    assert_eq!(dropped(&s, "br1"), ["x1"]);
    assert_verifies(&s);
    e.flush(&mut s);

    // A follower's index, maintained by import, is the writer's.
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(index_rows(&f), rows);
    assert_eq!(dropped(&f, MAIN), ["e2", "e3"]);
    assert_verifies(&f);

    // Verify sees a damaged index; a rebuild derives it from the log.
    let c = Connection::open(s.db_path()).unwrap();
    c.execute("DELETE FROM drops WHERE tx_id = 'd2'", [])
        .unwrap();
    c.execute("INSERT INTO drops VALUES ('zz', 'main', 1, 'm0', 1)", [])
        .unwrap();
    drop(c);
    let v = s.verify();
    assert!(!v.ok);
    let text = serde_json::to_string(&v).unwrap();
    assert!(text.contains("\"drops\":[{"), "{text}");
    s.rebuild().unwrap();
    assert_eq!(index_rows(&s), rows);
    assert_verifies(&s);
}

#[test]
fn write_merged_state_equals_compute_with_drops() {
    let e = is::env();
    let mut s = writer(&e);
    assert_eq!(merge(&mut s, "mA", "br1", MAIN).merged, ["e1", "e2", "e3"]);
    assert_eq!(state(&s, "br1"), BranchState::Merged);
    edit(&mut s, "br1", "e4", vec![insert(0, "a1")]);
    assert_eq!(state(&s, "br1"), BranchState::Active);
    // What is left (e4) is dropped: merged at write time, as the log says.
    s.drop(req("d1", "br1", MAIN, &["e4"], &[])).unwrap();
    assert_eq!(state(&s, "br1"), BranchState::Merged);
    assert_verifies(&s);
    s.recompute_derived_for_test().unwrap();
    assert_eq!(state(&s, "br1"), BranchState::Merged);
    // A child of br1 forked now holds e4 too: merged into main by the same drop.
    let br2 = new_branch(&mut s, "b2", "child", "br1");
    assert_eq!(state(&s, &br2), BranchState::Active, "never merged");
    // Undropped, br1 has something to merge again.
    s.drop(req("d2", "br1", MAIN, &[], &["e4"])).unwrap();
    assert_eq!(state(&s, "br1"), BranchState::Active);
    assert_verifies(&s);
    // Dropped for main by a drop from another branch that holds a copy of e4 (br1 merged
    // into br3): the drop names e4's root.
    let br3 = new_branch(&mut s, "b3", "side", MAIN);
    merge(&mut s, "mB", "br1", &br3);
    s.drop(req("d3", &br3, MAIN, &["e4"], &[])).unwrap();
    assert_eq!(state(&s, "br1"), BranchState::Merged);
    assert_verifies(&s);
    e.flush(&mut s);
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(f.branches(true).unwrap(), s.branches(true).unwrap());
    assert_verifies(&f);
}

/// Copies `from` to `to` without the lease files: the folder as another server finds it.
fn copy_folder(from: &Path, to: &Path) {
    is::copy_dir(from, to);
    for e in fs::read_dir(to).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("lease") {
            fs::remove_file(e.path()).unwrap();
        }
    }
    let dot = to.join(".finisher");
    for e in fs::read_dir(&dot).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with("lease") {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[test]
fn merge_after_move_keeps_drop() {
    let e = is::env();
    let mut a = writer(&e);
    a.drop(req("d1", "br1", MAIN, &["e2"], &[])).unwrap();
    e.flush(&mut a);
    drop(a);
    // The project moves: its folder, without the old server's lease, on a fresh server data.
    let moved = e.root.join("moved");
    copy_folder(&e.dir, &moved);
    let mut b = e.try_open_at("b", &moved).unwrap();
    integration_support::diverge::poll_until_held(&e, &mut b, 80);
    e.advance(10);
    e.poll(&mut b);
    assert_eq!(dropped(&b, MAIN), ["e2"]);
    let r = merge(&mut b, "m", "br1", MAIN);
    assert_eq!(r.merged, ["e1", "e3"]);
    assert_eq!(r.excluded[0].tx_id, "e2");
    assert_eq!(r.excluded[0].reason, "DROPPED");
    assert_verifies(&b);
}

/// A rejection on a line that lost to another server (a diverged tail) does not vanish with
/// it: recovery makes it again on the branch it binds, standing for the quarantined row.
#[test]
fn drop_in_diverged_tail_is_made_again_on_its_branch() {
    let e = is::env();
    let before = |a: &mut Store| {
        new_branch(a, "b1", "feature", MAIN);
        edit(a, "br1", "e1", vec![insert(0, "a1")]);
        edit(a, "br1", "e2", vec![insert(0, "a1")]);
    };
    let mut s = split(&e, before, |_| {}, winner);
    s.a.drop(req("xd", "br1", MAIN, &["e1"], &[])).unwrap();
    edit(&mut s.a, MAIN, "x1", vec![insert(1, "a1")]);
    s.a.set_test_lease(1, false);
    e.poll(&mut s.a);
    s.a.set_test_lease(3, true);
    let Split { mut a, mut f, .. } = s;
    assert!(dropped(&a, MAIN).is_empty(), "the drop went to quarantine");
    let groups = a.recover_groups().unwrap();
    assert_eq!(groups.len(), 1, "{groups:#?}");
    assert!(groups[0].recoverable, "{groups:#?}");
    let r = a
        .recover(RecoverReq {
            tx_id: "rec".into(),
            author: "rec".into(),
            create: Some(RecoverSel::All),
            import_dir: None,
            dismiss: None,
            dry_run: false,
        })
        .unwrap();
    let made = &r.created[0];
    let again: Vec<&CommittedTx> = r
        .committed
        .iter()
        .filter(|c| matches!(c.link.as_deref(), Some(Link::Drop { .. })))
        .collect();
    assert_eq!(again.len(), 1, "{:#?}", r.committed);
    assert_eq!(again[0].tx.branch, MAIN, "made again where it binds");
    let Some(Link::Drop {
        from,
        drops,
        recovers,
        ..
    }) = again[0].link.as_deref()
    else {
        unreachable!()
    };
    assert_eq!(
        (from.as_str(), drops.as_slice()),
        ("br1", &["e1".to_string()][..])
    );
    assert!(recovers.is_some());
    assert_eq!(dropped(&a, MAIN), ["e1"]);
    assert!(dropped(&a, &made.id).is_empty());
    // Recovered: no longer listed; a retry answers from the log.
    assert!(a.recover_groups().unwrap().is_empty());
    assert_eq!(a.status().quarantine.count, 0);
    assert_verifies(&a);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(dropped(&f, MAIN), ["e1"]);
    assert_eq!(merge(&mut a, "m", "br1", MAIN).merged, ["e2"]);
    assert_verifies(&f);
}

// ---- Properties (Review Focus 1) ----

#[derive(Debug, Clone)]
enum Act {
    /// An insert on branch `b`.
    Edit { b: usize },
    /// A branch forked from branch `from` at its head.
    Branch { from: usize },
    /// A drop (`undo`: an undrop) on `into` of the `k`th edit made so far, from `from`.
    Drop {
        into: usize,
        from: usize,
        k: usize,
        undo: bool,
    },
    /// A merge from `from` into `into`, skipping conflicts.
    Merge { from: usize, into: usize },
    /// The writer exports and the follower imports.
    Sync,
}

fn act() -> impl Strategy<Value = Act> {
    prop_oneof![
        3 => (0..6usize).prop_map(|b| Act::Edit { b }),
        1 => (0..6usize).prop_map(|from| Act::Branch { from }),
        3 => (0..6usize, 0..6usize, 0..40usize, prop::bool::weighted(0.25))
            .prop_map(|(into, from, k, undo)| Act::Drop { into, from, k, undo }),
        3 => (0..6usize, 0..6usize).prop_map(|(from, into)| Act::Merge { from, into }),
        1 => Just(Act::Sync),
    ]
}

fn config(default: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default);
    ProptestConfig {
        cases,
        ..ProptestConfig::default()
    }
}

/// The test's own model of what is dropped (Decision 2), from the drops it made: each branch's
/// parent and fork seq, and its marks `(seq, origin, dropped)`.
#[derive(Default)]
struct Model {
    parent: BTreeMap<String, (String, u64)>,
    marks: BTreeMap<String, Vec<(u64, String, bool)>>,
    /// Every txId's origin root.
    roots: BTreeMap<String, String>,
}

impl Model {
    fn dropped(&self, branch: &str) -> BTreeSet<String> {
        // Own branch first, each ancestor up to the lowest fork below it.
        let mut chain = vec![(branch.to_string(), u64::MAX)];
        let (mut cur, mut bound) = (branch.to_string(), u64::MAX);
        while let Some((p, fork)) = self.parent.get(&cur) {
            bound = bound.min(*fork);
            chain.push((p.clone(), bound));
            cur = p.clone();
        }
        let mut last = BTreeMap::new();
        for (b, through) in chain.iter().rev() {
            for (seq, origin, d) in self.marks.get(b).into_iter().flatten() {
                if seq <= through {
                    last.insert(origin.clone(), *d);
                }
            }
        }
        last.into_iter()
            .filter(|(_, d)| *d)
            .map(|(o, _)| o)
            .collect()
    }
}

/// What a run leaves: the writer, its follower and the model.
struct Run {
    e: Env,
    a: Store,
    f: Store,
    branches: Vec<String>,
    model: Model,
}

fn root_of(model: &Model, c: &CommittedTx) -> String {
    match c.link.as_deref() {
        Some(Link::Merge { origin, .. } | Link::CherryPick { origin, .. }) => model
            .roots
            .get(origin)
            .cloned()
            .unwrap_or_else(|| origin.clone()),
        _ => c.tx.tx_id.clone(),
    }
}

fn run(acts: &[Act]) -> Result<Run, TestCaseError> {
    let e = is::env();
    let mut a = e.create("a");
    edit(&mut a, MAIN, "t0", vec![asset("/m/a.mov")]);
    let mut f = e.open("f");
    let mut branches = vec![MAIN.to_string()];
    let mut model = Model::default();
    let mut edits: Vec<String> = vec![];
    for (n, act) in acts.iter().enumerate() {
        let pick = |i: usize| branches[i % branches.len()].clone();
        match act {
            Act::Edit { b } => {
                let id = format!("e{n}");
                edit(&mut a, &pick(*b), &id, vec![insert(0, "a1")]);
                model.roots.insert(id.clone(), id.clone());
                edits.push(id);
            }
            Act::Branch { from } => {
                if branches.len() < 6 {
                    let parent = pick(*from);
                    let fork = a.head(&parent);
                    let id = new_branch(&mut a, &format!("b{n}"), &format!("n{n}"), &parent);
                    model.parent.insert(id.clone(), (parent, fork));
                    branches.push(id);
                }
            }
            Act::Drop {
                into,
                from,
                k,
                undo,
            } => {
                let (into, from) = (pick(*into), pick(*from));
                let Some(id) = (!edits.is_empty()).then(|| edits[k % edits.len()].as_str()) else {
                    continue;
                };
                let (d, u): (&[&str], &[&str]) = if *undo { (&[], &[id]) } else { (&[id], &[]) };
                match a.drop(req(&format!("d{n}"), &from, &into, d, u)) {
                    Ok(c) => {
                        model.marks.entry(into.clone()).or_default().push((
                            c.rev,
                            id.to_string(),
                            !undo,
                        ));
                    }
                    Err(err) => {
                        let err = CoreError::from(err);
                        let reason = err.details["reason"].as_str().unwrap_or_default();
                        prop_assert!(
                            ["notOnSource", "alreadyPresent", "notDropped"].contains(&reason),
                            "{:?}",
                            err
                        );
                    }
                }
            }
            Act::Merge { from, into } => {
                let (from, into) = (pick(*from), pick(*into));
                if from == into {
                    continue;
                }
                let dropped = model.dropped(&into);
                prop_assert_eq!(a.dropped(&into).unwrap(), dropped.clone());
                let r = a.merge(merge_req(&format!("m{n}"), &from, &into)).unwrap();
                for c in &r.committed {
                    let root = root_of(&model, c);
                    prop_assert!(!dropped.contains(&root), "{} copied into {}", root, into);
                    model.roots.insert(c.tx.tx_id.clone(), root);
                }
            }
            Act::Sync => {
                e.flush(&mut a);
                e.poll(&mut f);
            }
        }
    }
    Ok(Run {
        e,
        a,
        f,
        branches,
        model,
    })
}

/// Every segment file of `dir`, relative to its segments root.
fn segment_files(dir: &Path) -> Vec<PathBuf> {
    let root = dir.join(".finisher").join("segments");
    all_segments(dir)
        .into_iter()
        .map(|p| p.strip_prefix(&root).unwrap().to_path_buf())
        .collect()
}

fn shuffle<T>(v: &mut [T], mut seed: u64) {
    for i in (1..v.len()).rev() {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        v.swap(i, (seed >> 33) as usize % (i + 1));
    }
}

proptest! {
    #![proptest_config(config(64))]

    /// No merge, on the writer or planned on a follower, copies an origin dropped for its
    /// target, whatever the order of drops, undrops, merges into the target and its
    /// descendants, exports and imports; what is dropped is what the model says.
    #[test]
    fn prop_dropped_never_copied(acts in prop::collection::vec(act(), 1..24)) {
        let Run { e, mut a, mut f, branches, model } = run(&acts)?;
        e.flush(&mut a);
        e.poll(&mut f);
        for b in &branches {
            prop_assert_eq!(a.dropped(b).unwrap(), model.dropped(b), "{}", b);
            prop_assert_eq!(f.dropped(b).unwrap(), model.dropped(b), "{}", b);
        }
        for from in &branches {
            for into in branches.iter().filter(|i| *i != from) {
                let dry = MergeReq { dry_run: true, ..merge_req("plan", from, into) };
                let r = f.merge(dry).unwrap();
                for c in &r.committed {
                    let root = root_of(&model, c);
                    prop_assert!(!model.dropped(into).contains(&root), "{} into {}", root, into);
                }
            }
        }
        prop_assert!(a.verify().ok);
        prop_assert!(f.verify().ok);
    }

    /// A follower that receives the writer's segments in another order derives the same
    /// branch index (`merged` states included) and the same dropped origins, and verifies.
    #[test]
    fn prop_writer_and_follower_agree(
        acts in prop::collection::vec(act(), 1..20),
        seed in any::<u64>(),
    ) {
        let Run { e, mut a, branches, .. } = run(&acts)?;
        e.flush(&mut a);
        let mut files = segment_files(&e.dir);
        shuffle(&mut files, seed);
        let dir = e.root.join("order");
        is::copy_dir(&e.dir, &dir);
        for p in all_segments(&dir) {
            fs::remove_file(p).unwrap();
        }
        let root = e.dir.join(".finisher").join("segments");
        let mut x = e.try_open_at("x", &dir).unwrap();
        for rel in &files {
            let to = dir.join(".finisher").join("segments").join(rel);
            fs::create_dir_all(to.parent().unwrap()).unwrap();
            fs::copy(root.join(rel), to).unwrap();
            e.poll(&mut x);
        }
        prop_assert_eq!(x.branches(true).unwrap(), a.branches(true).unwrap());
        for b in &branches {
            prop_assert_eq!(x.dropped(b).unwrap(), a.dropped(b).unwrap(), "{}", b);
        }
        let v = x.verify();
        prop_assert!(v.ok, "{:?}", v);
        prop_assert!(a.verify().ok);
    }
}
