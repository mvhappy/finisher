//! Branches through the project folder (plan 4B Task 9): export writes parents before
//! children, import learns branches from the `branch` records it applies (a worklist, parents
//! first), child segments that arrive first wait, a set-aside `branch` record takes its
//! branch's segments with it (Decision 8), and the id counters are raised past imported and
//! quarantined ids (Decision 28).
//!
//! Two kinds of setup: servers on the real lease ([`integration_support`], a leader and a
//! follower on one folder), and servers whose DB is a copy of the creator's at rev 0 with the
//! test lease at a chosen epoch (as `tests/import.rs`), for epoch scenarios.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fin_core::branch::BranchState;
use fin_core::{CommittedTx, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::lease::MonoClock;
use fin_store::names::{SegmentName, hash12};
use fin_store::segment::{self, Header};
use fin_store::{
    At, BranchCreate, BranchCreated, BranchSet, FailPoint, MAIN, ReadOnlyReason, Store, StoreEvent,
    db_path,
};
use integration_support as is;
use rusqlite::Connection;
use tempfile::TempDir;

fn asset(path: &str) -> Op {
    Op::AssetAdd {
        id: "$a".into(),
        path: path.into(),
        duration: Rational::from_int(600),
        video: Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
        }),
        audio: None,
        fingerprint: "fp".into(),
    }
}

fn insert(at: i64) -> Op {
    insert_as("$c", at)
}

fn insert_as(id: &str, at: i64) -> Op {
    Op::ClipInsert {
        id: id.into(),
        track: "t1".into(),
        at: Rational::from_int(at),
        asset: "a1".into(),
        source_in: Rational::from_int(0),
        duration: Rational::from_int(1),
    }
}

fn commit_on(s: &mut Store, id: &str, branch: &str, ops: Vec<Op>) -> CommittedTx {
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: s.head(branch),
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    })
    .unwrap()
}

fn create_req(tx_id: &str, name: &str, from: &str, at: Option<At>) -> BranchCreate {
    BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at,
        dry_run: false,
    }
}

fn new_branch(s: &mut Store, tx_id: &str, name: &str, from: &str) -> BranchCreated {
    s.create_branch(create_req(tx_id, name, from, None))
        .unwrap()
}

fn set(
    s: &mut Store,
    tx_id: &str,
    branch: &str,
    name: Option<&str>,
    archived: Option<bool>,
) -> CommittedTx {
    s.set_branch(BranchSet {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: branch.into(),
        name: name.map(String::from),
        archived,
        dry_run: false,
    })
    .unwrap()
}

fn applied(events: &[StoreEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|ev| match ev {
            StoreEvent::Applied(c) => Some((c.tx.branch.clone(), c.tx.tx_id.clone())),
            _ => None,
        })
        .collect()
}

/// Every branch, archived ones too, with its state at the head.
fn states(s: &Store) -> Vec<(String, u64, fin_core::Project)> {
    s.branches(true)
        .unwrap()
        .into_iter()
        .map(|b| {
            let (rev, p) = s.snapshot(&b.id).unwrap();
            (b.id, rev, p)
        })
        .collect()
}

fn branch_dir(dir: &Path, branch: &str) -> PathBuf {
    dir.join(".finisher").join("segments").join(branch)
}

fn files(dir: &Path, branch: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(branch_dir(dir, branch))
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    v.retain(|p| p.extension().is_some_and(|x| x == "fseg"));
    v.sort();
    v
}

fn decode(path: &Path) -> segment::Segment {
    segment::decode(&fs::read(path).unwrap()).unwrap()
}

// ---- real lease: a leader and a follower on one folder ----------------------------------

