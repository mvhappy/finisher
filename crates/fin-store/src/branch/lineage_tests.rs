//! Lineage reads over hand-made index rows (Task 8 writes them from `branch` records).

use std::collections::HashMap;

use fin_core::{Edge, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use rusqlite::{Connection, params};

use super::*;
use crate::{MAIN, Store};

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
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

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (data, dir) = (tmp.path().join("data"), tmp.path().join("p"));
    let s = Store::create(&data, &dir, "Demo", secs(30), None).unwrap();
    (tmp, s)
}

/// Main with the asset (rev 1) and `n - 1` one-second inserts at the start (revs 2..=n).
fn main_with(s: &mut Store, n: u64) {
    s.commit(tx("m1", MAIN, 0, vec![asset()])).unwrap();
    for i in 2..=n {
        let id = format!("m{i}");
        s.commit(tx(&id, MAIN, i - 1, vec![insert("$c", 0, 1)]))
            .unwrap();
    }
}

/// An index row for `id` forking from `parent` at `fork_seq`, and the parent's state there as
/// its snapshot (Decision 14). Returns the fork hash.
fn fork(conn: &Connection, id: &str, parent: &str, fork_seq: u64) -> Option<String> {
    let fork_hash = hash_at(conn, parent, fork_seq).unwrap();
    conn.execute(
        "INSERT INTO branches VALUES (?1, ?1, ?2, ?3, ?4, NULL, 'test', 0, NULL, 0, 0, 0)",
        params![id, parent, fork_seq as i64, fork_hash],
    )
    .unwrap();
    let state = state_at(conn, parent, fork_seq).unwrap();
    db::insert_snapshot(conn, id, fork_seq, &state).unwrap();
    fork_hash
}

fn seg(branch: &str, after: u64, through: Option<u64>, fork_hash: Option<String>) -> LineageSeg {
    LineageSeg {
        branch: branch.into(),
        after,
        through,
        fork_hash,
    }
}

/// Rewinding from the head and replaying from a snapshot give the same state at every rev.
#[test]
fn rewind_from_head_equals_replay() {
    let (_tmp, mut s) = store();
    let edits = vec![
        vec![asset()],
        vec![insert("$c", 0, 5), insert("$d", 5, 3)],
        vec![
            fin_core::ItemTrim {
                item: "c1".into(),
                edge: Edge::Start,
                delta: secs(1),
                mode: fin_core::TrimMode::Ripple,
                alone: false,
            }
            .into(),
        ],
        vec![
            fin_core::ItemDelete {
                item: "c2".into(),
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
        ],
        vec![insert("$e", 0, 2)],
        vec![
            fin_core::ItemTrim {
                item: "c3".into(),
                edge: Edge::End,
                delta: secs(4),
                mode: fin_core::TrimMode::Ripple,
                alone: false,
            }
            .into(),
        ],
    ];
    for (i, ops) in edits.into_iter().enumerate() {
        s.commit(tx(&format!("tx-{i}"), MAIN, i as u64, ops))
            .unwrap();
    }
    let (head_rev, head) = s.snapshot(MAIN).unwrap();
    let segs = lineage(&s.conn, MAIN).unwrap();
    for seq in 0..=head_rev {
        let replayed = state_at(&s.conn, MAIN, seq).unwrap();
        let rewound = rewind_to(&s.conn, &segs, seq, head_rev, &head).unwrap();
        assert_eq!(rewound.as_ref(), Some(&replayed), "seq {seq}");
        let near = state_near_head(&s.conn, MAIN, seq, head_rev, &head).unwrap();
        assert_eq!(near, replayed, "seq {seq}");
    }
    // Rows that do not reach the given head are not rewound.
    assert_eq!(
        rewind_to(&s.conn, &segs, 2, head_rev + 1, &head).unwrap(),
        None
    );
}

/// At its fork a branch's tip is the fork hash (it has no row there), so `refresh` keeps its
/// cache and the first record links to it.
#[test]
fn refresh_tip_falls_back_to_fork_hash() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 3);
    let fork_hash = fork(&s.conn, "br1", MAIN, 2);
    assert!(fork_hash.is_some());
    let mut heads = HashMap::new();
    crate::refresh(&s.conn, &mut heads, "br1").unwrap();
    let head = heads["br1"].clone();
    assert_eq!((head.rev, head.tip.clone()), (2, fork_hash.clone()));
    assert_eq!(head.project, state_at(&s.conn, MAIN, 2).unwrap());
    assert_eq!(tip_hash(&s.conn, "br1").unwrap(), fork_hash);
    // The cached head is still valid: refresh keeps it (same tip, no reload).
    heads.get_mut("br1").unwrap().project.timelines[0]
        .tracks
        .clear();
    crate::refresh(&s.conn, &mut heads, "br1").unwrap();
    assert!(heads["br1"].project.timelines[0].tracks.is_empty());
    // Main itself is unchanged by the fallback.
    assert_eq!(
        tip_hash(&s.conn, MAIN).unwrap(),
        hash_at(&s.conn, MAIN, 3).unwrap()
    );
    // A branch forked at 0 has no fork hash.
    assert_eq!(fork(&s.conn, "br2", MAIN, 0), None);
    assert_eq!(tip_hash(&s.conn, "br2").unwrap(), None);
}

