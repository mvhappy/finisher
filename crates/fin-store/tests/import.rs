//! Import (plan Task 8, spec §3.3, §3.6, §3.7): folder segments enter the DB, set-aside
//! records go to quarantine, id counters are raised past every id in the log.
//!
//! Multi-server scenarios use one project folder and several server-data dirs. Each server's
//! DB starts as a copy of the creator's DB at rev 0 (rebuilding a DB from the folder is Task
//! 12), and the test lease hook stands in for the lease (Task 9).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fin_core::{
    CoreError, MemIds, Op, Rational, Record, SCHEMA_VERSION, Tx, VideoInfo, apply_tx,
    encode_record, resolve_ops,
};
use fin_store::chain::ForkReason;
use fin_store::conflict_names::Provider;
use fin_store::names::{SegmentName, hash12};
use fin_store::scan;
use fin_store::segment::{self, Header, Segment};
use fin_store::{FailPoint, MAIN, ReadOnlyReason, Store, db_path};
use rusqlite::Connection;
use tempfile::TempDir;

const S: Duration = Duration::from_secs(1);

struct Env {
    _tmp: TempDir,
    root: PathBuf,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let dir = root.join("My Project");
    Env {
        root,
        dir,
        _tmp: tmp,
    }
}

impl Env {
    fn data(&self, server: &str) -> PathBuf {
        self.root.join(format!("server-{server}"))
    }
}

/// Server `a` creates the project.
fn create(e: &Env) -> Store {
    Store::create(&e.data("a"), &e.dir, "Demo", Rational::from_int(30), None).unwrap()
}

/// Another server whose DB is a copy of `a`'s at rev 0. `a` holds the real lease, so this one
/// would follow (`LEASE_HELD`); these tests are about import, not the lease, so it gets the
/// test lease at epoch 0, as every store had before the lease was wired in (Task 11): it may
/// commit, and its commits are at epoch 0. Tests set other epochs where the epoch matters.
fn server(e: &Env, a: &Store, name: &str) -> Store {
    let to = db_path(&e.data(name), a.project_id());
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    Connection::open(a.db_path())
        .unwrap()
        .execute("VACUUM INTO ?1", [to.to_string_lossy()])
        .unwrap();
    let mut s = Store::open(&e.data(name), &e.dir).unwrap();
    s.set_test_lease(0, true);
    s
}

fn asset(path: &str) -> Op {
    fin_core::AssetAdd::media(
        "$a",
        path,
        Rational::from_int(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
            display: None,
            rotation: None,
            pixel_aspect: None,
        }),
        None,
        "fp",
    )
    .into()
}