/// The folder round trip: branches made on one server (a fork at the head, below it, at 0,
/// and a branch of a branch) reach another, with the same index, names, states and heads.
#[test]
fn follower_learns_branch_from_parent_record() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    commit_on(&mut a, "t2", MAIN, vec![insert(0)]);
    let feature = new_branch(&mut a, "b1", "feature", MAIN);
    assert_eq!(
        (feature.branch.id.as_str(), feature.branch.fork_seq),
        ("br1", 2)
    );
    commit_on(&mut a, "c1", "feature", vec![insert(1)]);
    a.create_branch(create_req("b2", "early", MAIN, Some(At::Seq(1))))
        .unwrap();
    a.create_branch(create_req("b3", "zero", MAIN, Some(At::Seq(0))))
        .unwrap();
    commit_on(&mut a, "z1", "zero", vec![asset("/m/z.mov")]);
    new_branch(&mut a, "b4", "deep", "feature");
    commit_on(&mut a, "d1", "deep", vec![insert(2)]);
    e.flush(&mut a);

    let r = e.poll(&mut b);
    let got = applied(&r.events);
    for want in [
        ("main", "t1"),
        ("main", "b1"),
        ("br1", "c1"),
        ("br3", "z1"),
        ("br1", "b4"),
        ("br4", "d1"),
    ] {
        assert!(
            got.contains(&(want.0.to_string(), want.1.to_string())),
            "{want:?} in {got:?}"
        );
    }
    assert_eq!(b.branches(true).unwrap(), a.branches(true).unwrap());
    assert_eq!(states(&b), states(&a));
    let st = b.status();
    assert_eq!(st.branches, a.status().branches);
    assert!(st.unknown_branches.is_empty());
    let deep = st.branches.iter().find(|s| s.branch == "br4").unwrap();
    assert_eq!(
        (
            deep.name.as_str(),
            deep.parent.as_deref(),
            deep.fork_seq,
            deep.state
        ),
        ("deep", Some("br1"), 3, BranchState::Active)
    );
    assert_eq!(
        st.branches
            .iter()
            .map(|s| s.branch.as_str())
            .collect::<Vec<_>>(),
        ["main", "br1", "br2", "br3", "br4"],
        "main first, then by numeric id"
    );

    // Following: later edits on a branch arrive too.
    commit_on(&mut a, "c2", "feature", vec![insert(3)]);
    e.flush(&mut a);
    let r = e.poll(&mut b);
    assert_eq!(applied(&r.events), [("br1".to_string(), "c2".to_string())]);
    assert_eq!(states(&b), states(&a));
}

#[test]
fn branch_set_and_rename_follow() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    new_branch(&mut a, "b1", "feature", MAIN);
    new_branch(&mut a, "b2", "other", MAIN);
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.branches(true).unwrap(), a.branches(true).unwrap());

    set(&mut a, "s1", "feature", Some("cut"), None);
    set(&mut a, "s2", "other", None, Some(true));
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.branches(true).unwrap(), a.branches(true).unwrap());
    assert_eq!(b.resolve_branch("cut").unwrap(), "br1");
    assert_eq!(b.resolve_branch("other#br2").unwrap(), "br2");
    assert!(
        b.resolve_branch("other").is_err(),
        "archived names resolve by id"
    );
    let st = b.status();
    let other = st.branches.iter().find(|s| s.branch == "br2").unwrap();
    assert_eq!(other.state, BranchState::Archived);

    // Restored under a new name.
    set(&mut a, "s3", "other#br2", Some("back"), Some(false));
    e.flush(&mut a);
    e.poll(&mut b);
    assert_eq!(b.branches(true).unwrap(), a.branches(true).unwrap());
    assert_eq!(b.resolve_branch("back").unwrap(), "br2");
}

#[test]
fn export_writes_parent_before_child() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    new_branch(&mut a, "b1", "feature", MAIN);
    commit_on(&mut a, "c1", "feature", vec![insert(0)]);
    // "br1" sorts before "main": by name the child would go first. Export stops after the
    // first segment it places.
    e.poll(&mut a);
    e.advance(3);
    a.fail_next_export_at(FailPoint::AfterRename);
    a.poll_now_for_test();
    assert!(a.tick_report(e.clock.now()).is_err());
    assert_eq!(files(&e.dir, MAIN).len(), 1, "the parent's segment first");
    assert!(files(&e.dir, "br1").is_empty());

    // The follower after the same poll: the branch record is there, nothing waits.
    let r = e.poll(&mut b);
    assert!(b.status().unknown_branches.is_empty());
    assert_eq!(applied(&r.events).len(), 2);
    assert_eq!(b.head("br1"), 1, "the fork state");

    e.flush(&mut a);
    e.poll(&mut b);
    assert!(b.status().unknown_branches.is_empty());
    assert_eq!(states(&b), states(&a));
}

