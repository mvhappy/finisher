//! Lineage-aware reads (plan 4B Task 7, Decision 9): a branch below its fork point reads its
//! parent's history. Branches are made here by hand (an index row in `branches` plus the
//! parent's state at the fork as the child's snapshot, Decision 14), as Task 8's
//! `create_branch` will make them.

use std::path::PathBuf;

use fin_core::{CommittedTx, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::{MAIN, Store};
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
    fin_core::AssetAdd::media(
        "$a",
        "/media/a.mov",
        secs(60),
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

fn clips(p: &Project) -> Vec<(String, Rational, Rational)> {
    p.timelines[0].tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

fn hash_at(conn: &Connection, branch: &str, seq: u64) -> String {
    conn.query_row(
        "SELECT tx_hash FROM txs WHERE branch = ?1 AND seq = ?2",
        params![branch, seq as i64],
        |r| r.get(0),
    )
    .unwrap()
}

/// Main: the asset (rev 1), `c1` at 0 for 5 s (rev 2), then `c2` at 5 for 3 s (rev 3);
/// `br1` forks from main at 2. Returns the store and main's state at 2.
fn forked(e: &Env) -> (Store, Project) {
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    s.commit(tx("m1", MAIN, 0, vec![asset()])).unwrap();
    s.commit(tx("m2", MAIN, 1, vec![insert("$c", 0, 5)]))
        .unwrap();
    let (rev, at_fork) = s.snapshot(MAIN).unwrap();
    assert_eq!(rev, 2);
    s.commit(tx("m3", MAIN, 2, vec![insert("$d", 5, 3)]))
        .unwrap();
    let conn = Connection::open(s.db_path()).unwrap();
    let fork_hash = hash_at(&conn, MAIN, 2);
    conn.execute(
        "INSERT INTO branches VALUES ('br1', 'feature', 'main', 2, ?1, 3, 'test', 0, NULL, 0, 0, 0)",
        params![fork_hash],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO snapshots (branch, seq, json) VALUES ('br1', 2, ?1)",
        params![serde_json::to_string(&at_fork).unwrap()],
    )
    .unwrap();
    (s, at_fork)
}

#[test]
fn ops_since_below_fork_reads_parent() {
    let e = env();
    let (mut s, at_fork) = forked(&e);
    let main = s.ops_since(MAIN, 0).unwrap();
    assert_eq!(s.snapshot("br1").unwrap(), (2, at_fork.clone()));
    assert_eq!(s.ops_since("br1", 0).unwrap(), main[..2].to_vec());
    assert_eq!(s.ops_since("br1", 1).unwrap(), main[1..2].to_vec());
    assert_eq!(s.ops_since("br1", 2).unwrap(), vec![]);

    // The first record of br1 continues the seqs from the fork and links to the fork hash.
    let own = s
        .commit(tx("b3", "br1", 2, vec![insert("$e", 0, 2)]))
        .unwrap();
    assert_eq!(own.rev, 3);
    let conn = Connection::open(s.db_path()).unwrap();
    let parent: Option<String> = conn
        .query_row(
            "SELECT parent_hash FROM txs WHERE branch = 'br1' AND seq = 3",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(parent, Some(hash_at(&conn, MAIN, 2)));
    let mut expected = main[..2].to_vec();
    expected.push(own.clone());
    assert_eq!(s.ops_since("br1", 0).unwrap(), expected);
    assert_eq!(s.ops_since("br1", 2).unwrap(), vec![own]);
    // Main's own rev 3 never shows on br1.
    let (rev, p) = s.snapshot("br1").unwrap();
    assert_eq!(rev, 3);
    assert_eq!(
        clips(&p),
        vec![
            ("c3".to_string(), secs(0), secs(2)),
            ("c1".to_string(), secs(2), secs(5)),
        ]
    );
    assert_eq!(s.head(MAIN), 3);
}

/// A stale tx on a branch whose base is below the fork is rebased over the parent's records
/// after its base, then the branch's own.
#[test]
fn stale_commit_below_fork_rebases_over_lineage() {
    let e = env();
    let (mut s, _) = forked(&e);
    // Built at rev 1 (before main's c1), so it is rebased over main's rev 2.
    let c = s
        .commit(tx("b-stale", "br1", 1, vec![insert("$e", 0, 2)]))
        .unwrap();
    assert_eq!((c.rev, c.tx.base_rev), (3, 2));
    let (_, p) = s.snapshot("br1").unwrap();
    // The committed insert at the same point stays first (as on a single branch).
    assert_eq!(
        clips(&p),
        vec![
            ("c1".to_string(), secs(0), secs(5)),
            ("c3".to_string(), secs(5), secs(2)),
        ]
    );
    // Rebased over br1's own record as well, from a base below the fork.
    let c = s.commit(tx("b-stale-2", "br1", 0, vec![asset()])).unwrap();
    assert_eq!(c.rev, 4);
}

/// Pending diffs (a seeded rebuild) on the parent are filled when a read on the child
/// reaches below its fork, and only then.
#[test]
fn ops_since_fills_pending_ancestor_diffs() {
    let e = env();
    let (s, _) = forked(&e);
    let main: Vec<CommittedTx> = s.ops_since(MAIN, 0).unwrap();
    let conn = Connection::open(s.db_path()).unwrap();
    conn.execute(
        "UPDATE txs SET diff = 'null' WHERE branch = 'main' AND seq <= 2",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO meta (key, value) VALUES ('diffsPending.main', '2')",
        [],
    )
    .unwrap();
    let pending = || -> Option<String> {
        conn.query_row(
            "SELECT value FROM meta WHERE key = 'diffsPending.main'",
            [],
            |r| r.get(0),
        )
        .ok()
    };
    // Reads at or above the fork do not reach main's records.
    assert_eq!(s.ops_since("br1", 2).unwrap(), vec![]);
    assert_eq!(pending().as_deref(), Some("2"));
    assert_eq!(s.ops_since("br1", 0).unwrap(), main[..2].to_vec());
    assert_eq!(pending(), None);
    let nulls: i64 = conn
        .query_row("SELECT COUNT(*) FROM txs WHERE diff = 'null'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(nulls, 0);
}