fn insert(at: i64, asset: &str) -> Op {
    fin_core::ItemInsert {
        id: "$c".into(),
        track: "t1".into(),
        at: Rational::from_int(at),
        duration: Rational::from_int(1),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: asset.into(),
            source_in: Rational::from_int(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into()
}

fn tx(s: &Store, id: &str, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: s.head(MAIN),
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    }
}

fn commit(s: &mut Store, id: &str, ops: Vec<Op>) -> fin_core::CommittedTx {
    let t = tx(s, id, ops);
    s.commit(t).unwrap()
}

/// Exports everything pending now (the 2 s delay passed).
fn export(s: &mut Store) -> u32 {
    let t = Instant::now();
    s.tick_report(t).unwrap();
    let r = s.tick_report(t + 3 * S).unwrap().export;
    assert_eq!(r.error, None);
    r.segments_written
}

fn conn(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

/// `(seq, tx_id, record_bytes, tx_hash, exported, epoch)` of every `main` row.
type Row = (u64, String, Vec<u8>, String, bool, u64);

fn rows(s: &Store) -> Vec<Row> {
    let c = conn(s);
    let mut stmt = c
        .prepare(
            "SELECT seq, tx_id, record_bytes, tx_hash, exported, epoch FROM txs
             WHERE branch = ?1 ORDER BY seq",
        )
        .unwrap();
    stmt.query_map([MAIN], |r| {
        Ok((
            r.get(0)?,
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

/// `(hash, state)` of every `segments` row, by hash.
fn segment_rows(s: &Store) -> Vec<(String, String)> {
    let c = conn(s);
    let mut stmt = c
        .prepare("SELECT hash, state FROM segments ORDER BY hash")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// `(tx_hash, reason, origin, record_bytes, current)` of every quarantine row.
type QRow = (String, String, String, Vec<u8>, bool);

fn quarantine(s: &Store) -> Vec<QRow> {
    let c = conn(s);
    let mut stmt = c
        .prepare(
            "SELECT tx_hash, reason, origin, record_bytes, current FROM quarantine ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn hash(b: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(b).to_hex())
}

fn seg_dir(dir: &Path) -> PathBuf {
    dir.join(".finisher").join("segments").join(MAIN)
}

/// The canonical segment files on `main`, by first seq.
fn segments(dir: &Path) -> Vec<(PathBuf, Segment)> {
    let mut out: Vec<_> = fs::read_dir(seg_dir(dir))
        .map(|rd| {
            rd.map(|e| e.unwrap().path())
                .filter_map(|p| {
                    let name = SegmentName::parse(p.file_name()?.to_str()?)?;
                    let seg = segment::decode(&fs::read(&p).unwrap()).unwrap();
                    Some((name.first_seq(), p, seg))
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|(f, _, _)| *f);
    out.into_iter().map(|(_, p, s)| (p, s)).collect()
}

fn header(s: &Store, epoch: u64, first: u64, last: u64, prev: Option<String>) -> Header {
    Header {
        format_version: 1,
        min_reader_version: 1,
        project_id: s.project_id().into(),
        branch: MAIN.into(),
        writer_id: "someone-else".into(),
        epoch,
        first_seq: first,
        last_seq: last,
        prev_segment_hash: prev,
        op_schema_versions: [1, 1],
        created_at: 0,
        genesis_hash: s.genesis_hash(),
        branch_record: None,
        pack: false,
        replaces: Vec::new(),
    }
}

/// Writes a segment under its canonical name; returns its hash.
fn plant(dir: &Path, h: &Header, records: &[&[u8]]) -> String {
    let (bytes, hash) = segment::encode(h, records);
    let name = SegmentName::new(h.epoch, h.first_seq, h.last_seq, hash12(&hash).unwrap()).unwrap();
    fs::create_dir_all(seg_dir(dir)).unwrap();
    fs::write(seg_dir(dir).join(name.to_string()), bytes).unwrap();
    hash
}

fn code(e: fin_store::StoreError) -> CoreError {
    CoreError::from(e)
}

#[test]
fn second_store_imports_first_stores_segments() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    assert_eq!(export(&mut a), 1);
    assert_ne!(a.server_id(), b.server_id());

    let r = b.import().unwrap();
    assert_eq!(r.applied.len(), 3);
    assert_eq!(
        r.applied,
        a.ops_since(MAIN, 0).unwrap(),
        "same txs, diffs and bindings"
    );
    assert_eq!((r.quarantined, r.fork, r.newer), (0, None, false));
    assert!(r.waiting.is_empty() && r.foreign.is_empty() && r.error.is_none());
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    // The exact bytes and hashes, already exported, and the segment is known as imported.
    let (ra, rb) = (rows(&a), rows(&b));
    assert_eq!(ra.len(), 3);
    for (x, y) in ra.iter().zip(&rb) {
        assert_eq!((x.0, &x.1, &x.2, &x.3), (y.0, &y.1, &y.2, &y.3));
        assert_eq!(y.3, hash(&y.2));
        assert!(y.4, "imported rows are exported");
        assert_eq!(y.5, x.5, "epoch");
    }
    let segs = segments(&e.dir);
    assert_eq!(
        segment_rows(&b),
        [(segs[0].1.hash.clone(), "imported".into())]
    );

    // Nothing new: nothing applied.
    assert!(b.import().unwrap().applied.is_empty());

    // Following: only the new records.
    commit(&mut a, "t4", vec![insert(2, "a1")]);
    export(&mut a);
    let r = b.import().unwrap();
    assert_eq!(r.applied.len(), 1);
    assert_eq!(r.applied[0].tx.tx_id, "t4");
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    assert_eq!(segment_rows(&b).len(), 2);

    // A fresh `Store` on B's DB sees the same.
    drop(b);
    let b = Store::open(&e.data("b"), &e.dir).unwrap();
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
}

/// A hand-written record: a clip half a frame long, which `apply_tx` rejects (policy) and
/// replay accepts. Its bytes are not what `encode_record` writes (a space after a colon), so
/// a re-encode would change the hash.
fn half_frame_record(a: &Store, seq: u64, parent: &str) -> Vec<u8> {
    let half: Rational = "1/60".parse().unwrap();
    let ops = vec![
        fin_core::ItemInsert {
            id: "$x".into(),
            track: "t1".into(),
            at: Rational::from_int(5),
            duration: half,
            mode: fin_core::InsertMode::Ripple,
            body: fin_core::ItemBody::Clip {
                asset: "a1".into(),
                source_in: Rational::from_int(0),
                speed: fin_core::Rational::from_int(1),
            },
        }
        .into(),
    ];
    let bindings = BTreeMap::from([("$x".to_string(), "c2".to_string())]);
    let mut intent = tx(a, "policy", ops.clone());
    intent.base_rev = seq - 1;
    assert_eq!(
        apply_tx(&a.snapshot(MAIN).unwrap().1, &intent, &mut MemIds::new())
            .unwrap_err()
            .code,
        "INVALID_RANGE",
        "the edit path refuses it"
    );
    let resolved = resolve_ops(&ops, &bindings).unwrap();
    let record = Record::new_edit(
        "policy".into(),
        seq,
        SCHEMA_VERSION,
        Some(parent.into()),
        "test".into(),
        1,
        intent,
        resolved,
        vec![],
        bindings,
    );
    let bytes = String::from_utf8(encode_record(&record)).unwrap();
    let odd = bytes.replacen(&format!("\"seq\":{seq}"), &format!("\"seq\": {seq}"), 1);
    assert_ne!(odd, bytes);
    odd.into_bytes()
}

#[test]
fn import_upcasts_and_applies_without_policy() {
    // Schema version 1 has no upcast steps yet: the record goes through `upcast_record` (the
    // same path an old record takes) and keeps its bytes exactly.
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    let tip = rows(&a)[1].3.clone();
    let rec = half_frame_record(&a, 3, &tip);
    plant(&e.dir, &header(&a, 0, 3, 3, Some(prev)), &[&rec]);

    let r = b.import().unwrap();
    assert_eq!(r.applied.len(), 3, "{r:?}");
    assert_eq!(r.applied[2].bindings["$x"], "c2");
    let (_, project) = b.snapshot(MAIN).unwrap();
    let clip = project.timelines[0].tracks[0]
        .items
        .iter()
        .find(|c| c.id == "c2")
        .unwrap();
    assert_eq!(clip.duration, "1/60".parse().unwrap());
    let row = &rows(&b)[2];
    assert_eq!(row.2, rec, "bytes kept exactly");
    assert_eq!(row.3, hash(&rec));
}

/// `x` from server `c` at epoch 1 and `y1` from `a` at epoch 2 both start `main`. `a` has
/// committed nothing yet. Returns `x`'s record bytes.
fn stale_line(e: &Env, a: &mut Store) -> Vec<u8> {
    let mut c = server(e, a, "c");
    c.set_test_lease(1, true);
    commit(&mut c, "x", vec![asset("/m/x.mov")]);
    assert_eq!(export(&mut c), 1);
    let x_bytes = rows(&c)[0].2.clone();
    a.set_test_lease(2, true);
    commit(a, "y1", vec![asset("/m/y.mov")]);
    // A's tail (epoch 2) beats x (epoch 1): x is set aside before A exports.
    let r = a.import().unwrap();
    assert_eq!((r.quarantined, r.applied.len(), r.fork), (1, 0, None));
    assert_eq!(export(a), 1);
    x_bytes
}

#[test]
fn quarantine_counts_and_keeps_bytes() {
    let e = env();
    let mut a = create(&e);
    let mut f = server(&e, &a, "f");
    let x_bytes = stale_line(&e, &mut a);

    let r = f.import().unwrap();
    assert_eq!(r.applied.len(), 1);
    assert_eq!(r.applied[0].tx.tx_id, "y1");
    assert_eq!(r.quarantined, 1);
    let q = quarantine(&f);
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].0, hash(&x_bytes));
    assert_eq!(q[0].1, "STALE_EPOCH");
    assert_eq!(q[0].2, "folder");
    assert_eq!(q[0].3, x_bytes, "the bytes are kept");
    assert!(q[0].4, "current");

    // The stale line is reported again on every poll; it stays one row.
    for i in 2..5 {
        commit(&mut a, &format!("y{i}"), vec![insert(i, "a1")]);
        export(&mut a);
        let r = f.import().unwrap();
        assert_eq!((r.applied.len(), r.quarantined), (1, 1), "poll {i}");
        assert_eq!(quarantine(&f).len(), 1, "poll {i}");
    }
    assert_eq!(f.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
}

#[test]
fn ids_never_reused_after_takeover() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "asset", vec![asset("/m/a.mov")]);
    for i in 0..5 {
        commit(&mut a, &format!("c{i}"), vec![insert(i, "a1")]);
    }
    // c5 is deleted: the snapshot no longer holds it, the log still does.
    commit(
        &mut a,
        "del",
        vec![
            fin_core::ItemDelete {
                item: "c5".into(),
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
        ],
    );
    export(&mut a);
    assert_eq!(b.import().unwrap().applied.len(), 7);
    let c = commit(&mut b, "b1", vec![asset("/m/b.mov"), insert(10, "$a")]);
    assert_eq!(c.bindings["$c"], "c6");
    assert_eq!(c.bindings["$a"], "a2");
    let next: i64 = conn(&b)
        .query_row("SELECT next FROM ids WHERE kind = 'branch'", [], |r| {
            r.get(0)
        })
        .unwrap_or(1);
    assert_eq!(next, 1, "no branch ids yet");
}

#[test]
fn branch_counter_is_raised_by_branch_names_in_the_log() {
    let e = env();
    let a = create(&e);
    let b = server(&e, &a, "b");
    // A 4B branch in the DB (only its name matters here).
    let c = conn(&b);
    c.execute(
        "INSERT INTO snapshots (branch, seq, json) SELECT 'br4', seq, json FROM snapshots WHERE branch = 'main'",
        [],
    )
    .unwrap();
    drop(c);
    drop(b);
    // The first pass of a store (here: the one inside open) raises every counter.
    let b = Store::open(&e.data("b"), &e.dir).unwrap();
    let next: i64 = conn(&b)
        .query_row("SELECT next FROM ids WHERE kind = 'branch'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(next, 5);
}

#[test]
fn newer_segment_makes_read_only_newer_format() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    let mut h = header(&a, 0, 2, 2, Some(prev.clone()));
    // 5E reads formats 1 to 3: 4 is newer.
    h.format_version = 4;
    h.min_reader_version = 4;
    let path = {
        let (bytes, hash) = segment::encode(&h, &[b"{\"future\":true}".as_slice()]);
        let name = SegmentName::new(0, 2, 2, hash12(&hash).unwrap()).unwrap();
        let p = seg_dir(&e.dir).join(name.to_string());
        fs::write(&p, bytes).unwrap();
        p
    };
    let r = b.import().unwrap();
    assert!(r.newer);
    assert_eq!(
        r.applied.len(),
        1,
        "what this Finisher can read is imported"
    );
    assert_eq!(b.read_only(), Some(ReadOnlyReason::NewerFormat));
    let t = tx(&b, "b1", vec![asset("/m/b.mov")]);
    let err = code(b.commit(t.clone()).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY");
    assert_eq!(err.details["reason"], "NEWER_FORMAT");
    assert!(err.fix.is_some());

    // Verdicts are per poll: once the file is gone, the project is writable again.
    fs::remove_file(&path).unwrap();
    let r = b.import().unwrap();
    assert!(!r.newer);
    assert_eq!(b.read_only(), None);
    b.commit(t).unwrap();
}

#[test]
fn a_record_with_a_newer_schema_is_newer_format_and_stops_there() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    let tip = rows(&a)[0].3.clone();
    let rec = format!(
        r#"{{"txId":"future","seq":2,"kind":"edit","schemaVersion":{},"parentHash":"{tip}"}}"#,
        SCHEMA_VERSION + 1
    );
    plant(&e.dir, &header(&a, 0, 2, 2, Some(prev)), &[rec.as_bytes()]);
    let r = b.import().unwrap();
    assert!(r.newer, "{r:?}");
    assert_eq!(r.applied.len(), 1);
    assert_eq!(b.head(MAIN), 1);
    assert_eq!(b.read_only(), Some(ReadOnlyReason::NewerFormat));
}

#[test]
fn placeholder_hydrates_then_applies() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let (path, _) = segments(&e.dir).remove(0);
    scan::mark_placeholder(&path);

    let reads = scan::reads();
    let r = b.import().unwrap();
    assert!(r.applied.is_empty());
    assert_eq!(r.hydrated, std::slice::from_ref(&path));
    assert_eq!(scan::reads(), reads, "a hydration read is not a decode");
    // The next pass sees the hydrated file.
    let r = b.import().unwrap();
    assert!(r.hydrated.is_empty());
    assert_eq!(r.applied.len(), 1);
    scan::clear_placeholders();
}

#[test]
fn crash_after_rename_is_adopted_by_import() {
    let e = env();
    let mut a = create(&e);
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    a.fail_next_export_at(FailPoint::AfterRename);
    let t = Instant::now();
    a.tick_report(t).unwrap();
    assert!(a.tick(t + 3 * S).is_err());
    assert!(!rows(&a)[1].4, "not marked");

    let r = a.import().unwrap();
    assert!(r.applied.is_empty() && r.quarantined == 0 && r.fork.is_none());
    assert!(
        rows(&a).iter().all(|r| r.4),
        "marked exported, never re-applied"
    );
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 2);
    let known = segment_rows(&a);
    assert!(
        known.contains(&(segs[1].1.hash.clone(), "exported".into())),
        "{known:?}"
    );
    assert_eq!(export(&mut a), 0, "nothing left to write");
    // The chain continues from the adopted file.
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    export(&mut a);
    let segs = segments(&e.dir);
    assert_eq!(
        segs[2].1.header.prev_segment_hash,
        Some(segs[1].1.hash.clone())
    );
}

#[test]
fn diverged_tail_moves_to_quarantine_and_ids_are_not_reused() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    // B committed at epoch 1 and lost the lease before exporting.
    b.set_test_lease(1, true);
    commit(&mut b, "b-asset", vec![asset("/m/b.mov")]);
    commit(
        &mut b,
        "b-clips",
        vec![insert(0, "a1"), {
            let mut op = insert(1, "a1");
            if let Op::ItemInsert(i) = &mut op {
                i.id = "$d".into();
            }
            op
        }],
    );
    b.set_test_lease(1, false);
    // The client retried "b-clips" against A after failover: same txId, other bytes.
    a.set_test_lease(2, true);
    commit(&mut a, "a-asset", vec![asset("/m/a.mov")]);
    commit(&mut a, "b-clips", vec![insert(0, "a1")]);
    export(&mut a);
    let tail: Vec<Vec<u8>> = rows(&b).into_iter().map(|r| r.2).collect();

    let r = b.import().unwrap();
    assert_eq!(r.fork, None, "the retried txId is not a reuse");
    assert_eq!(r.applied.len(), 2);
    assert_eq!(r.quarantined, 2);
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    let q = quarantine(&b);
    assert_eq!(q.len(), 2);
    for (row, bytes) in q.iter().zip(&tail) {
        assert_eq!(
            (&row.1, &row.2, &row.3, row.4),
            (
                &"DIVERGED_TAIL".to_string(),
                &"local".to_string(),
                bytes,
                true
            )
        );
    }
    // The set-aside tail stays counted on later polls.
    assert_eq!(b.import().unwrap().quarantined, 2);
    // B's tail allocated a1, c1 and c2; A's line a1 and c1. Nothing is handed out again.
    b.set_test_lease(3, true);
    let c = commit(&mut b, "b-next", vec![asset("/m/c.mov"), insert(5, "$a")]);
    assert_eq!(c.bindings["$a"], "a2");
    assert_eq!(c.bindings["$c"], "c3");
}

#[test]
fn diverged_tail_drops_a_db_snapshot_inside_it() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    b.set_test_lease(1, true);
    for i in 0..fin_store::SNAPSHOT_EVERY {
        commit(
            &mut b,
            &format!("b{i}"),
            vec![asset(&format!("/m/b{i}.mov"))],
        );
    }
    b.set_test_lease(1, false);
    a.set_test_lease(2, true);
    commit(&mut a, "a1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let r = b.import().unwrap();
    assert_eq!(
        (r.applied.len(), r.quarantined),
        (1, fin_store::SNAPSHOT_EVERY as u32)
    );
    let snaps: i64 = conn(&b)
        .query_row("SELECT COUNT(*) FROM snapshots WHERE seq > 0", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(snaps, 0);
    drop(b);
    let b = Store::open(&e.data("b"), &e.dir).unwrap();
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
}

#[test]
fn equal_epoch_lines_fork_and_block_commits() {
    let e = env();
    let a = create(&e);
    let mut c = server(&e, &a, "c");
    let mut d = server(&e, &a, "d");
    let mut f = server(&e, &a, "f");
    c.set_test_lease(1, true);
    commit(&mut c, "x", vec![asset("/m/x.mov")]);
    export(&mut c);
    let x_path = segments(&e.dir).remove(0).0;
    d.set_test_lease(1, true);
    commit(&mut d, "z", vec![asset("/m/z.mov")]);
    // D exports without importing first, as a buggy or copied server would (`close` flushes
    // without a poll; `tick` would import x first and see the fork).
    d.close().unwrap();

    let r = f.import().unwrap();
    let fork = r.fork.expect("a fork");
    assert_eq!(fork.reason, ForkReason::EqualEpoch);
    assert!(r.applied.is_empty());
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ChainFork));
    let t = tx(&f, "f1", vec![asset("/m/f.mov")]);
    let err = code(f.commit(t).unwrap_err());
    assert_eq!(
        (err.code.as_str(), &err.details["reason"]),
        ("PROJECT_READ_ONLY", &serde_json::json!("CHAIN_FORK"))
    );
    // A fork blocks export too: nothing more lands in the folder from here.
    let r = f.import().unwrap();
    assert!(r.fork.is_some(), "re-reported");

    // One side removed by hand: the fork is gone on the next poll.
    fs::remove_file(x_path).unwrap();
    let r = f.import().unwrap();
    assert_eq!(r.fork, None);
    assert_eq!(r.applied.len(), 1);
    assert_eq!(f.read_only(), None);
}

#[test]
fn conflict_copy_duplicates_are_harmless_and_unknown_records_are_quarantined() {
    let e = env();
    let mut a = create(&e);
    let mut f = server(&e, &a, "f");
    let mut c = server(&e, &a, "c");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    export(&mut a);
    f.import().unwrap();
    let (path, _) = segments(&e.dir).remove(0);
    let copy = path.with_file_name(format!(
        "{} (conflicted copy 2026-11-04).fseg",
        path.file_stem().unwrap().to_str().unwrap()
    ));
    fs::copy(&path, &copy).unwrap();
    let r = f.import().unwrap();
    assert_eq!(r.foreign.len(), 1);
    let ff = &r.foreign[0];
    assert_eq!(ff.path, copy);
    assert_eq!(ff.conflict_copy, Some(Provider::Dropbox));
    assert_eq!((ff.known, ff.quarantined), (2, 0));
    assert_eq!(r.quarantined, 0);
    assert!(quarantine(&f).is_empty(), "known records are not set aside");

    // A copy holding records nobody exported.
    c.set_test_lease(0, true);
    commit(&mut c, "u1", vec![asset("/m/u.mov")]);
    commit(&mut c, "u2", vec![insert(0, "a1")]);
    c.set_test_lease(0, false);
    let recs: Vec<Vec<u8>> = rows(&c).into_iter().map(|r| r.2).collect();
    let lines: Vec<&[u8]> = recs.iter().map(Vec::as_slice).collect();
    let (bytes, _) = segment::encode(&header(&c, 0, 1, 2, None), &lines);
    let other =
        seg_dir(&e.dir).join("00000000-000000000001-000000000002-abc.sync-conflict-20261104.fseg");
    fs::write(&other, bytes).unwrap();
    let r = f.import().unwrap();
    assert_eq!(r.quarantined, 2);
    let q = quarantine(&f);
    assert_eq!(q.len(), 2);
    assert!(
        q.iter()
            .all(|r| r.1 == "CONFLICT_COPY" && r.2 == "foreign" && r.4)
    );
    assert_eq!(q.iter().map(|r| r.3.clone()).collect::<Vec<_>>(), recs);
    let ff = r.foreign.iter().find(|f| f.path == other).unwrap();
    assert_eq!((ff.known, ff.quarantined), (0, 2));
    assert_eq!(f.read_only(), Some(ReadOnlyReason::ConflictCopy));
    // Dedupe across polls.
    assert_eq!(f.import().unwrap().quarantined, 2);
    assert_eq!(quarantine(&f).len(), 2);

    // Gone from the folder: no longer counted, the bytes are kept.
    fs::remove_file(&other).unwrap();
    let r = f.import().unwrap();
    assert_eq!(r.quarantined, 0);
    assert_eq!(
        f.read_only(),
        Some(ReadOnlyReason::ConflictCopy),
        "the Dropbox copy is still there"
    );
    let q = quarantine(&f);
    assert_eq!(q.len(), 2);
    assert!(q.iter().all(|r| !r.4));
}

#[test]
fn resolve_is_skipped_when_nothing_changed() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    b.import().unwrap();
    // The pass after an import resolves once more against the new DB state.
    b.import().unwrap();
    let n = b.resolve_calls();
    for _ in 0..3 {
        let r = b.import().unwrap();
        assert!(r.applied.is_empty());
    }
    assert_eq!(b.resolve_calls(), n, "no folder or DB change, no resolve");
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    export(&mut a);
    assert_eq!(b.import().unwrap().applied.len(), 1);
    assert!(b.resolve_calls() > n);
    // A commit on B changes the DB tail: resolved again.
    let m = {
        b.import().unwrap();
        b.resolve_calls()
    };
    commit(&mut b, "b1", vec![insert(3, "a1")]);
    b.import().unwrap();
    assert_eq!(b.resolve_calls(), m + 1);
}

#[test]
fn follower_imports_across_a_pack() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    export(&mut a);
    b.import().unwrap();
    assert_eq!(b.head(MAIN), 2);
    commit(&mut a, "t3", vec![insert(1, "a1")]);
    commit(&mut a, "t4", vec![insert(2, "a1")]);
    export(&mut a);
    commit(&mut a, "t5", vec![insert(3, "a1")]);
    export(&mut a);
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 3);
    // Compaction by hand: a pack replaces the first two segments, which are then deleted.
    let records: Vec<&[u8]> = segs[0]
        .1
        .records
        .iter()
        .chain(&segs[1].1.records)
        .map(Vec::as_slice)
        .collect();
    let mut h = header(&a, 0, 1, 4, None);
    h.pack = true;
    h.replaces = segs[..2]
        .iter()
        .map(|s| fin_store::segment::Replaced {
            hash: s.1.hash.clone(),
            last_tx_hash: format!(
                "blake3:{}",
                blake3::hash(s.1.records.last().unwrap()).to_hex()
            ),
        })
        .collect();
    let pack = plant(&e.dir, &h, &records);
    fs::remove_file(&segs[0].0).unwrap();
    fs::remove_file(&segs[1].0).unwrap();

    // B's head (t2) is in the middle of the pack: it resumes inside it.
    let r = b.import().unwrap();
    assert_eq!(r.fork, None);
    assert_eq!(
        r.applied
            .iter()
            .map(|c| c.tx.tx_id.as_str())
            .collect::<Vec<_>>(),
        ["t3", "t4", "t5"]
    );
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    let known = segment_rows(&b);
    assert!(known.contains(&(pack, "imported".into())), "{known:?}");
    assert!(known.contains(&(segs[2].1.hash.clone(), "imported".into())));
    // B takes over and continues the chain from t5's segment.
    b.set_test_lease(1, true);
    commit(&mut b, "b1", vec![insert(4, "a1")]);
    assert_eq!(export(&mut b), 1);
    let last = segments(&e.dir).pop().unwrap().1;
    assert_eq!(last.header.prev_segment_hash, Some(segs[2].1.hash.clone()));
}

#[test]
fn an_import_that_replaces_a_tail_invalidates_another_store_on_the_db() {
    let e = env();
    let mut a = create(&e);
    let mut b1 = server(&e, &a, "b");
    let mut b2 = Store::open(&e.data("b"), &e.dir).unwrap();
    // B1 commits two rows, then loses the lease; B2 (same DB) caches them as its head.
    b1.set_test_lease(1, true);
    commit(&mut b1, "b-1", vec![asset("/m/b.mov")]);
    commit(&mut b1, "b-2", vec![insert(0, "a1")]);
    b1.set_test_lease(1, false);
    assert_eq!(b2.snapshot(MAIN).unwrap(), b1.snapshot(MAIN).unwrap());
    // A commits two other rows at epoch 2 and exports: same number of rows.
    a.set_test_lease(2, true);
    commit(&mut a, "a-1", vec![asset("/m/a.mov")]);
    commit(&mut a, "a-2", vec![insert(3, "a1")]);
    export(&mut a);
    let r = b1.import().unwrap();
    assert_eq!((r.applied.len(), r.quarantined), (2, 2));
    assert_eq!(b1.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());

    assert_eq!(
        b2.snapshot(MAIN).unwrap(),
        b1.snapshot(MAIN).unwrap(),
        "B2 sees the import"
    );
    // An insert at 0 ripples what is there, so its diff depends on the head it applies to.
    b2.set_test_lease(2, true);
    let c = commit(&mut b2, "b2-1", vec![insert(0, "a1")]);
    let after = b2.snapshot(MAIN).unwrap();
    drop((b1, b2));
    let fresh = Store::open(&e.data("b"), &e.dir).unwrap();
    assert_eq!(fresh.snapshot(MAIN).unwrap(), after, "replays identically");
    assert_eq!(fresh.ops_since(MAIN, 2).unwrap(), [c]);
}

/// A record at `seq` after `parent` holding `ops` with no bindings.
fn raw_record(s: &Store, tx_id: &str, seq: u64, parent: &str, ops: Vec<Op>) -> Vec<u8> {
    let mut intent = tx(s, tx_id, ops.clone());
    intent.base_rev = seq - 1;
    encode_record(&Record::new_edit(
        tx_id.into(),
        seq,
        SCHEMA_VERSION,
        Some(parent.into()),
        "test".into(),
        1,
        intent,
        ops,
        vec![],
        BTreeMap::new(),
    ))
}

#[test]
fn a_record_that_cannot_be_replayed_blocks_writes_until_it_is_gone() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    let bad = raw_record(
        &a,
        "bad",
        2,
        &rows(&a)[0].3,
        vec![
            fin_core::ItemDelete {
                item: "c9".into(),
                mode: fin_core::DeleteMode::Ripple,
            }
            .into(),
        ],
    );
    let path = {
        plant(&e.dir, &header(&a, 0, 2, 2, Some(prev)), &[&bad]);
        segments(&e.dir).pop().unwrap().0
    };
    // A follower and the writer itself (whose lease the record continues) both stop.
    for s in [&mut b, &mut a] {
        let r = s.import().unwrap();
        assert_eq!(
            r.error.as_ref().map(|e| e.code.as_str()),
            Some("REPLAY_FAILED")
        );
        assert_eq!(s.head(MAIN), 1);
        assert_eq!(s.read_only(), Some(ReadOnlyReason::ImportError));
        let t = tx(s, "next", vec![asset("/m/n.mov")]);
        let err = code(s.commit(t).unwrap_err());
        assert_eq!(err.code, "PROJECT_READ_ONLY");
        assert_eq!(err.details["reason"], "IMPORT_ERROR");
        assert!(err.fix.is_some());
    }
    assert_eq!(
        rows(&a).len(),
        1,
        "nothing committed past the unreadable record"
    );

    fs::remove_file(path).unwrap();
    for s in [&mut b, &mut a] {
        let r = s.import().unwrap();
        assert_eq!((r.error, s.read_only()), (None, None));
    }
    commit(&mut a, "next", vec![asset("/m/n.mov")]);
}

#[test]
fn literal_ids_in_imported_records_raise_the_counters() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    // Replay accepts literal ids: a7 is added, then removed, all in one import.
    let mut add = asset("/m/x.mov");
    if let Op::AssetAdd(a) = &mut add {
        a.id = "a7".into();
    }
    let r2 = raw_record(&a, "lit-add", 2, &rows(&a)[0].3, vec![add]);
    let r3 = raw_record(
        &a,
        "lit-del",
        3,
        &hash(&r2),
        vec![Op::AssetRemove { id: "a7".into() }],
    );
    plant(&e.dir, &header(&a, 0, 2, 3, Some(prev)), &[&r2, &r3]);
    let r = b.import().unwrap();
    assert_eq!((r.applied.len(), r.error), (3, None));
    assert!(!b.snapshot(MAIN).unwrap().1.assets.contains_key("a7"));
    let c = commit(&mut b, "b1", vec![asset("/m/b.mov")]);
    assert_eq!(c.bindings["$a"], "a8");
}

