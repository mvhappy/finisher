//! `Store::write`: dedupe, dry runs on a follower, rollback on failure, several records in one
//! transaction.

use std::collections::BTreeMap;

use fin_core::{Diff, Link, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo, apply_tx};
use rusqlite::params;

use crate::db::Counters;
use crate::write::{Draft, WriteCtx};
use crate::{MAIN, ReadOnlyReason, Store, StoreError, TxStatus};

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

fn tx(id: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
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
            display: None,
            rotation: None,
            pixel_aspect: None,
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

/// A fresh edit at the head of main, as a draft (ids from the counters in the transaction).
fn edit(ctx: &mut WriteCtx<'_>, id: &str, ops: Vec<Op>) -> Result<Draft, StoreError> {
    let txn = ctx.txn;
    let head = ctx.head(MAIN)?;
    let intent = tx(id, head.rev, ops);
    let mut ids = Counters::load(txn)?;
    let applied = apply_tx(&head.project, &intent, &mut ids)?;
    ids.save(txn)?;
    Ok(Draft {
        branch: MAIN.into(),
        link: Link::Edit,
        intent,
        inverse: applied.inverse,
        bindings: applied.bindings,
        diff: applied.diff,
        project: applied.project,
    })
}

/// A checkpoint record at the head of main (no ops).
fn checkpoint(ctx: &mut WriteCtx<'_>, id: &str, name: &str) -> Result<Draft, StoreError> {
    let head = ctx.head(MAIN)?;
    Ok(Draft {
        branch: MAIN.into(),
        link: Link::Checkpoint {
            name: name.into(),
            recovers: None,
        },
        intent: tx(id, head.rev, vec![]),
        inverse: vec![],
        bindings: BTreeMap::new(),
        diff: Diff::default(),
        project: head.project.clone(),
    })
}

/// `(txs, record_links, ids, snapshots)` row dumps.
fn rows(s: &Store) -> (i64, i64, Vec<(String, i64)>, i64) {
    let count = |sql: &str| s.conn.query_row(sql, [], |r| r.get(0)).unwrap();
    let mut stmt = s
        .conn
        .prepare("SELECT kind, next FROM ids ORDER BY kind")
        .unwrap();
    let ids = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    (
        count("SELECT COUNT(*) FROM txs"),
        count("SELECT COUNT(*) FROM record_links"),
        ids,
        count("SELECT COUNT(*) FROM snapshots"),
    )
}

fn store() -> (tempfile::TempDir, Store) {
    let tmp = tempfile::tempdir().unwrap();
    let (data, dir) = (tmp.path().join("data"), tmp.path().join("p"));
    let mut s = Store::create(&data, &dir, "Demo", secs(30), None).unwrap();
    s.commit(tx("m1", 0, vec![asset()])).unwrap();
    s.commit(tx("m2", 1, vec![insert("$c", 0, 5)])).unwrap();
    (tmp, s)
}

#[test]
fn write_dry_run_on_follower_plans_and_rolls_back() {
    let (tmp, mut a) = store();
    let mut b = Store::open(&tmp.path().join("data"), &tmp.path().join("p")).unwrap();
    assert_eq!(b.read_only(), Some(ReadOnlyReason::LeaseHeld));
    let before = rows(&b);
    let stale = tx("dry", 1, vec![insert("$y", 0, 2)]);
    assert_eq!(
        b.commit(stale.clone()).unwrap_err().code(),
        "PROJECT_READ_ONLY"
    );

    // A dry run plans the commit on the follower (rebased over rev 2) and writes nothing.
    let planned = b.commit_in(stale, None, true).unwrap();
    assert_eq!((planned.rev, planned.tx.base_rev), (3, 2));
    assert_eq!(planned.bindings["$y"], "c2");
    assert_eq!(rows(&b), before);
    assert_eq!(b.tx_status("dry").unwrap(), TxStatus::Unknown);
    assert_eq!(
        b.heads.borrow()[MAIN].rev,
        2,
        "the cached head is untouched"
    );
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());

    // `write` itself: no epoch, the link comes back, several records stage on one head.
    let out = b
        .write("dry-cp", true, |ctx| {
            assert!(ctx.dry_run);
            assert_eq!(ctx.epoch, None);
            let d = edit(ctx, "dry-cp", vec![insert("$z", 0, 1)])?;
            let first = ctx.append(d)?;
            let d = checkpoint(ctx, "dry-cp-2", "before")?;
            Ok((first, ctx.append(d)?))
        })
        .unwrap()
        .unwrap();
    assert_eq!((out.0.rev, out.1.rev), (3, 4));
    assert_eq!(out.0.link, None);
    assert_eq!(
        out.1.link.as_deref(),
        Some(&Link::Checkpoint {
            name: "before".into(),
            recovers: None,
        })
    );
    assert_eq!(rows(&b), before);
    assert_eq!(b.head(MAIN), 2);

    // Nothing was consumed: the leader's next commit binds the ids the dry runs did.
    let real = a.commit(tx("real", 2, vec![insert("$z", 5, 1)])).unwrap();
    assert_eq!(real.bindings["$z"], "c2");
    // A dry run of a committed tx id is deduped without running.
    assert!(
        b.write("real", true, |_| -> Result<(), StoreError> {
            panic!("deduped")
        })
        .unwrap()
        .is_none()
    );
}

