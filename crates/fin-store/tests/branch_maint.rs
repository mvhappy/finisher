//! Branches under verify, rebuild and compaction (plan 4B Task 11): a rebuild from the folder
//! gives the follower's database back (branches, heads, snapshots, index, links,
//! quarantine), verify checks each branch from its fork point and the derived tables
//! against the log (`branch_index`), and compaction packs a branch's segments without
//! touching its fork link, its parent, or another `branch` record's segments.

mod integration_support;

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::CommittedTx;
use fin_store::names::{SegmentName, hash12};
use fin_store::segment::{self, Segment};
use fin_store::{At, BranchCreate, BranchSet, MAIN, Store, VerifyReport};
use integration_support::*;
use rusqlite::Connection;

fn on(s: &mut Store, id: &str, branch: &str, ops: Vec<fin_core::Op>) -> CommittedTx {
    let base = s.head(branch);
    s.commit(fin_core::Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: fin_core::SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    })
    .unwrap()
}

fn branch(s: &mut Store, tx_id: &str, name: &str, from: &str, at: Option<u64>) -> String {
    s.create_branch(BranchCreate {
        tx_id: tx_id.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at: at.map(At::Seq),
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn set(s: &mut Store, tx_id: &str, b: &str, name: Option<&str>, archived: Option<bool>) {
    s.set_branch(BranchSet {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: b.into(),
        name: name.map(String::from),
        archived,
        dry_run: false,
        args_hash: None,
    })
    .unwrap();
}

fn db(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

fn rows(c: &Connection, sql: &str) -> Vec<String> {
    let mut stmt = c.prepare(sql).unwrap();
    let n = stmt.column_count();
    stmt.query_map([], |r| {
        let mut v = vec![];
        for i in 0..n {
            v.push(format!("{:?}", r.get_ref(i)?));
        }
        Ok(v.join("|"))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// What a rebuild must give back: every branch's state at its head, the log, the snapshots,
/// the index, the links and the quarantine (by record and reason).
type Dump = (Vec<(String, u64, fin_core::Project)>, [Vec<String>; 5]);

fn dump(s: &Store) -> Dump {
    let states = s
        .branches(true)
        .unwrap()
        .into_iter()
        .map(|b| {
            let (rev, p) = s.snapshot(&b.id).unwrap();
            (b.id, rev, p)
        })
        .collect();
    let c = db(s);
    (
        states,
        [
            rows(
                &c,
                "SELECT branch, seq, tx_hash, exported FROM txs ORDER BY branch, seq",
            ),
            rows(&c, "SELECT branch, seq FROM snapshots ORDER BY branch, seq"),
            rows(&c, "SELECT * FROM branches ORDER BY id"),
            rows(&c, "SELECT * FROM record_links ORDER BY tx_id"),
            rows(
                &c,
                "SELECT tx_hash, reason, current FROM quarantine ORDER BY tx_hash",
            ),
        ],
    )
}

fn check<'a>(r: &'a VerifyReport, name: &str) -> &'a fin_store::Check {
    r.checks.iter().find(|c| c.name == name).unwrap()
}

fn assert_verifies(r: &VerifyReport) {
    assert!(
        r.ok,
        "{:#?}",
        r.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>()
    );
}

/// Only `name` fails.
fn fails_only(r: &VerifyReport, name: &str) {
    let failed: Vec<&str> = r
        .checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(failed, [name], "{r:#?}");
}

fn dir_of(dir: &Path, branch: &str) -> PathBuf {
    dir.join(".finisher").join("segments").join(branch)
}

fn files(dir: &Path, branch: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir_of(dir, branch))
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    v.retain(|p| p.extension().is_some_and(|x| x == "fseg"));
    v.sort();
    v
}

fn decode(p: &Path) -> Segment {
    segment::decode(&fs::read(p).unwrap()).unwrap()
}

/// A, the writer: main, a branch at the head (renamed), one below the head (archived), one at
/// 0, a branch of a branch, edits on each, and main moving on.
fn history(e: &Env) -> Store {
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    assert_eq!(branch(&mut a, "b1", "feature", MAIN, None), "br1");
    on(&mut a, "c1", "br1", vec![insert(1, "a1")]);
    assert_eq!(branch(&mut a, "b2", "early", MAIN, Some(1)), "br2");
    on(&mut a, "e1", "br2", vec![insert(5, "a1")]);
    assert_eq!(branch(&mut a, "b3", "zero", MAIN, Some(0)), "br3");
    on(&mut a, "z1", "br3", vec![asset("/m/z.mov")]);
    e.flush(&mut a);
    assert_eq!(branch(&mut a, "b4", "deep", "br1", None), "br4");
    on(&mut a, "d1", "br4", vec![insert(2, "a1")]);
    set(&mut a, "s1", "feature", Some("cut"), None);
    set(&mut a, "s2", "early", None, Some(true));
    on(&mut a, "c2", "br1", vec![insert(3, "a1")]);
    commit(&mut a, "t3", vec![insert(7, "a1")]);
    e.flush(&mut a);
    a
}

/// Review focus 7: a rebuild from the folder alone gives the follower's database back, and
/// so does a server that has none; `branch_index` passes on each.
#[test]
fn rebuild_restores_branches_and_index() {
    let e = env();
    let a = history(&e);
    let mut f = e.open("f");
    e.poll(&mut f);
    let want = dump(&f);
    assert_eq!(want.0, dump(&a).0);
    assert_eq!(want.0.len(), 5);
    assert_verifies(&f.verify());
    assert_verifies(&a.verify());

    let rep = f.rebuild().unwrap();
    assert_eq!(rep.import_error, None);
    assert_eq!(rep.missing_from_folder, 0);
    assert!(rep.verify.ok, "{:#?}", rep.verify);
    assert!(check(&rep.verify, "branch_index").ok);
    e.poll(&mut f);
    assert_eq!(dump(&f), want);
    assert_verifies(&f.verify());

    let g = e.open("g");
    assert_eq!(g.last_rebuild().unwrap().reason, "missing");
    assert_eq!(dump(&g), want);
    assert_verifies(&g.verify());
}

#[test]
fn verify_checks_branch_fork_and_index() {
    let e = env();
    let a = history(&e);
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_verifies(&f.verify());
    let c = db(&f);

    // An index row that is not what the log says.
    c.execute("UPDATE branches SET name = 'tampered' WHERE id = 'br4'", [])
        .unwrap();
    let v = f.verify();
    fails_only(&v, "branch_index");
    assert_eq!(
        check(&v, "branch_index").details["index"][0]["branch"],
        "br4"
    );
    c.execute("UPDATE branches SET name = 'deep' WHERE id = 'br4'", [])
        .unwrap();
    assert_verifies(&f.verify());

    // A link row that is not what its record says, and an inert mark the log does not give.
    c.execute("UPDATE record_links SET name = 'x' WHERE tx_id = 's1'", [])
        .unwrap();
    c.execute(
        "UPDATE record_links SET ignored = 'no' WHERE tx_id = 'b2'",
        [],
    )
    .unwrap();
    let v = f.verify();
    fails_only(&v, "branch_index");
    let ids: BTreeSet<String> = check(&v, "branch_index").details["links"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["txId"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ids, BTreeSet::from(["b2".to_string(), "s1".to_string()]));
    c.execute_batch(
        "UPDATE record_links SET name = 'cut' WHERE tx_id = 's1';
         UPDATE record_links SET ignored = NULL WHERE tx_id = 'b2';",
    )
    .unwrap();
    assert_verifies(&f.verify());

    // A wrong first parent on a branch: the fork link.
    let (fork, hash): (i64, String) = c
        .query_row(
            "SELECT fork_seq, fork_hash FROM branches WHERE id = 'br1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let bogus = format!("blake3:{}", "0".repeat(64));
    c.execute(
        "UPDATE txs SET parent_hash = ?1 WHERE branch = 'br1' AND seq = ?2",
        rusqlite::params![bogus, fork + 1],
    )
    .unwrap();
    let v = f.verify();
    let problems = check(&v, "chain_continuity").details["problems"].clone();
    assert!(!check(&v, "chain_continuity").ok);
    assert!(
        problems
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["branch"] == "br1"
                && p["problem"] == "parent_hash is not the branch's forkHash"),
        "{problems:#}"
    );
    c.execute(
        "UPDATE txs SET parent_hash = ?1 WHERE branch = 'br1' AND seq = ?2",
        rusqlite::params![hash, fork + 1],
    )
    .unwrap();
    assert_verifies(&f.verify());

    // An index row without any state (re-review of 9b: such a branch cannot be read).
    let saved = rows(&c, "SELECT seq FROM snapshots WHERE branch = 'br4'");
    assert!(!saved.is_empty());
    c.execute_batch(
        "CREATE TEMP TABLE keep_txs AS SELECT * FROM txs WHERE branch = 'br4';
         CREATE TEMP TABLE keep_snaps AS SELECT * FROM snapshots WHERE branch = 'br4';
         DELETE FROM txs WHERE branch = 'br4'; DELETE FROM snapshots WHERE branch = 'br4';",
    )
    .unwrap();
    let v = f.verify();
    assert!(!check(&v, "branch_index").ok);
    assert_eq!(
        check(&v, "branch_index").details["stateless"][0]["branch"],
        "br4"
    );
    c.execute_batch(
        "INSERT INTO txs SELECT * FROM keep_txs; INSERT INTO snapshots SELECT * FROM keep_snaps;",
    )
    .unwrap();
    assert_verifies(&f.verify());

    // A branch segment that names a live branch record of another branch.
    let seg = decode(&files(&e.dir, "br1")[0]);
    let br4 = c
        .query_row("SELECT record FROM branches WHERE id = 'br4'", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    let mut h = seg.header.clone();
    h.branch_record = Some(br4.clone());
    let lines: Vec<&[u8]> = seg.records.iter().map(Vec::as_slice).collect();
    let (bytes, hash) = segment::encode(&h, &lines);
    let name = SegmentName::new(h.epoch, h.first_seq, h.last_seq, hash12(&hash).unwrap()).unwrap();
    let odd = dir_of(&e.dir, "br1").join(name.to_string());
    fs::write(&odd, bytes).unwrap();
    let v = f.verify();
    fails_only(&v, "branch_index");
    let bad = &check(&v, "branch_index").details["segments"][0];
    assert_eq!(bad["branch"], "br1");
    assert!(
        bad["problem"]
            .as_str()
            .unwrap()
            .contains("does not make branch br1"),
        "{bad}"
    );
    fs::remove_file(&odd).unwrap();
    assert_verifies(&f.verify());
    drop(a);
}

/// A branch's pack starts at its fork point like the segments it replaces, holds only
/// branch-kind segments' format (2), and once the originals are gone a fresh follower still
/// reads the branch; the parent's files are untouched.
#[test]
fn compaction_packs_branch_segments() {
    let e = env();
    let mut a = e.create("a");
    a.set_compaction_grace_for_test(Duration::from_secs(5));
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    assert_eq!(branch(&mut a, "b1", "feature", MAIN, None), "br1");
    e.flush(&mut a);
    for i in 0..3 {
        on(&mut a, &format!("c{i}"), "br1", vec![insert(i, "a1")]);
        assert_eq!(e.flush(&mut a), 1);
    }
    let main_before = files(&e.dir, MAIN);
    assert_eq!(main_before.len(), 1);
    let originals = files(&e.dir, "br1");
    assert_eq!(originals.len(), 3);
    let first = decode(&originals[0]);
    let mut f = e.open("f");
    e.poll(&mut f);

    let r = a.compact(true).unwrap();
    assert_eq!(r.packs_written, 1, "{r:?}");
    let pack: Vec<Segment> = files(&e.dir, "br1")
        .iter()
        .map(|p| decode(p))
        .filter(|s| s.header.pack)
        .collect();
    assert_eq!(pack.len(), 1);
    let p = &pack[0].header;
    assert_eq!(
        p.prev_segment_hash, None,
        "the branch start, as its first segment"
    );
    assert_eq!((p.first_seq, p.last_seq), (2, 4), "forked at 1");
    assert_eq!(p.branch_record, first.header.branch_record);
    assert_eq!(
        (p.format_version, p.min_reader_version),
        (2, 2),
        "a branch segment is format 2"
    );
    let rec: serde_json::Value = serde_json::from_slice(&pack[0].records[0]).unwrap();
    let parent: serde_json::Value = serde_json::from_slice(&first.records[0]).unwrap();
    assert_eq!(rec["parentHash"], parent["parentHash"], "the fork link");

    e.advance(6);
    let done = e.poll(&mut a).compact;
    assert_eq!(done.deleted, 3, "{done:?}");
    assert_eq!(files(&e.dir, "br1").len(), 1);
    assert_eq!(files(&e.dir, MAIN), main_before, "the parent is untouched");
    e.poll(&mut f);
    let g = e.open("g");
    for s in [&a, &f, &g] {
        assert_eq!(dump(s).0, dump(&a).0);
        assert_verifies(&s.verify());
    }
    on(&mut a, "c9", "br1", vec![insert(9, "a1")]);
    e.flush(&mut a);
    e.poll(&mut f);
    assert_eq!(
        dump(&f).0,
        dump(&a).0,
        "the branch keeps following after its pack"
    );
}

/// Review of b6fdab8 (rv_x1): a follower whose database is just behind the folder, with new
/// branches (one made on a new branch) it has not imported, verifies: those directories are
/// the folder ahead of it (`ahead`), as `main`'s new records are `folderAhead`.
#[test]
fn verify_behind_the_folder_with_new_branches() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut f = e.open("f");
    e.poll(&mut f);
    assert_eq!(branch(&mut a, "b1", "feature", MAIN, None), "br1");
    on(&mut a, "c1", "br1", vec![insert(1, "a1")]);
    assert_eq!(branch(&mut a, "b2", "deep", "br1", None), "br2");
    on(&mut a, "d1", "br2", vec![insert(2, "a1")]);
    e.flush(&mut a);
    let v = f.verify();
    assert_verifies(&v);
    let ahead = check(&v, "branch_index").details["ahead"].clone();
    let dirs: BTreeSet<&str> = ahead
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["branch"].as_str().unwrap())
        .collect();
    assert_eq!(dirs, BTreeSet::from(["br1", "br2"]), "{ahead:#}");
    e.poll(&mut f);
    assert_verifies(&f.verify());
    assert_eq!(dump(&f).0, dump(&a).0);
}