#[test]
fn first_pass_after_open_raises_counters_past_the_snapshot() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    for i in 0..3 {
        commit(&mut a, &format!("c{i}"), vec![insert(i, "a1")]);
    }
    export(&mut a);
    b.import().unwrap();
    drop(b);
    // Counters fell behind (damage, or a DB from before the repair): nothing new to import.
    let c = Connection::open(db_path(&e.data("b"), a.project_id())).unwrap();
    c.execute("UPDATE ids SET next = 1", []).unwrap();
    drop(c);
    let mut b = Store::open(&e.data("b"), &e.dir).unwrap();
    assert!(b.import().unwrap().applied.is_empty());
    b.set_test_lease(0, true);
    let c = commit(&mut b, "b1", vec![asset("/m/b.mov"), insert(9, "$a")]);
    assert_eq!(
        (c.bindings["$a"].as_str(), c.bindings["$c"].as_str()),
        ("a2", "c4")
    );
}

/// B6 (plan 4C Decision 24): the ids nested in a restored item (its effects', its link
/// group's) raise the counters like any literal id in the log.
#[test]
fn counters_see_ids_inside_restores() {
    let e = env();
    let mut a = create(&e);
    let mut b = server(&e, &a, "b");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    export(&mut a);
    let prev = segments(&e.dir)[0].1.hash.clone();
    let r = Rational::from_int;
    let mut item = fin_core::Item::clip("c9", "a1", r(0), r(0), r(1));
    item.common.effects.push(fin_core::Effect {
        id: "fx7".into(),
        kind: "blur".into(),
        enabled: true,
        params: Default::default(),
    });
    item.common.link = Some("lk3".into());
    let restore = Op::from(fin_core::ItemRestore {
        item,
        track: "t1".into(),
        mode: fin_core::DeleteMode::Lift,
    });
    let r2 = raw_record(&a, "restore", 2, &rows(&a)[0].3, vec![restore]);
    let remove = Op::from(fin_core::ItemRemove {
        id: "c9".into(),
        mode: fin_core::DeleteMode::Lift,
    });
    let r3 = raw_record(&a, "remove", 3, &hash(&r2), vec![remove]);
    plant(&e.dir, &header(&a, 0, 2, 3, Some(prev)), &[&r2, &r3]);
    let report = b.import().unwrap();
    assert_eq!((report.applied.len(), report.error), (3, None));
    drop(b);
    let c = Connection::open(db_path(&e.data("b"), a.project_id())).unwrap();
    let next = |kind: &str| -> i64 {
        c.query_row("SELECT next FROM ids WHERE kind = ?1", [kind], |r| r.get(0))
            .unwrap()
    };
    assert_eq!((next("clip"), next("effect"), next("link")), (10, 8, 4));
}
