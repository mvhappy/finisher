//! Branches derived from the log (plan 4B Task 8): `branch` and `branchSet` records on the
//! parent, the index, names and references, archived branches.

use std::path::PathBuf;

use fin_core::branch::BranchState;
use fin_core::{CoreError, Link, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::{At, BranchCreate, BranchCreated, BranchSet, MAIN, Store, StoreError};
use rusqlite::{Connection, params};
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

fn tx(id: &str, branch: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops,
    }
}

fn asset() -> Op {
    Op::AssetAdd {
        id: "$a".into(),
        path: "/media/a.mov".into(),
        duration: secs(60),
        video: Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        audio: None,
        fingerprint: "fp".into(),
    }
}

fn insert(id: &str, at: i64, duration: i64) -> Op {
    Op::ClipInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        asset: "a1".into(),
        source_in: secs(0),
        duration: secs(duration),
    }
}

fn clips(p: &Project) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

fn code(e: StoreError) -> CoreError {
    e.into()
}

fn conn(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

fn hash_at(s: &Store, branch: &str, seq: u64) -> String {
    conn(s)
        .query_row(
            "SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2",
            params![branch, seq as i64],
            |r| r.get(0),
        )
        .unwrap()
}

/// Everything a write could leave behind, for "nothing was written" checks.
fn rows(s: &Store) -> Vec<String> {
    let c = conn(s);
    let mut out = vec![];
    for q in [
        "SELECT branch || ':' || seq || ':' || tx_id FROM txs ORDER BY branch, seq",
        "SELECT branch || ':' || seq FROM snapshots ORDER BY branch, seq",
        "SELECT kind || '=' || next FROM ids ORDER BY kind",
        "SELECT tx_id || ':' || kind FROM record_links ORDER BY tx_id",
        "SELECT id || ':' || name || ':' || archived FROM branches ORDER BY id",
    ] {
        let mut stmt = c.prepare(q).unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        out.extend(r.map(Result::unwrap));
    }
    out
}

/// Main at rev 2: the asset, then `c1` at 0 for 5 s.
fn project(e: &Env) -> Store {
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    s.commit(tx("m1", MAIN, 0, vec![asset()])).unwrap();
    s.commit(tx("m2", MAIN, 1, vec![insert("$c", 0, 5)]))
        .unwrap();
    s
}

fn create(id: &str, from: &str, name: Option<&str>) -> BranchCreate {
    BranchCreate {
        tx_id: id.into(),
        author: "ann".into(),
        name: name.map(String::from),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    }
}

fn set(id: &str, branch: &str, name: Option<&str>, archived: Option<bool>) -> BranchSet {
    BranchSet {
        tx_id: id.into(),
        author: "ann".into(),
        branch: branch.into(),
        name: name.map(String::from),
        archived,
        dry_run: false,
        args_hash: None,
    }
}

fn new_branch(s: &mut Store, id: &str, from: &str, name: &str) -> BranchCreated {
    s.create_branch(create(id, from, Some(name))).unwrap()
}

#[test]
fn create_branch_appends_branch_record_on_parent() {
    let e = env();
    let mut s = project(&e);
    let fork_hash = hash_at(&s, MAIN, 2);
    let c = new_branch(&mut s, "b1", MAIN, "feature");
    assert!(!c.dry_run);
    assert_eq!(c.record.rev, 3);
    assert_eq!(c.record.tx.branch, MAIN);
    assert_eq!(c.record.tx.base_rev, 2);
    assert_eq!(c.record.tx.author, "ann");
    assert!(c.record.tx.ops.is_empty());
    assert_eq!(
        c.record.link.as_deref(),
        Some(&Link::Branch {
            branch_id: "br1".into(),
            name: "feature".into(),
            fork_seq: 2,
            fork_hash: Some(fork_hash.clone()),
            epoch: 1,
            recovers: None,
        })
    );
    assert_eq!(s.head(MAIN), 3);
    let v = &c.branch;
    assert_eq!(
        (v.id.as_str(), v.name.as_str(), v.display.as_str()),
        ("br1", "feature", "feature")
    );
    assert_eq!(
        (v.parent.as_deref(), v.fork_seq, v.head),
        (Some(MAIN), 2, 2)
    );
    assert_eq!(
        (v.state, v.created_by.as_str()),
        (BranchState::Active, "ann")
    );

    // The record and its link row are on main; the index row on its own.
    let c2 = conn(&s);
    let kind: String = c2
        .query_row(
            "SELECT kind FROM txs WHERE branch = 'main' AND seq = 3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, "branch");
    let link: (String, String, String) = c2
        .query_row(
            "SELECT branch, targets, name FROM record_links WHERE tx_id = 'b1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(link, ("main".into(), "[\"br1\"]".into(), "feature".into()));
    let row: (String, String, i64, Option<String>, i64, String) = c2
        .query_row(
            "SELECT name, parent, fork_seq, fork_hash, created_seq, created_by FROM branches
             WHERE id = 'br1'",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(
        row,
        (
            "feature".into(),
            "main".into(),
            2,
            Some(fork_hash),
            3,
            "ann".into()
        )
    );

    let all = s.branches(false).unwrap();
    let ids: Vec<(&str, u64)> = all.iter().map(|b| (b.id.as_str(), b.head)).collect();
    assert_eq!(ids, vec![("main", 3), ("br1", 2)]);
}

#[test]
fn branch_head_is_fork_seq_and_state_matches_parent_at_fork() {
    let e = env();
    let mut s = project(&e);
    let (_, at_fork) = s.snapshot(MAIN).unwrap();
    new_branch(&mut s, "b1", MAIN, "feature");
    // Main moves on; the branch keeps the state at its fork.
    s.commit(tx("m4", MAIN, 3, vec![insert("$d", 5, 3)]))
        .unwrap();
    assert_eq!(s.head("br1"), 2);
    assert_eq!(s.snapshot("br1").unwrap(), (2, at_fork.clone()));
    // The fork state is stored as the child's snapshot (Decision 14).
    let json: String = conn(&s)
        .query_row(
            "SELECT json FROM snapshots WHERE branch = 'br1' AND seq = 2",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(serde_json::from_str::<Project>(&json).unwrap(), at_fork);
    // A reopened store sees the same.
    drop(s);
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(s.snapshot("br1").unwrap(), (2, at_fork));
}

#[test]
fn first_record_on_branch_links_to_fork_hash_and_seq_continues() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    let c = s
        .commit(tx("f3", "br1", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
    assert_eq!(c.rev, 3);
    assert_eq!(c.tx.branch, "br1");
    let parent: Option<String> = conn(&s)
        .query_row(
            "SELECT parent_hash FROM txs WHERE branch = 'br1' AND seq = 3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, Some(hash_at(&s, MAIN, 2)));
    let c = s
        .commit(tx("f4", "br1", 3, vec![insert("$f", 0, 1)]))
        .unwrap();
    assert_eq!(c.rev, 4);
    assert_eq!(s.head("br1"), 4);
    assert_eq!(s.head(MAIN), 3, "main only got the branch record");
}

#[test]
fn from_older_seq_forks_there() {
    let e = env();
    let mut s = project(&e);
    let at1 = s.ops_since(MAIN, 0).unwrap()[0].clone();
    let mut r = create("b1", MAIN, Some("early"));
    r.at = Some(At::Seq(1));
    let c = s.create_branch(r).unwrap();
    assert_eq!(c.record.rev, 3, "the record goes at the parent's head");
    assert_eq!(
        c.record.link.as_deref(),
        Some(&Link::Branch {
            branch_id: "br1".into(),
            name: "early".into(),
            fork_seq: 1,
            fork_hash: Some(hash_at(&s, MAIN, 1)),
            epoch: 1,
            recovers: None,
        })
    );
    assert_eq!(c.branch.head, 1);
    let (rev, p) = s.snapshot("br1").unwrap();
    assert_eq!(rev, 1);
    assert!(clips(&p).is_empty(), "main's c1 came at 2");
    assert_eq!(s.ops_since("br1", 0).unwrap(), vec![at1]);

    // A fork below a branch's own fork reads the grandparent.
    let mut r = create("b2", "br1", Some("earlier"));
    r.at = Some(At::Seq(1));
    let c = s.create_branch(r).unwrap();
    assert_eq!(c.record.tx.branch, "br1");
    assert_eq!((c.record.rev, c.branch.fork_seq), (2, 1));
    assert_eq!(c.branch.parent.as_deref(), Some("br1"));
    assert_eq!(s.snapshot("br2").unwrap().0, 1);

    // Beyond the head is refused; so are checkpoints, until Task 15.
    let mut r = create("b3", MAIN, Some("late"));
    r.at = Some(At::Seq(9));
    assert_eq!(code(s.create_branch(r).unwrap_err()).code, "INVALID_OP");
    let mut r = create("b3", MAIN, Some("late"));
    r.at = Some(At::Checkpoint("cut".into()));
    let err = code(s.create_branch(r).unwrap_err());
    assert_eq!(
        (err.code.as_str(), &err.details["kind"]),
        ("NOT_FOUND", &serde_json::json!("checkpoint"))
    );
}

#[test]
fn fork_at_zero_has_null_fork_hash() {
    let e = env();
    let mut s = project(&e);
    let mut r = create("b1", MAIN, Some("blank"));
    r.at = Some(At::Seq(0));
    let c = s.create_branch(r).unwrap();
    assert_eq!(
        c.record.link.as_deref(),
        Some(&Link::Branch {
            branch_id: "br1".into(),
            name: "blank".into(),
            fork_seq: 0,
            fork_hash: None,
            epoch: 1,
            recovers: None,
        })
    );
    let (rev, p) = s.snapshot("br1").unwrap();
    assert_eq!(rev, 0);
    assert!(p.assets.is_empty());
    let c = s.commit(tx("z1", "br1", 0, vec![asset()])).unwrap();
    assert_eq!(c.rev, 1);
    let parent: Option<String> = conn(&s)
        .query_row(
            "SELECT parent_hash FROM txs WHERE branch = 'br1' AND seq = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, None);
}

#[test]
fn duplicate_active_name_refused_name_taken() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    let before = rows(&s);
    for name in ["feature", "FEATURE"] {
        let err = code(s.create_branch(create("b2", MAIN, Some(name))).unwrap_err());
        assert_eq!(err.code, "NAME_TAKEN");
        assert_eq!(err.details["name"], name);
        assert_eq!(err.details["branch"], "br1");
        assert!(err.fix.is_some());
    }
    // A bad name is refused before anything else.
    let err = code(
        s.create_branch(create("b2", MAIN, Some("main")))
            .unwrap_err(),
    );
    assert_eq!(err.code, "INVALID_NAME");
    // Renaming onto a taken name too; renaming to a new case of one's own name is not.
    new_branch(&mut s, "b2", MAIN, "other");
    let err = code(
        s.set_branch(set("r1", "other", Some("Feature"), None))
            .unwrap_err(),
    );
    assert_eq!(err.code, "NAME_TAKEN");
    s.set_branch(set("r2", "feature", Some("Feature"), None))
        .unwrap();
    assert_eq!(s.resolve_branch("feature").unwrap(), "br1");
    assert_ne!(rows(&s), before);
}

#[test]
fn archived_name_can_be_reused() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    s.set_branch(set("a1", "feature", None, Some(true)))
        .unwrap();
    let c = new_branch(&mut s, "b2", MAIN, "feature");
    assert_eq!(c.branch.id, "br2");
    assert_eq!(s.resolve_branch("feature").unwrap(), "br2");
    assert_eq!(s.resolve_branch("feature#br1").unwrap(), "br1");
    // Listings hide archived branches unless asked; an archived one shows as name#id.
    let shown: Vec<String> = s
        .branches(false)
        .unwrap()
        .into_iter()
        .map(|b| b.display)
        .collect();
    assert_eq!(shown, vec!["main", "feature"]);
    let all: Vec<(String, BranchState)> = s
        .branches(true)
        .unwrap()
        .into_iter()
        .map(|b| (b.display, b.state))
        .collect();
    assert_eq!(
        all,
        vec![
            ("main".into(), BranchState::Active),
            ("feature#br1".into(), BranchState::Archived),
            ("feature".into(), BranchState::Active),
        ]
    );
}

#[test]
fn restore_onto_taken_name_needs_as() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    s.set_branch(set("a1", "br1", None, Some(true))).unwrap();
    new_branch(&mut s, "b2", MAIN, "feature");
    let err = code(
        s.set_branch(set("a2", "br1", None, Some(false)))
            .unwrap_err(),
    );
    assert_eq!(err.code, "NAME_TAKEN");
    assert_eq!(err.details["branch"], "br2");
    assert!(
        err.fix.as_deref().unwrap().contains("--restore --as"),
        "{:?}",
        err.fix
    );
    // An archived branch may be renamed without restoring it, even to a name an active
    // branch holds: archived names hold nothing.
    s.set_branch(set("a3", "feature#br1", Some("Feature"), None))
        .unwrap();
    // `--restore --as <new>`.
    let c = s
        .set_branch(set("a4", "Feature#br1", Some("feature-old"), Some(false)))
        .unwrap();
    assert_eq!(
        c.link.as_deref(),
        Some(&Link::BranchSet {
            branch_id: "br1".into(),
            name: Some("feature-old".into()),
            archived: Some(false),
            recovers: None,
        })
    );
    assert_eq!(s.resolve_branch("feature-old").unwrap(), "br1");
    assert_eq!(s.resolve_branch("feature").unwrap(), "br2");
}

#[test]
fn rename_and_archive_are_branch_set_on_parent() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    new_branch(&mut s, "b2", "feature", "deep");
    let r = s
        .set_branch(set("r1", "deep", Some("deeper"), None))
        .unwrap();
    // br2's parent is br1: the record is on br1's log, at its head.
    assert_eq!((r.tx.branch.as_str(), r.rev), ("br1", 4));
    assert!(r.tx.ops.is_empty());
    assert_eq!(
        r.link.as_deref(),
        Some(&Link::BranchSet {
            branch_id: "br2".into(),
            name: Some("deeper".into()),
            archived: None,
            recovers: None,
        })
    );
    let a = s
        .set_branch(set("r2", "feature", None, Some(true)))
        .unwrap();
    assert_eq!((a.tx.branch.as_str(), a.rev), (MAIN, 4));
    let views = s.branches(true).unwrap();
    let br1 = views.iter().find(|b| b.id == "br1").unwrap();
    assert_eq!(br1.state, BranchState::Archived);
    let br2 = views.iter().find(|b| b.id == "br2").unwrap();
    assert_eq!(
        (br2.name.as_str(), br2.fork_seq, br2.head),
        ("deeper", 2, 2)
    );
    // Nothing to set is refused.
    let err = code(s.set_branch(set("r3", "deeper", None, None)).unwrap_err());
    assert_eq!(err.code, "INVALID_OP");
    // A set of a child of an archived branch still goes on it.
    s.set_branch(set("r4", "deeper", None, Some(true))).unwrap();
    // A dry run writes nothing.
    let before = rows(&s);
    let mut d = set("r5", "deeper#br2", None, Some(false));
    d.dry_run = true;
    let planned = s.set_branch(d).unwrap();
    assert_eq!(planned.rev, 6);
    assert_eq!(rows(&s), before);
}

#[test]
fn archived_branch_refuses_commits() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    s.set_branch(set("a1", "feature", None, Some(true)))
        .unwrap();
    for r in ["br1", "feature#br1"] {
        let err = code(
            s.commit(tx("x", r, 2, vec![insert("$e", 0, 2)]))
                .unwrap_err(),
        );
        assert_eq!(err.code, "BRANCH_ARCHIVED", "{r}");
        assert_eq!(err.details["branch"], "br1");
        assert_eq!(err.fix.as_deref(), Some("fin branch archive br1 --restore"));
    }
    // By its old name it is not found at all.
    let err = code(
        s.commit(tx("x", "feature", 2, vec![insert("$e", 0, 2)]))
            .unwrap_err(),
    );
    assert_eq!(err.code, "BRANCH_NOT_FOUND");
    // No branch forks from it either.
    let err = code(
        s.create_branch(create("b2", "br1", Some("child")))
            .unwrap_err(),
    );
    assert_eq!(err.code, "BRANCH_ARCHIVED");
    // Restored, it takes commits again.
    s.set_branch(set("a2", "br1", None, Some(false))).unwrap();
    s.commit(tx("x", "feature", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
}

#[test]
fn main_cannot_be_renamed_or_archived() {
    let e = env();
    let mut s = project(&e);
    for (name, archived) in [
        (Some("trunk"), None),
        (None, Some(true)),
        (None, Some(false)),
    ] {
        let err = code(s.set_branch(set("m", MAIN, name, archived)).unwrap_err());
        assert_eq!(err.code, "INVALID_OP");
        assert_eq!(err.details["branch"], MAIN);
    }
    assert_eq!(s.head(MAIN), 2);
}

#[test]
fn resolve_by_name_or_id_everywhere() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    let c = s
        .commit(tx("f3", "Feature", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
    // The stored intent holds the id.
    assert_eq!(c.tx.branch, "br1");
    assert_eq!(s.ops_since("br1", 2).unwrap()[0].tx.branch, "br1");
    for r in ["br1", "feature", "FEATURE", "feature#br1"] {
        assert_eq!(s.resolve_branch(r).unwrap(), "br1", "{r}");
        assert_eq!(s.head(r), 3, "{r}");
        assert_eq!(s.snapshot(r).unwrap().0, 3, "{r}");
        assert_eq!(s.ops_since(r, 2).unwrap(), vec![c.clone()], "{r}");
        assert_eq!(s.ops_since_at(r, 2).unwrap().1, vec![c.clone()], "{r}");
        assert_eq!(s.generation(r), 0);
    }
    // A retried commit by name dedupes on its txId.
    let again = s
        .commit(tx("f3", "feature", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
    assert_eq!(again, c);
    for bad in ["nope", "feature#br2", "br7"] {
        let err = code(s.resolve_branch(bad).unwrap_err());
        assert_eq!(err.code, "BRANCH_NOT_FOUND", "{bad}");
        assert_eq!(err.details["branch"], bad);
        assert_eq!(s.head(bad), 0);
        assert_eq!(code(s.snapshot(bad).unwrap_err()).code, "BRANCH_NOT_FOUND");
        assert_eq!(
            code(s.ops_since(bad, 0).unwrap_err()).code,
            "BRANCH_NOT_FOUND"
        );
    }
}

#[test]
fn ops_since_below_fork_reads_parent() {
    let e = env();
    let mut s = project(&e);
    let main = s.ops_since(MAIN, 0).unwrap();
    new_branch(&mut s, "b1", MAIN, "feature");
    assert_eq!(s.ops_since("feature", 0).unwrap(), main);
    assert_eq!(s.ops_since("feature", 1).unwrap(), main[1..].to_vec());
    assert_eq!(s.ops_since("feature", 2).unwrap(), vec![]);
    // The branch record itself (main's rev 3) is not on the branch.
    assert_eq!(s.ops_since(MAIN, 2).unwrap().len(), 1);
}

#[test]
fn stale_base_below_fork_rebases_over_lineage() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    // Built at rev 1, before main's c1: rebased over main's rev 2.
    let c = s
        .commit(tx("f-stale", "feature", 1, vec![insert("$e", 0, 2)]))
        .unwrap();
    assert_eq!((c.rev, c.tx.base_rev), (3, 2));
    let (_, p) = s.snapshot("br1").unwrap();
    assert_eq!(
        clips(&p),
        vec![
            ("c1".to_string(), secs(0), secs(5)),
            ("c2".to_string(), secs(5), secs(2)),
        ]
    );
}

#[test]
fn recompute_matches_incremental() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    new_branch(&mut s, "b2", "feature", "deep");
    new_branch(&mut s, "b3", MAIN, "other");
    s.set_branch(set("r1", "deep", Some("deeper"), None))
        .unwrap();
    s.set_branch(set("r2", "other", None, Some(true))).unwrap();
    s.commit(tx("f", "deeper", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
    // A link row whose record is gone is collected.
    conn(&s)
        .execute(
            "INSERT INTO record_links (tx_id, branch, seq, kind, author) VALUES ('gone', 'main', 99, 'checkpoint', 'x')",
            [],
        )
        .unwrap();
    let table = |s: &Store| -> Vec<String> {
        let c = conn(s);
        let mut stmt = c
            .prepare(
                "SELECT id || '|' || name || '|' || IFNULL(parent, '-') || '|' || fork_seq || '|'
                        || IFNULL(fork_hash, '-') || '|' || IFNULL(created_seq, '-') || '|'
                        || created_by || '|' || created_at || '|' || archived || '|' || merged
                 FROM branches ORDER BY id",
            )
            .unwrap();
        let r = stmt.query_map([], |r| r.get::<_, String>(0)).unwrap();
        r.map(Result::unwrap).collect()
    };
    let before = table(&s);
    assert_eq!(before.len(), 4);
    let links = |s: &Store| -> i64 {
        conn(s)
            .query_row("SELECT COUNT(*) FROM record_links", [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(links(&s), 6);
    s.recompute_derived_for_test().unwrap();
    assert_eq!(table(&s), before);
    assert_eq!(links(&s), 5);
}

#[test]
fn branch_ids_never_reused_after_reopen() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    s.set_branch(set("a1", "feature", None, Some(true)))
        .unwrap();
    drop(s);
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    let c = new_branch(&mut s, "b2", MAIN, "feature");
    assert_eq!(c.branch.id, "br2");
    // A failed create consumes no id.
    assert!(
        s.create_branch(create("b3", MAIN, Some("feature")))
            .is_err()
    );
    assert_eq!(new_branch(&mut s, "b4", MAIN, "third").branch.id, "br3");
}

#[test]
fn create_branch_dry_run_writes_nothing() {
    let e = env();
    let mut s = project(&e);
    let before = rows(&s);
    let mut r = create("b1", MAIN, Some("feature"));
    r.dry_run = true;
    let planned = s.create_branch(r).unwrap();
    assert!(planned.dry_run);
    assert_eq!(
        (planned.branch.id.as_str(), planned.branch.head),
        ("br1", 2)
    );
    assert_eq!(planned.record.rev, 3);
    assert_eq!(rows(&s), before);
    assert_eq!(s.head(MAIN), 2);
    assert_eq!(
        code(s.resolve_branch("feature").unwrap_err()).code,
        "BRANCH_NOT_FOUND"
    );

    // On a follower too (no lease): planned against the local database, nothing kept.
    let mut f = Store::open(&e.data, &e.dir).unwrap();
    assert!(f.read_only().is_some());
    let err = code(
        f.create_branch(create("b1", MAIN, Some("feature")))
            .unwrap_err(),
    );
    assert_eq!(err.code, "PROJECT_READ_ONLY");
    let mut r = create("b1", MAIN, Some("feature"));
    r.dry_run = true;
    let planned = f.create_branch(r).unwrap();
    assert_eq!(planned.branch.id, "br1");
    assert_eq!(rows(&f), before);

    // The real create gets the id the dry runs planned.
    assert_eq!(new_branch(&mut s, "b1", MAIN, "feature").branch.id, "br1");
}

#[test]
fn branch_args_hash_is_stored_and_checked_on_retry() {
    let e = env();
    let mut s = project(&e);
    let h1 = format!("blake3:{}", "a".repeat(64));
    let h2 = format!("blake3:{}", "b".repeat(64));
    let mut c = create("b1", MAIN, Some("feature"));
    c.args_hash = Some(h1.clone());
    let first = s.create_branch(c.clone()).unwrap();
    assert_eq!(first.record.tx.args_hash.as_deref(), Some(h1.as_str()));
    assert_eq!(s.create_branch(c.clone()).unwrap().record, first.record);
    c.args_hash = Some(h2.clone());
    assert_eq!(
        code(s.create_branch(c).unwrap_err()).code,
        "IDEM_KEY_REUSED"
    );
    let mut bad = create("b2", MAIN, None);
    bad.args_hash = Some("nope".into());
    assert_eq!(code(s.create_branch(bad).unwrap_err()).code, "INVALID_OP");

    let mut r = set("r1", "feature", Some("renamed"), None);
    r.args_hash = Some(h1.clone());
    let rec = s.set_branch(r.clone()).unwrap();
    assert_eq!(rec.tx.args_hash.as_deref(), Some(h1.as_str()));
    assert_eq!(s.set_branch(r.clone()).unwrap(), rec);
    r.args_hash = Some(h2);
    assert_eq!(code(s.set_branch(r).unwrap_err()).code, "IDEM_KEY_REUSED");
}

#[test]
fn create_branch_retry_same_tx_id_returns_first() {
    let e = env();
    let mut s = project(&e);
    let first = new_branch(&mut s, "b1", MAIN, "feature");
    let before = rows(&s);
    // The retry is answered from the log, whatever else it asks.
    let again = s.create_branch(create("b1", MAIN, Some("other"))).unwrap();
    assert_eq!(again.record, first.record);
    assert_eq!(again.branch, first.branch);
    assert_eq!(rows(&s), before);
    // A set retry likewise.
    let r = s
        .set_branch(set("r1", "feature", Some("renamed"), None))
        .unwrap();
    assert_eq!(
        s.set_branch(set("r1", "renamed", None, Some(true)))
            .unwrap(),
        r
    );
    // The key of another kind of record is reused.
    let err = code(s.create_branch(create("r1", MAIN, Some("x"))).unwrap_err());
    assert_eq!(err.code, "IDEM_KEY_REUSED");
    let err = code(
        s.set_branch(set("m1", "renamed", Some("y"), None))
            .unwrap_err(),
    );
    assert_eq!(err.code, "IDEM_KEY_REUSED");
}

#[test]
fn default_try_name_uses_branch_id() {
    let e = env();
    let mut s = project(&e);
    let c = s.create_branch(create("t1", MAIN, None)).unwrap();
    assert_eq!(
        (c.branch.id.as_str(), c.branch.name.as_str()),
        ("br1", "try-1")
    );
    new_branch(&mut s, "b2", MAIN, "x");
    let c = s.create_branch(create("t3", "try-1", None)).unwrap();
    assert_eq!(
        (c.branch.id.as_str(), c.branch.name.as_str()),
        ("br3", "try-3")
    );
    assert_eq!(c.branch.parent.as_deref(), Some("br1"));
}

// Task 8 review: a user branch holding `try-<n>` never blocks unnamed creates that draw br<n>.
#[test]
fn default_try_name_skips_a_taken_one() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "try-2");
    let c = s.create_branch(create("t2", MAIN, None)).unwrap();
    assert_eq!(
        (c.branch.id.as_str(), c.branch.name.as_str()),
        ("br2", "try-2-2")
    );
    new_branch(&mut s, "b3", MAIN, "try-3-2");
    new_branch(&mut s, "b4", MAIN, "try-5");
    let c = s.create_branch(create("t5", MAIN, None)).unwrap();
    assert_eq!(c.branch.name, "try-5-2");
}

// Task 8 review: a `branchSet` that changes nothing is refused, never written.
#[test]
fn branch_set_that_changes_nothing_is_refused() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    let before = rows(&s);
    for (id, name, archived) in [
        ("n1", None, Some(false)),
        ("n2", Some("feature"), None),
        ("n3", Some("feature"), Some(false)),
    ] {
        let err = code(s.set_branch(set(id, "br1", name, archived)).unwrap_err());
        assert_eq!(err.code, "INVALID_OP", "{id}");
        assert!(err.message.contains("nothing to change"), "{id}");
    }
    s.set_branch(set("a1", "br1", None, Some(true))).unwrap();
    let err = code(
        s.set_branch(set("a2", "feature#br1", None, Some(true)))
            .unwrap_err(),
    );
    assert_eq!(err.code, "INVALID_OP");
    assert!(err.fix.unwrap().contains("--restore"));
    // A change of case is a rename.
    s.set_branch(set("a3", "feature#br1", Some("Feature"), None))
        .unwrap();
    assert_ne!(rows(&s), before);
}

// Task 8 review: rows without an index row read as a root, but no write goes there.
#[test]
fn branch_without_index_row_refuses_writes() {
    let e = env();
    let mut s = project(&e);
    new_branch(&mut s, "b1", MAIN, "feature");
    s.commit(tx("c1", "br1", 2, vec![insert("$c", 5, 1)]))
        .unwrap();
    conn(&s)
        .execute("DELETE FROM branches WHERE id = 'br1'", [])
        .unwrap();
    assert_eq!(s.head("br1"), 3, "still readable");
    let err = code(
        s.commit(tx("c2", "br1", 3, vec![insert("$c", 6, 1)]))
            .unwrap_err(),
    );
    assert_eq!(err.code, "DB_ERROR");
    assert!(err.fix.unwrap().contains("rebuild"));
    let err = code(s.create_branch(create("b2", "br1", Some("x"))).unwrap_err());
    assert_eq!(err.code, "DB_ERROR");
}