#[test]
fn export_branch_first_segment_has_no_prev_and_parent_is_fork_hash() {
    let e = is::env();
    let mut a = e.create("a");
    let t1 = commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    commit_on(&mut a, "t2", MAIN, vec![insert(0)]);
    a.create_branch(create_req("b1", "early", MAIN, Some(At::Seq(1))))
        .unwrap();
    commit_on(&mut a, "c1", "early", vec![insert(5)]);
    e.flush(&mut a);
    assert_eq!(t1.rev, 1);
    let main = files(&e.dir, MAIN);
    assert_eq!(main.len(), 1);
    let main_seg = decode(&main[0]);
    let fork_hash = format!("blake3:{}", blake3::hash(&main_seg.records[0]).to_hex());

    let child = files(&e.dir, "br1");
    assert_eq!(child.len(), 1);
    let seg = decode(&child[0]);
    assert_eq!(seg.header.prev_segment_hash, None);
    assert_eq!((seg.header.first_seq, seg.header.last_seq), (2, 2));
    let first: serde_json::Value = serde_json::from_slice(&seg.records[0]).unwrap();
    assert_eq!(first["parentHash"], fork_hash.as_str());
    assert_eq!(first["seq"], 2);
}

#[test]
fn ids_counter_raised_by_imported_branch_record() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    new_branch(&mut a, "b1", "feature", MAIN);
    e.flush(&mut a);
    e.poll(&mut b);
    // A dry run on the follower plans against its own counters.
    let mut req = create_req("b2", "next", MAIN, None);
    req.dry_run = true;
    let planned = b.create_branch(req).unwrap();
    assert!(planned.dry_run);
    assert_eq!(planned.branch.id, "br2");
}

// ---- copies of the creator's DB on the test lease ---------------------------------------