#[test]
fn lineage_walks_parents_and_clamps_to_the_lowest_fork() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 5);
    assert_eq!(
        lineage(&s.conn, MAIN).unwrap(),
        vec![seg(MAIN, 0, None, None)]
    );
    let h4 = fork(&s.conn, "br1", MAIN, 4);
    // br2 forks from br1 below br1's own fork: main's records through 2 only.
    let h2 = fork(&s.conn, "br2", "br1", 2);
    assert_eq!(h2, hash_at(&s.conn, MAIN, 2).unwrap());
    assert_eq!(
        lineage(&s.conn, "br2").unwrap(),
        vec![
            seg("br2", 2, None, h2.clone()),
            seg("br1", 4, Some(2), h4.clone()),
            seg(MAIN, 0, Some(2), None),
        ]
    );
    assert_eq!(hash_at(&s.conn, "br2", 2).unwrap(), h2);
    assert_eq!(hash_at(&s.conn, "br2", 3).unwrap(), None);
    assert_eq!(
        state_at(&s.conn, "br2", 1).unwrap(),
        state_at(&s.conn, MAIN, 1).unwrap()
    );
    // An unknown branch is a root of its own; a cycle is damage.
    assert_eq!(
        lineage(&s.conn, "x").unwrap(),
        vec![seg("x", 0, None, None)]
    );
    for (id, parent) in [("br8", "br9"), ("br9", "br8")] {
        s.conn
            .execute(
                "INSERT INTO branches VALUES (?1, ?1, ?2, 1, 'h', NULL, 't', 0, NULL, 0, 0, 0)",
                params![id, parent],
            )
            .unwrap();
    }
    assert_eq!(lineage(&s.conn, "br8").unwrap_err().code(), "DB_ERROR");
}

/// Below the fork: the parent's states and hashes; at it: the fork hash; above it: only the
/// branch's own rows (never the parent's later ones).
#[test]
fn reads_below_fork_read_the_parent() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 4);
    let fork_hash = fork(&s.conn, "br1", MAIN, 2);
    for seq in 0..=2 {
        assert_eq!(
            state_at(&s.conn, "br1", seq).unwrap(),
            state_at(&s.conn, MAIN, seq).unwrap(),
            "seq {seq}"
        );
    }
    assert_eq!(hash_at(&s.conn, "br1", 0).unwrap(), None);
    assert_eq!(
        hash_at(&s.conn, "br1", 1).unwrap(),
        hash_at(&s.conn, MAIN, 1).unwrap()
    );
    assert_eq!(hash_at(&s.conn, "br1", 2).unwrap(), fork_hash);
    assert_eq!(hash_at(&s.conn, "br1", 3).unwrap(), None);
    assert!(records_since(&s.conn, "br1", 2).unwrap().is_empty());

    s.commit(tx("b3", "br1", 2, vec![insert("$e", 0, 3)]))
        .unwrap();
    s.commit(tx("b4", "br1", 3, vec![insert("$f", 0, 4)]))
        .unwrap();
    let since = records_since(&s.conn, "br1", 0).unwrap();
    let ids: Vec<(&str, &str)> = since
        .iter()
        .map(|c| (c.tx.branch.as_str(), c.tx.tx_id.as_str()))
        .collect();
    assert_eq!(
        ids,
        [(MAIN, "m1"), (MAIN, "m2"), ("br1", "b3"), ("br1", "b4")]
    );
    assert_ne!(
        state_at(&s.conn, "br1", 3).unwrap(),
        state_at(&s.conn, MAIN, 3).unwrap()
    );
    // Rewinding from br1's head through its own records and then main's agrees with replay.
    let (head_rev, head) = s.snapshot("br1").unwrap();
    let segs = lineage(&s.conn, "br1").unwrap();
    for seq in 0..=head_rev {
        let rewound = rewind_to(&s.conn, &segs, seq, head_rev, &head).unwrap();
        assert_eq!(
            rewound,
            Some(state_at(&s.conn, "br1", seq).unwrap()),
            "seq {seq}"
        );
    }
}