#[test]
fn write_failure_after_apply_rolls_back() {
    let (_tmp, mut s) = store();
    let before = (s.snapshot(MAIN).unwrap(), rows(&s));
    s.fail_next_commit_after_apply();
    // A dry run leaves the hook for the next real write.
    s.commit_in(tx("dry", 2, vec![insert("$x", 5, 1)]), None, true)
        .unwrap();
    let err = s
        .write("pair-1", false, |ctx| {
            let d = edit(ctx, "pair-1", vec![insert("$x", 5, 1)])?;
            ctx.append(d)?;
            let d = checkpoint(ctx, "pair-2", "cp")?;
            ctx.append(d)
        })
        .unwrap_err();
    assert_eq!(err.code(), "TEST_FAULT");
    assert_eq!((s.snapshot(MAIN).unwrap(), rows(&s)), before);
    assert_eq!(s.heads.borrow()[MAIN].rev, 2);

    // Without the fault both land, chained, and the staged head is adopted.
    let epoch = s.lease().unwrap().epoch();
    let (first, second) = s
        .write("pair-1", false, |ctx| {
            assert_eq!(ctx.epoch, Some(epoch));
            let d = edit(ctx, "pair-1", vec![insert("$x", 5, 1)])?;
            let first = ctx.append(d)?;
            assert_eq!(ctx.head(MAIN)?.rev, 3);
            let d = checkpoint(ctx, "pair-2", "cp")?;
            Ok((first, ctx.append(d)?))
        })
        .unwrap()
        .unwrap();
    assert_eq!((first.rev, second.rev), (3, 4));
    let hash_of = |seq: i64, col: &str| -> Option<String> {
        s.conn
            .query_row(
                &format!("SELECT {col} FROM txs WHERE branch = 'main' AND seq = ?1"),
                params![seq],
                |r| r.get(0),
            )
            .unwrap()
    };
    assert_eq!(hash_of(4, "parent_hash"), hash_of(3, "tx_hash"));
    let cached = s.heads.borrow()[MAIN].clone();
    assert_eq!((cached.rev, cached.tip.clone()), (4, hash_of(4, "tx_hash")));
    let mut fresh = std::collections::HashMap::new();
    crate::refresh(&s.conn, &mut fresh, MAIN).unwrap();
    assert_eq!(fresh[MAIN].project, cached.project);
    // The checkpoint keeps its link when read back (Task 12 relies on it).
    let read = s.ops_since(MAIN, 3).unwrap();
    assert_eq!(read, vec![second]);
    // A retry of the first tx id is deduped.
    assert!(
        s.write("pair-1", false, |_| -> Result<(), StoreError> {
            panic!("deduped")
        })
        .unwrap()
        .is_none()
    );
}

/// A dry run takes the write lock up front (IMMEDIATE), so a leader committing beside a
/// follower's dry run waits for it instead of failing the dry run's first write.
#[test]
fn dry_run_holds_the_write_lock_from_the_start() {
    let (_tmp, mut s) = store();
    let path = s.db_path().to_path_buf();
    let other = rusqlite::Connection::open(&path).unwrap();
    other.busy_timeout(std::time::Duration::ZERO).unwrap();
    let planned = s
        .write("dry", true, |ctx| {
            let _ = ctx.head(MAIN)?;
            // Another connection cannot start a write while the dry run plans.
            let err = other.execute_batch("BEGIN IMMEDIATE").unwrap_err();
            assert!(err.to_string().contains("locked"), "{err}");
            let d = edit(ctx, "dry", vec![insert("$x", 5, 1)])?;
            ctx.append(d)
        })
        .unwrap()
        .unwrap();
    assert_eq!(planned.rev, 3);
    // Released after the rollback.
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
}

/// Rows written through the transaction before any append (ids, Task 8's index row and fork
/// snapshot) are uncommitted: heads read after them never reach the shared cache.
#[test]
fn heads_after_uncommitted_rows_stay_out_of_the_cache() {
    let (_tmp, mut s) = store();
    let main_before = s.heads.borrow()[MAIN].clone();
    let (_, at_head) = s.snapshot(MAIN).unwrap();
    let head = s
        .write("dry-branch", true, |ctx| {
            ctx.txn.execute(
                "INSERT INTO branches VALUES ('br1', 'br1', 'main', 2, NULL, NULL, 't', 0, NULL, 0, 0, 0)",
                [],
            )?;
            db_snapshot(ctx, "br1", 2, &at_head)?;
            // Main's cached head is still current in the transaction: used as it is.
            assert_eq!(ctx.head(MAIN)?.rev, 2);
            Ok(ctx.head("br1")?.clone())
        })
        .unwrap()
        .unwrap();
    assert_eq!((head.rev, head.project), (2, at_head));
    let cache = s.heads.borrow();
    assert!(!cache.contains_key("br1"), "uncommitted branch head cached");
    assert_eq!(cache[MAIN].tip, main_before.tip);
}

fn db_snapshot(
    ctx: &mut WriteCtx<'_>,
    branch: &str,
    seq: u64,
    p: &fin_core::Project,
) -> Result<(), StoreError> {
    crate::db::insert_snapshot(ctx.txn, branch, seq, p)
}

/// The note is the last write's: a retry loop that is deduped, then writes again (its record
/// gone, rewound by an import between the two reads) reports `Written`.
#[test]
fn retry_loop_reports_last_write() {
    use crate::WriteOutcome;
    let (_tmp, mut s) = store();
    let deduped = s.write("m2", false, |_| -> Result<(), StoreError> {
        panic!("deduped")
    });
    assert!(deduped.unwrap().is_none());
    assert_eq!(s.write_outcome, Some(WriteOutcome::Deduped));
    s.conn
        .execute("DELETE FROM txs WHERE tx_id = 'm2'", [])
        .unwrap();
    s.write("m2", false, |_| Ok(())).unwrap().unwrap();
    assert_eq!(s.take_write_outcome(), Some(WriteOutcome::Written));
    let err = s.write("t9", false, |_| -> Result<(), StoreError> {
        Err(fin_core::CoreError::new("TEST_FAULT", "x").into())
    });
    assert!(err.is_err());
    assert_eq!(s.take_write_outcome(), None);
}
