//! Commit stores exact record bytes; reopen replays them without re-judging history.

use std::path::PathBuf;

use fin_core::{
    CoreError, Op, Rational, Record, SCHEMA_VERSION, Tx, VideoInfo, encode_record, upcast_record,
};
use fin_store::{MAIN, Store};
use rusqlite::{Connection, params};
use tempfile::TempDir;

struct Env {
    _tmp: TempDir,
    data: PathBuf,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    Env {
        data: tmp.path().join("server-data"),
        dir: tmp.path().join("My Project"),
        _tmp: tmp,
    }
}

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

fn insert(id: &str, at: i64) -> Op {
    Op::ClipInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        asset: "a1".into(),
        source_in: secs(0),
        duration: secs(2),
    }
}

fn hash_of(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

/// `(seq, record_bytes, tx_hash, parent_hash)` of every `main` row.
fn rows(conn: &Connection) -> Vec<(i64, Vec<u8>, String, Option<String>)> {
    let mut stmt = conn
        .prepare("SELECT seq, record_bytes, tx_hash, parent_hash FROM txs ORDER BY seq")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn store_with_three(e: &Env) -> Store {
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    s.commit(tx("tx-1", 0, vec![asset()])).unwrap();
    s.commit(tx("tx-2", 1, vec![insert("$c", 0)])).unwrap();
    s.commit(tx("tx-3", 2, vec![insert("$d", 2)])).unwrap();
    s
}

/// Rewrites row `seq`'s record with `edit`, keeping `tx_hash` consistent with the new bytes.
fn rewrite(conn: &Connection, seq: i64, edit: impl FnOnce(&mut Record)) {
    let bytes: Vec<u8> = conn
        .query_row("SELECT record_bytes FROM txs WHERE seq = ?1", [seq], |r| {
            r.get(0)
        })
        .unwrap();
    let mut rec = upcast_record(serde_json::from_slice(&bytes).unwrap()).unwrap();
    edit(&mut rec);
    let new = encode_record(&rec);
    conn.execute(
        "UPDATE txs SET record_bytes = ?1, tx_hash = ?2 WHERE seq = ?3",
        params![new, hash_of(&new), seq],
    )
    .unwrap();
}

#[test]
fn commit_chains_hashes() {
    let e = env();
    let s = store_with_three(&e);
    let conn = Connection::open(s.db_path()).unwrap();
    let rows = rows(&conn);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].3, None);
    for (i, (_, bytes, hash, parent)) in rows.iter().enumerate() {
        assert_eq!(*hash, hash_of(bytes), "hash of the stored bytes");
        if i > 0 {
            assert_eq!(parent.as_ref(), Some(&rows[i - 1].2));
        }
        let rec = upcast_record(serde_json::from_slice(bytes).unwrap()).unwrap();
        assert_eq!(
            rec.parent_hash, *parent,
            "the record carries the same parent"
        );
    }
}

#[test]
fn idempotent_commit_returns_same_record() {
    let e = env();
    let mut s = store_with_three(&e);
    let again = s.commit(tx("tx-2", 0, vec![insert("$c", 0)])).unwrap();
    assert_eq!(again.rev, 2);
    assert_eq!(s.head(MAIN), 3);
    let conn = Connection::open(s.db_path()).unwrap();
    let rows = rows(&conn);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].0, 2);
    let again2 = s.commit(tx("tx-2", 0, vec![insert("$c", 0)])).unwrap();
    assert_eq!(again2, again);
    assert_eq!(hash_of(&rows[1].1), rows[1].2);
}

#[test]
fn reopen_survives_policy_tightening() {
    let e = env();
    let s = store_with_three(&e);
    let db = s.db_path().to_path_buf();
    drop(s);
    let conn = Connection::open(&db).unwrap();
    // Policy would reject a clip shorter than one frame (1/60 s at 30 fps); structure allows it.
    rewrite(&conn, 3, |rec| {
        let tiny = Rational::new(1, 60).unwrap();
        for op in &mut rec.intent.ops {
            if let Op::ClipInsert { duration, .. } = op {
                *duration = tiny;
            }
        }
        for op in &mut rec.resolved {
            if let Op::ClipInsert { duration, .. } = op {
                *duration = tiny;
            }
        }
    });
    drop(conn);
    let s = Store::open(&e.data, &e.dir).unwrap();
    let (rev, project) = s.snapshot(MAIN).unwrap();
    assert_eq!(rev, 3);
    let items = &project.timelines[0].tracks[0].items;
    assert_eq!(items.len(), 2);
    assert_eq!(items[1].duration, Rational::new(1, 60).unwrap());
}

#[test]
fn reopen_reports_structural_damage() {
    let e = env();
    let s = store_with_three(&e);
    let db = s.db_path().to_path_buf();
    drop(s);
    let conn = Connection::open(&db).unwrap();
    // Tx 3 now deletes a clip that does not exist (hash kept consistent with the new bytes).
    rewrite(&conn, 3, |rec| {
        rec.intent.ops = vec![Op::ClipDelete { clip: "c99".into() }];
        rec.resolved = rec.intent.ops.clone();
        rec.bindings.clear();
    });
    drop(conn);
    let err = CoreError::from(Store::open(&e.data, &e.dir).unwrap_err());
    assert_eq!(err.code, "REPLAY_FAILED");
    assert_eq!(err.details["txId"], "tx-3");
}

#[test]
fn reopen_rejects_a_record_copied_into_another_row() {
    let e = env();
    let s = store_with_three(&e);
    let db = s.db_path().to_path_buf();
    drop(s);
    let conn = Connection::open(&db).unwrap();
    // Row 3 gets row 2's bytes (and hash); its own seq and tx_id columns stay.
    let bytes: Vec<u8> = conn
        .query_row("SELECT record_bytes FROM txs WHERE seq = 2", [], |r| {
            r.get(0)
        })
        .unwrap();
    conn.execute(
        "UPDATE txs SET record_bytes = ?1, tx_hash = ?2 WHERE seq = 3",
        params![bytes, hash_of(&bytes)],
    )
    .unwrap();
    drop(conn);
    let err = CoreError::from(Store::open(&e.data, &e.dir).unwrap_err());
    assert_eq!(err.code, "REPLAY_FAILED");
    assert_eq!(err.details["seq"], 3);
    assert_eq!(err.details["txId"], "tx-3");
    assert_eq!(err.details["recordTxId"], "tx-2");
}