/// `records_since` fills pending diffs per branch of the lineage, only where it reads.
#[test]
fn records_since_fills_pending_ancestor_diffs() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 3);
    fork(&s.conn, "br1", MAIN, 2);
    s.commit(tx("b3", "br1", 2, vec![insert("$e", 0, 3)]))
        .unwrap();
    let full = records_since(&s.conn, "br1", 0).unwrap();
    for (branch, through) in [(MAIN, 3), ("br1", 3)] {
        s.conn
            .execute(
                "UPDATE txs SET diff = 'null' WHERE branch = ?1 AND seq <= ?2",
                params![branch, through],
            )
            .unwrap();
        crate::diffs::set_pending(&s.conn, branch, through).unwrap();
    }
    // From the fork up: br1's own diffs are filled (from its fork snapshot), main's are not.
    assert_eq!(
        records_since(&s.conn, "br1", 2).unwrap(),
        full[2..].to_vec()
    );
    assert_eq!(crate::diffs::pending(&s.conn, "br1").unwrap(), None);
    assert_eq!(crate::diffs::pending(&s.conn, MAIN).unwrap(), Some(3));
    assert_eq!(records_since(&s.conn, "br1", 0).unwrap(), full);
    assert_eq!(crate::diffs::pending(&s.conn, MAIN).unwrap(), None);
    let nulls: i64 = s
        .conn
        .query_row("SELECT COUNT(*) FROM txs WHERE diff = 'null'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(nulls, 0);
}

/// A child seeded at its fork seq (a rebuild from a folder snapshot there) has no rows to
/// replay: the fill keeps its fork snapshot, its start.
#[test]
fn fill_at_the_fork_keeps_the_fork_snapshot() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 3);
    fork(&s.conn, "br1", MAIN, 2);
    crate::diffs::set_pending(&s.conn, "br1", 2).unwrap();
    crate::diffs::fill(&s.conn, "br1").unwrap();
    assert_eq!(crate::diffs::pending(&s.conn, "br1").unwrap(), None);
    assert!(db::snapshot_at(&s.conn, "br1", 2).unwrap().is_some());
    let mut heads = HashMap::new();
    crate::refresh(&s.conn, &mut heads, "br1").unwrap();
    assert_eq!(heads["br1"].rev, 2);
}

/// A parent that does not reach the fork is damage, not a shorter history.
#[test]
fn parent_short_of_the_fork_is_db_error() {
    let (_tmp, mut s) = store();
    main_with(&mut s, 3);
    let (_, head) = s.snapshot(MAIN).unwrap();
    s.conn
        .execute(
            "INSERT INTO branches VALUES ('br1', 'br1', 'main', 5, 'h5', NULL, 't', 0, NULL, 0, 0, 0)",
            [],
        )
        .unwrap();
    db::insert_snapshot(&s.conn, "br1", 5, &head).unwrap();
    for err in [
        state_at(&s.conn, "br1", 4).unwrap_err(),
        records_since(&s.conn, "br1", 0).unwrap_err(),
    ] {
        let e = fin_core::CoreError::from(err);
        assert_eq!(
            (e.code.as_str(), &e.details["seq"]),
            ("DB_ERROR", &serde_json::json!(4))
        );
    }
    // At and above the fork the branch's own rows are read.
    assert!(records_since(&s.conn, "br1", 5).unwrap().is_empty());
    assert_eq!(state_at(&s.conn, "br1", 5).unwrap(), head);
}