struct T {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

fn tenv() -> T {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    T {
        dir: root.join("My Project"),
        root,
        _tmp: tmp,
    }
}

impl T {
    fn data(&self, server: &str) -> PathBuf {
        self.root.join(format!("server-{server}"))
    }
}

fn create(t: &T) -> Store {
    Store::create(&t.data("a"), &t.dir, "Demo", Rational::from_int(30), None).unwrap()
}

/// Another server whose DB is a copy of `a`'s at rev 0, on the test lease at epoch 0.
fn server(t: &T, a: &Store, name: &str) -> Store {
    let to = db_path(&t.data(name), a.project_id());
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    Connection::open(a.db_path())
        .unwrap()
        .execute("VACUUM INTO ?1", [to.to_string_lossy()])
        .unwrap();
    let mut s = Store::open(&t.data(name), &t.dir).unwrap();
    s.set_test_lease(0, true);
    s
}

/// Exports everything pending now (the 2 s delay passed).
fn export(s: &mut Store) -> u32 {
    let at = Instant::now();
    s.tick_report(at).unwrap();
    let r = s.tick_report(at + Duration::from_secs(3)).unwrap().export;
    assert_eq!(r.error, None);
    r.segments_written
}

/// `(txId, branch, reason, origin, detail, current)` of every quarantine row.
type QRow = (String, String, String, String, String, bool);

fn quarantine(s: &Store) -> Vec<QRow> {
    let c = Connection::open(s.db_path()).unwrap();
    let mut stmt = c
        .prepare("SELECT record_bytes, branch, reason, origin, detail, current FROM quarantine ORDER BY id")
        .unwrap();
    stmt.query_map([], |r| {
        let bytes: Vec<u8> = r.get(0)?;
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        Ok((
            v["txId"].as_str().unwrap().to_string(),
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn row_bytes(s: &Store, tx_id: &str) -> Vec<u8> {
    let c = Connection::open(s.db_path()).unwrap();
    c.query_row(
        "SELECT record_bytes FROM txs WHERE tx_id = ?1",
        [tx_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn short(bytes: &[u8]) -> String {
    let h = format!("blake3:{}", blake3::hash(bytes).to_hex());
    hash12(&h).unwrap().to_string()
}

#[test]
fn child_segments_before_branch_record_wait() {
    let t = tenv();
    let mut a = create(&t);
    let mut f = server(&t, &a, "f");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    new_branch(&mut a, "b1", "feature", MAIN);
    commit_on(&mut a, "c1", "feature", vec![insert(0)]);
    assert_eq!(export(&mut a), 2);

    // The parent's segment has not arrived yet: the child's directory waits.
    let hidden = t.root.join("hidden");
    fs::create_dir_all(&hidden).unwrap();
    let main = files(&t.dir, MAIN);
    for p in &main {
        fs::rename(p, hidden.join(p.file_name().unwrap())).unwrap();
    }
    let r = f.import().unwrap();
    assert_eq!(r.unknown_branches, ["br1"]);
    assert!(r.applied.is_empty());
    assert_eq!(r.quarantined, 0);
    assert_eq!(f.read_only(), None, "waiting is never read-only");
    assert!(quarantine(&f).is_empty(), "nothing is set aside");

    // It arrives: the parent's records and the child's apply in one pass.
    for p in &main {
        fs::rename(hidden.join(p.file_name().unwrap()), p).unwrap();
    }
    let r = f.import().unwrap();
    assert!(r.unknown_branches.is_empty());
    let ids: Vec<&str> = r.applied.iter().map(|c| c.tx.tx_id.as_str()).collect();
    assert_eq!(ids, ["t1", "b1", "c1"]);
    assert_eq!(states(&f), states(&a));
    assert_eq!(f.branches(true).unwrap(), a.branches(true).unwrap());
}

/// A stale line on `main` (epoch 1) holds a `branch` record, and that branch a `branch`
/// record of its own: every record of both children's directories goes to quarantine with
/// the parent record's reason; nothing waits, nothing is applied.
#[test]
fn set_aside_branch_record_quarantines_child_segments() {
    let t = tenv();
    let mut a = create(&t);
    let mut f = server(&t, &a, "f");
    let mut c = server(&t, &a, "c");
    c.set_test_lease(1, true);
    commit_on(&mut c, "x1", MAIN, vec![asset("/m/x.mov")]);
    new_branch(&mut c, "xb", "side", MAIN);
    commit_on(&mut c, "x2", "side", vec![insert(0)]);
    new_branch(&mut c, "xg", "deeper", "side");
    commit_on(&mut c, "x3", "deeper", vec![insert(1)]);
    assert_eq!(export(&mut c), 3);
    let (xb, xg) = (row_bytes(&c, "xb"), row_bytes(&c, "xg"));

    a.set_test_lease(2, true);
    commit_on(&mut a, "y1", MAIN, vec![asset("/m/y.mov")]);
    // A's tail (epoch 2) beats c's line: it is set aside with its children before A exports.
    let r = a.import().unwrap();
    assert_eq!((r.quarantined, r.applied.len(), r.fork), (5, 0, None));
    assert!(r.unknown_branches.is_empty());
    assert_eq!(export(&mut a), 1);

    let r = f.import().unwrap();
    let ids: Vec<&str> = r.applied.iter().map(|c| c.tx.tx_id.as_str()).collect();
    assert_eq!(ids, ["y1"]);
    assert!(r.unknown_branches.is_empty(), "nothing waits");
    assert_eq!(r.quarantined, 5);
    assert_eq!(f.read_only(), None);
    let mut q = quarantine(&f);
    q.sort();
    let side = format!("branch record {} set aside", short(&xb));
    let deeper = format!("branch record {} set aside", short(&xg));
    let rows: Vec<(&str, &str, &str, &str, bool)> = q
        .iter()
        .map(|r| (r.0.as_str(), r.1.as_str(), r.2.as_str(), r.3.as_str(), r.5))
        .collect();
    assert_eq!(
        rows,
        [
            ("x1", "main", "STALE_EPOCH", "folder", true),
            ("x2", "br1", "STALE_EPOCH", "folder", true),
            ("x3", "br2", "STALE_EPOCH", "folder", true),
            ("xb", "main", "STALE_EPOCH", "folder", true),
            ("xg", "br1", "STALE_EPOCH", "folder", true),
        ]
    );
    assert_eq!(q[1].4, side);
    assert_eq!(q[2].4, deeper);
    assert_eq!(q[4].4, side);
    assert_eq!(f.branches(true).unwrap().len(), 1, "main only");
    let st = f.status();
    assert!(st.unknown_branches.is_empty());
    assert_eq!(st.quarantine.reasons["STALE_EPOCH"], 5);

    // Stable across polls.
    let r = f.import().unwrap();
    assert_eq!((r.quarantined, r.applied.len()), (5, 0));
    assert_eq!(quarantine(&f).len(), 5);
}

/// Decision 28 (B5): ids the set-aside records used are never handed out here.
#[test]
fn ids_counter_raised_past_quarantined_ids() {
    let t = tenv();
    let mut a = create(&t);
    let mut f = server(&t, &a, "f");
    let mut c = server(&t, &a, "c");
    c.set_test_lease(1, true);
    commit_on(&mut c, "x1", MAIN, vec![asset("/m/x.mov")]);
    commit_on(
        &mut c,
        "x2",
        MAIN,
        vec![insert_as("$c1", 0), insert_as("$c2", 1)],
    );
    new_branch(&mut c, "xb", "side", MAIN);
    assert_eq!(export(&mut c), 1);
    a.set_test_lease(2, true);
    commit_on(&mut a, "y1", MAIN, vec![asset("/m/y.mov")]);
    a.import().unwrap();
    export(&mut a);

    f.import().unwrap();
    assert_eq!(quarantine(&f).len(), 3);
    f.set_test_lease(3, true);
    let made = new_branch(&mut f, "fb", "mine", MAIN);
    assert_eq!(made.branch.id, "br2", "br1 was used by a set-aside record");
    let tx = commit_on(&mut f, "f1", MAIN, vec![insert(0)]);
    assert_eq!(
        tx.bindings["$c"], "c3",
        "c1 and c2 were used by a set-aside record"
    );
}

/// Plan 4B Decision 8 with the same id made on two servers across a takeover: X (epoch 1)
/// makes `br1` and edits it, Y (epoch 2) took over before X's files landed and makes its own
/// `br1` at the same fork point. X's segments under `segments/br1/` belong to X's set-aside
/// branch, never to Y's.
#[test]
fn same_branch_id_on_two_servers_across_a_takeover() {
    let t = tenv();
    let a = create(&t);
    let mut x = server(&t, &a, "x");
    let mut y = server(&t, &a, "y");
    let mut f = server(&t, &a, "f");
    x.set_test_lease(1, true);
    commit_on(&mut x, "r1", MAIN, vec![asset("/m/a.mov")]);
    assert_eq!(export(&mut x), 1);
    y.import().unwrap();
    assert_eq!(y.head(MAIN), 1);

    // Y takes over; X, paused, still holds its own edits.
    y.set_test_lease(2, true);
    assert_eq!(new_branch(&mut y, "yb", "y-side", MAIN).branch.id, "br1");
    assert_eq!(new_branch(&mut x, "xb", "x-side", MAIN).branch.id, "br1");
    commit_on(&mut x, "x2", "x-side", vec![insert(0)]);
    assert_eq!(export(&mut x), 2, "X's late files land");
    let xb = row_bytes(&x, "xb");

    // Y's tail (epoch 2) beats X's `branch` record; X's child segment sits in Y's br1
    // directory at the same fork point, and has no records of Y's to lose against.
    let r = y.import().unwrap();
    assert!(r.applied.is_empty(), "{:?}", r.applied);
    assert_eq!(r.fork, None);
    assert_eq!(y.head("br1"), 1, "nothing of X's is applied onto Y's br1");
    assert_eq!(y.read_only(), None);
    let q = quarantine(&y);
    let x2 = q.iter().find(|r| r.0 == "x2").unwrap();
    assert_eq!(
        (x2.1.as_str(), x2.2.as_str(), x2.4.clone(), x2.5),
        (
            "br1",
            "STALE_EPOCH",
            format!("branch record {} set aside", short(&xb)),
            true
        )
    );
    assert_eq!(export(&mut y), 1);
    commit_on(&mut y, "y2", "y-side", vec![insert(3)]);
    assert_eq!(export(&mut y), 1);

    // A follower that sees everything at once agrees.
    let r = f.import().unwrap();
    let ids: Vec<&str> = r.applied.iter().map(|c| c.tx.tx_id.as_str()).collect();
    assert_eq!(ids, ["r1", "yb", "y2"]);
    assert_eq!(f.branches(true).unwrap(), y.branches(true).unwrap());
    assert_eq!(states(&f), states(&y));
    assert_eq!(f.read_only(), None);
    let mut set_aside: Vec<(String, String)> = quarantine(&f)
        .into_iter()
        .filter(|r| r.5)
        .map(|r| (r.0, r.2))
        .collect();
    set_aside.sort();
    assert_eq!(
        set_aside,
        [
            ("x2".to_string(), "STALE_EPOCH".to_string()),
            ("xb".to_string(), "STALE_EPOCH".to_string()),
        ]
    );
    // And Y's next branch id is past both.
    assert_eq!(new_branch(&mut y, "yc", "y-more", MAIN).branch.id, "br2");
}

/// A `branch` or `branchSet` record in the folder that does not fit the index (made by hand
/// or by a bug) is an inert row on its parent's chain (plan 4B Decision 33): the parent
/// keeps following, the index ignores it, status warns; never an error of the pass.
#[test]
fn unfit_branch_records_are_inert_rows() {
    type Edit = fn(&mut serde_json::Value);
    // (A `forkHash` against `forkSeq` does not decode at all, `Record::link`: as any record
    // that cannot be read, it stops the branch with an import error, as in 4A.)
    // (A known id is an id collision: `tests/branch_collide.rs`.)
    let edits: [(&str, Edit); 3] = [
        ("an id name", |v| v["name"] = "br9".into()),
        ("a reference name", |v| v["name"] = "a#br1".into()),
        ("a fork hash not of the parent's record", |v| {
            v["forkSeq"] = 1.into()
        }),
    ];
    for (what, edit) in edits {
        let t = tenv();
        let mut a = create(&t);
        let mut f = server(&t, &a, "f");
        commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
        new_branch(&mut a, "b1", "feature", MAIN);
        assert_eq!(export(&mut a), 1);
        f.import().unwrap();
        // A second branch record, edited, after the exported segment.
        new_branch(&mut a, "b2", "other", MAIN);
        let mut v: serde_json::Value = serde_json::from_slice(&row_bytes(&a, "b2")).unwrap();
        v["txId"] = "b2-edited".into();
        v["intent"]["txId"] = "b2-edited".into();
        edit(&mut v);
        let bad = serde_json::to_vec(&v).unwrap();
        let prev = segment::decode(&fs::read(&files(&t.dir, MAIN)[0]).unwrap())
            .unwrap()
            .hash;
        let h = Header {
            format_version: segment::FORMAT_VERSION,
            min_reader_version: segment::MIN_READER_VERSION,
            project_id: a.project_id().into(),
            branch: MAIN.into(),
            writer_id: "someone-else".into(),
            epoch: 1,
            first_seq: 3,
            last_seq: 3,
            prev_segment_hash: Some(prev),
            op_schema_versions: [1, 1],
            created_at: 0,
            genesis_hash: a.genesis_hash(),
            branch_record: None,
            pack: false,
            replaces: Vec::new(),
        };
        let (bytes, hash) = segment::encode(&h, &[&bad]);
        let name = SegmentName::new(1, 3, 3, hash12(&hash).unwrap()).unwrap();
        fs::write(branch_dir(&t.dir, MAIN).join(name.to_string()), bytes).unwrap();

        let r = f.import().unwrap();
        let ids: Vec<&str> = r.applied.iter().map(|c| c.tx.tx_id.as_str()).collect();
        assert_eq!(ids, ["b2-edited"], "{what}");
        assert_eq!(r.error, None, "{what}");
        assert_eq!(r.quarantined, 0, "{what}");
        assert_eq!(f.read_only(), None, "{what}");
        assert_eq!(f.head(MAIN), 3, "{what}: main keeps following");
        assert_eq!(
            f.branches(true).unwrap().len(),
            2,
            "{what}: the index ignores it"
        );
        let st = f.status();
        let w = st
            .warnings
            .iter()
            .find(|w| w.code == "BRANCH_RECORDS_IGNORED")
            .unwrap_or_else(|| panic!("{what}: {:?}", st.warnings));
        assert!(w.message.contains("b2-edited"), "{what}: {}", w.message);
        assert!(f.import().unwrap().applied.is_empty(), "{what}");
    }
}

#[test]
fn waiting_child_is_listed_in_status_not_read_only() {
    let e = is::env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    commit_on(&mut a, "t1", MAIN, vec![asset("/m/a.mov")]);
    new_branch(&mut a, "b1", "feature", MAIN);
    commit_on(&mut a, "c1", "feature", vec![insert(0)]);
    e.flush(&mut a);
    let hidden = e.root.join("hidden");
    fs::create_dir_all(&hidden).unwrap();
    let main = files(&e.dir, MAIN);
    for p in &main {
        fs::rename(p, hidden.join(p.file_name().unwrap())).unwrap();
    }
    e.poll(&mut b);
    let st = b.status();
    assert_eq!(st.unknown_branches, ["br1"]);
    assert_eq!(
        b.read_only(),
        Some(ReadOnlyReason::LeaseHeld),
        "only the lease"
    );
}
