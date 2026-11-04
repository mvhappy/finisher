//! Export (plan Task 7, spec §3.4 "Export cadence"): committed records go to the project folder
//! as verified, chained, write-once segments.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use fin_core::{Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::chain::{Known, SegRef, resolve};
use fin_store::names::{SegmentName, temp_name};
use fin_store::scan::{Entry, NotArrivedWhy, scan};
use fin_store::segment::{self, Header, MAX_BYTES, MAX_RECORDS, Segment};
use fin_store::storage_kind::StorageKind;
use fin_store::{FailPoint, MAIN, Store, fsio, server_id};
use rusqlite::{Connection, params};
use tempfile::TempDir;

const S: Duration = Duration::from_secs(1);

struct Env {
    _tmp: TempDir,
    data: PathBuf,
    dir: PathBuf,
}

fn env_named(name: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("server-data");
    let dir = tmp.path().join(name);
    Env {
        data,
        dir,
        _tmp: tmp,
    }
}

fn env() -> Env {
    env_named("My Project")
}

fn create(e: &Env) -> Store {
    Store::create(&e.data, &e.dir, "Demo", Rational::from_int(30)).unwrap()
}

fn asset_op(path: &str) -> Op {
    Op::AssetAdd {
        id: "$a".into(),
        path: path.into(),
        duration: Rational::from_int(60),
        video: Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
        }),
        audio: None,
        fingerprint: "fp".into(),
    }
}

/// Commits one tx adding one asset at the head.
fn commit(s: &mut Store, id: &str) {
    commit_ops(s, id, vec![asset_op("/media/a.mov")]);
}

fn commit_ops(s: &mut Store, id: &str, ops: Vec<Op>) {
    let base_rev = s.head(MAIN);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        ops,
    })
    .unwrap();
}

fn seg_dir(dir: &Path) -> PathBuf {
    dir.join(".finisher").join("segments").join(MAIN)
}

/// The segment files on `main`, by first seq.
fn segments(dir: &Path) -> Vec<(SegmentName, Segment, u64)> {
    let mut out: Vec<_> = match fs::read_dir(seg_dir(dir)) {
        Ok(rd) => rd
            .map(|e| e.unwrap())
            .filter_map(|e| {
                let name = SegmentName::parse(e.file_name().to_str()?)?;
                let bytes = fs::read(e.path()).unwrap();
                let seg = segment::decode(&bytes).unwrap();
                Some((name, seg, bytes.len() as u64))
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    out.sort_by_key(|(n, _, _)| n.first_seq());
    out
}

fn conn(s: &Store) -> Connection {
    Connection::open(s.db_path()).unwrap()
}

/// `(seq, exported)` of every `main` row.
fn exported_flags(s: &Store) -> Vec<(u64, bool)> {
    let c = conn(s);
    let mut stmt = c
        .prepare("SELECT seq, exported FROM txs WHERE branch = ?1 ORDER BY seq")
        .unwrap();
    stmt.query_map([MAIN], |r| Ok((r.get::<_, u64>(0)?, r.get::<_, bool>(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Exported rows are a seq prefix of the branch.
fn assert_prefix(s: &Store) {
    let flags = exported_flags(s);
    let n = flags.iter().take_while(|(_, x)| *x).count();
    assert!(
        flags[n..].iter().all(|(_, x)| !x),
        "exported rows are not a prefix: {flags:?}"
    );
}

fn all_exported(s: &Store) -> bool {
    exported_flags(s).iter().all(|(_, x)| *x)
}

/// `Known` for `main`, built the way import (Task 8) builds it.
fn known(s: &Store) -> Known {
    let c = conn(s);
    let mut stmt = c
        .prepare("SELECT tx_id, tx_hash, exported, epoch FROM txs WHERE branch = ?1 ORDER BY seq")
        .unwrap();
    let rows: Vec<(String, String, bool, u64)> = stmt
        .query_map([MAIN], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut k = Known::empty(StorageKind::Local);
    k.holds_lease = true;
    for (id, hash, exported, epoch) in rows {
        k.known_tx.insert(id.clone(), hash.clone());
        if exported {
            k.head = Some(hash);
        } else {
            k.tail.push((id, hash));
            k.tail_epoch = k.tail_epoch.max(epoch);
        }
    }
    if let Some(head) = &k.head {
        let seq: u64 = c
            .query_row(
                "SELECT seq FROM txs WHERE tx_hash = ?1",
                params![head],
                |r| r.get(0),
            )
            .unwrap();
        k.last_hash = c
            .query_row(
                "SELECT hash FROM segments WHERE branch = ?1 AND last_seq = ?2",
                params![MAIN, seq],
                |r| r.get(0),
            )
            .ok();
    }
    k
}

/// Resolution of the scanned folder against the DB shows nothing to do: no fork, nothing to
/// import, mark, set aside or wait for. Every DB record is in exactly one segment file.
fn assert_folder_matches_db(s: &Store, dir: &Path) {
    let refs: Vec<SegRef> = scan(dir)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            Entry::Segment { seg, .. } => Some(SegRef::new(seg)),
            _ => None,
        })
        .collect();
    let r = resolve(&refs, &known(s));
    assert_eq!(r.fork, None);
    assert!(r.apply.is_empty() && r.mark_exported.is_empty() && r.quarantine.is_empty());
    assert!(r.waiting.is_empty() && r.pending.is_empty() && !r.local_tail_diverged);
    let mut count: HashMap<String, u32> = HashMap::new();
    for seg in &refs {
        for t in seg.txs() {
            *count.entry(t.tx_hash.clone()).or_default() += 1;
        }
    }
    let k = known(s);
    assert_eq!(count.len(), k.known_tx.len());
    for hash in k.known_tx.values() {
        assert_eq!(count.get(hash), Some(&1), "{hash} is in one segment");
    }
}

/// The chain links: each segment's `prevSegmentHash` is the previous one's hash, seqs continue.
fn assert_chained(segs: &[(SegmentName, Segment, u64)]) {
    let mut prev: Option<&Segment> = None;
    for (name, seg, _) in segs {
        let h = &seg.header;
        assert_eq!(h.prev_segment_hash, prev.map(|p| p.hash.clone()));
        assert_eq!(h.first_seq, prev.map_or(1, |p| p.header.last_seq + 1));
        assert_eq!(
            (name.first_seq(), name.last_seq()),
            (h.first_seq, h.last_seq)
        );
        assert_eq!(name.epoch(), h.epoch);
        prev = Some(seg);
    }
}

#[test]
fn nothing_is_exported_before_the_delay() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "t1");
    let t0 = Instant::now();
    assert_eq!(s.tick(t0).unwrap().segments_written, 0);
    assert_eq!(s.tick(t0 + S).unwrap().segments_written, 0);
    let r = s.tick(t0 + 2 * S).unwrap();
    assert_eq!((r.segments_written, r.records, r.error), (1, 1, None));
    assert!(all_exported(&s));
    assert_eq!(s.tick(t0 + 10 * S).unwrap().segments_written, 0);
}

#[test]
fn export_writes_capped_segments() {
    let e = env();
    let mut s = create(&e);
    for i in 0..2_500 {
        commit(&mut s, &format!("t{i}"));
    }
    let r = s.close().unwrap();
    assert_eq!((r.segments_written, r.records, r.error), (3, 2_500, None));
    let s = Store::open(&e.data, &e.dir).unwrap();
    let segs = segments(&e.dir);
    let sizes: Vec<_> = segs.iter().map(|(_, g, _)| g.records.len()).collect();
    assert_eq!(sizes, [MAX_RECORDS, MAX_RECORDS, 500]);
    assert_chained(&segs);
    let h = &segs[0].1.header;
    assert_eq!(h.writer_id, server_id(&e.data).unwrap());
    assert_eq!(h.project_id, s.project_id());
    assert_eq!(h.branch, MAIN);
    assert_eq!(h.op_schema_versions, [SCHEMA_VERSION, SCHEMA_VERSION]);
    assert!(all_exported(&s));
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn records_are_exported_byte_for_byte() {
    let e = env();
    let mut s = create(&e);
    for i in 0..5 {
        commit(&mut s, &format!("t{i}"));
    }
    let c = conn(&s);
    let mut stmt = c
        .prepare("SELECT record_bytes FROM txs WHERE branch = ?1 ORDER BY seq")
        .unwrap();
    let rows: Vec<Vec<u8>> = stmt
        .query_map([MAIN], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    s.close().unwrap();
    assert_eq!(segments(&e.dir)[0].1.records, rows);
}

#[test]
fn export_never_splits_a_tx() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "t0");
    // One tx with many ops is one record, so it is never split across segments; together with
    // a full segment's worth of others it starts the next segment.
    for i in 1..MAX_RECORDS {
        commit(&mut s, &format!("t{i}"));
    }
    let many: Vec<Op> = (0..300)
        .map(|i| asset_op(&format!("/media/{i}.mov")))
        .collect();
    let many = many
        .into_iter()
        .enumerate()
        .map(|(i, op)| match op {
            Op::AssetAdd {
                path,
                duration,
                video,
                audio,
                fingerprint,
                ..
            } => Op::AssetAdd {
                id: format!("$a{i}"),
                path,
                duration,
                video,
                audio,
                fingerprint,
            },
            other => other,
        })
        .collect();
    commit_ops(&mut s, "big", many);
    s.close().unwrap();
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0].1.records.len(), MAX_RECORDS);
    assert_eq!(segs[1].1.records.len(), 1);
    let big: serde_json::Value = serde_json::from_slice(&segs[1].1.records[0]).unwrap();
    assert_eq!(big["txId"], "big");
    assert_eq!(big["intent"]["ops"].as_array().unwrap().len(), 300);
}

/// One asset op whose path makes the record about `kib` KiB.
fn sized_op(i: usize, kib: usize) -> Op {
    let mut op = asset_op(&format!("/media/{i}/{}", "x".repeat(kib * 1024)));
    if let Op::AssetAdd { id, .. } = &mut op {
        *id = format!("$a{i}");
    }
    op
}

#[test]
fn export_caps_segments_at_4_mib_and_keeps_a_large_tx_whole() {
    let e = env();
    let mut s = create(&e);
    // Records of several hundred KiB: a handful fill 4 MiB.
    for i in 0..12 {
        commit_ops(&mut s, &format!("m{i}"), vec![sized_op(i, 256)]);
    }
    // One tx larger than the cap goes alone, whole.
    commit_ops(
        &mut s,
        "huge",
        vec![sized_op(100, 1100), sized_op(101, 1100)],
    );
    commit(&mut s, "after");
    s.close().unwrap();
    let segs = segments(&e.dir);
    assert_chained(&segs);
    assert!(segs.iter().filter(|(_, g, _)| g.records.len() > 1).count() >= 2);
    for w in segs.windows(2) {
        let ((_, a, len), (_, b, _)) = (&w[0], &w[1]);
        if a.records.len() > 1 {
            assert!(*len <= MAX_BYTES as u64, "{len}");
        }
        // Greedy: the next record would not have fitted.
        assert!(*len + b.records[0].len() as u64 + 1 > MAX_BYTES as u64);
    }
    let huge: Vec<_> = segs
        .iter()
        .filter(|(_, g, _)| {
            g.records
                .iter()
                .any(|r| serde_json::from_slice::<serde_json::Value>(r).unwrap()["txId"] == "huge")
        })
        .collect();
    assert_eq!(huge.len(), 1);
    assert_eq!(huge[0].1.records.len(), 1);
    assert!(huge[0].2 > MAX_BYTES as u64);
    assert_eq!(
        segs.last().unwrap().1.records.len(),
        1,
        "`after` starts a new segment"
    );
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn export_delay_grows_under_churn() {
    let e = env();
    let mut s = create(&e);
    let mut t = Instant::now();
    // 30 segments in a few minutes: still 2 s.
    for i in 0..=30 {
        commit(&mut s, &format!("t{i}"));
        assert_eq!(s.tick(t).unwrap().segments_written, 0);
        assert_eq!(
            s.tick(t + 2 * S).unwrap().segments_written,
            1,
            "segment {i}"
        );
        t += 3 * S;
    }
    // 31 in the last 10 minutes: now 30 s.
    commit(&mut s, "late");
    assert_eq!(s.tick(t).unwrap().segments_written, 0);
    assert_eq!(s.tick(t + 2 * S).unwrap().segments_written, 0);
    assert_eq!(s.tick(t + 29 * S).unwrap().segments_written, 0);
    assert_eq!(s.tick(t + 30 * S).unwrap().segments_written, 1);
    // Ten quiet minutes later the window has emptied: back to 2 s.
    t += 11 * 60 * S;
    commit(&mut s, "quiet");
    assert_eq!(s.tick(t).unwrap().segments_written, 0);
    assert_eq!(s.tick(t + 2 * S).unwrap().segments_written, 1);
}

#[test]
fn prev_segment_hash_chains_across_exports_and_reopen() {
    let e = env();
    let mut s = create(&e);
    let t = Instant::now();
    commit(&mut s, "a");
    s.tick(t).unwrap();
    s.tick(t + 2 * S).unwrap();
    commit(&mut s, "b");
    commit(&mut s, "c");
    s.close().unwrap();
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    commit(&mut s, "d");
    s.close().unwrap();
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0].1.header.prev_segment_hash, None);
    assert_chained(&segs);
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn crash_at_each_failpoint_recovers_without_fork() {
    for point in [
        FailPoint::AfterTempWrite,
        FailPoint::AfterRename,
        FailPoint::BeforeMarkExported,
    ] {
        let e = env();
        let mut s = create(&e);
        commit(&mut s, "a");
        s.close().unwrap();
        let mut s = Store::open(&e.data, &e.dir).unwrap();
        commit(&mut s, "b");
        commit(&mut s, "c");
        s.fail_next_export_at(point);
        let t = Instant::now();
        s.tick(t).unwrap();
        assert!(s.tick(t + 2 * S).is_err(), "{point:?} crashes the export");
        drop(s);

        let mut s = Store::open(&e.data, &e.dir).unwrap();
        assert!(!all_exported(&s), "{point:?}: nothing was marked");
        let t = Instant::now();
        s.tick(t).unwrap();
        let r = s.tick(t + 2 * S).unwrap();
        assert_eq!(r.error, None, "{point:?}");
        assert_eq!(r.records, 2, "{point:?}");
        assert!(all_exported(&s), "{point:?}");
        assert_prefix(&s);
        assert_eq!(segments(&e.dir).len(), 2, "{point:?}: no duplicate segment");
        assert_folder_matches_db(&s, &e.dir);
        // A crash before the rename leaves a young temp file, which is not ours to delete yet.
        let temps = scan(&e.dir)
            .unwrap()
            .into_iter()
            .filter(|e| {
                matches!(
                    e,
                    Entry::NotArrived {
                        why: NotArrivedWhy::Temp,
                        ..
                    }
                )
            })
            .count();
        assert_eq!(
            temps,
            usize::from(point == FailPoint::AfterTempWrite),
            "{point:?}"
        );
    }
}

#[test]
fn existing_identical_file_counts_as_written() {
    // write_once reports AlreadyExists (or NotFound) after a transient error although our file
    // is in place: the export reads it back and, with the same hash, takes it as written.
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    s.fail_next_export_at(FailPoint::AfterRename);
    let t = Instant::now();
    s.tick(t).unwrap();
    assert!(s.tick(t + 2 * S).is_err());
    let first = segments(&e.dir);
    assert_eq!(first.len(), 1);
    // The same Store retries with the same bytes, so the same name.
    let r = s.tick(t + 10 * S).unwrap();
    assert_eq!((r.records, r.error), (1, None));
    assert_eq!(segments(&e.dir).len(), 1);
    assert!(all_exported(&s));
}

#[test]
fn different_file_under_our_name_is_an_error_and_not_overwritten() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    // Plant a different, valid file under the exact name the next export will use.
    let planned = s.planned_segment_name(MAIN).unwrap().unwrap();
    let dir = seg_dir(&e.dir);
    fs::create_dir_all(&dir).unwrap();
    let header = Header {
        format_version: 1,
        min_reader_version: 1,
        project_id: s.project_id().into(),
        branch: MAIN.into(),
        writer_id: "someone-else".into(),
        epoch: 0,
        first_seq: 1,
        last_seq: 1,
        prev_segment_hash: None,
        op_schema_versions: [1, 1],
        created_at: 0,
        pack: false,
        replaces: Vec::new(),
    };
    let (other, _) = segment::encode(&header, &[b"{\"txId\":\"x\",\"seq\":1}".as_slice()]);
    fs::write(dir.join(&planned), &other).unwrap();
    let r = s.close().unwrap();
    let err = r.error.expect("a collision is reported");
    assert_eq!(err.code, "SEGMENT_COLLISION");
    assert_eq!(
        fs::read(dir.join(&planned)).unwrap(),
        other,
        "never overwritten"
    );
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert!(!all_exported(&s));
}

#[cfg(unix)]
#[test]
fn export_retries_after_unwritable_dir() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root ignores directory permissions");
        return;
    }
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    let t = Instant::now();
    s.tick(t).unwrap();
    assert_eq!(s.tick(t + 2 * S).unwrap().segments_written, 1);
    commit(&mut s, "b");
    let dir = seg_dir(&e.dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    s.tick(t + 3 * S).unwrap();
    let r = s.tick(t + 5 * S).unwrap();
    let err = r.error.expect("the failure is reported");
    assert_eq!(err.code, "PROJECT_DIR_UNWRITABLE");
    assert_eq!(
        s.export_error().map(|e| e.code.as_str()),
        Some("PROJECT_DIR_UNWRITABLE")
    );
    assert_eq!(exported_flags(&s), [(1, true), (2, false)]);
    // Not retried on every tick: the next attempt waits the delay again.
    assert_eq!(s.tick(t + 6 * S).unwrap().segments_written, 0);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let r = s.tick(t + 7 * S).unwrap();
    assert_eq!((r.segments_written, r.error), (1, None));
    assert_eq!(s.export_error(), None);
    assert!(all_exported(&s));
    assert_eq!(segments(&e.dir).len(), 2);
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn old_temp_files_are_removed() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    s.close().unwrap();
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    let dir = seg_dir(&e.dir);
    let old = dir.join(temp_name());
    let young = dir.join(temp_name());
    let not_ours = dir.join(".~fin-notaulid.tmp");
    for p in [&old, &young, &not_ours] {
        fs::write(p, b"partial").unwrap();
    }
    let past = SystemTime::now() - Duration::from_secs(60);
    for p in [&old, &not_ours] {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    s.tick(Instant::now()).unwrap();
    assert!(!old.exists(), "older than 20 s: removed");
    assert!(young.exists(), "may still be in flight");
    assert!(not_ours.exists(), "only our own temp names");
}

#[test]
fn unicode_and_space_folder_exports_ascii_names() {
    let e = env_named("Мой проект 1");
    let mut s = create(&e);
    commit(&mut s, "a");
    commit(&mut s, "b");
    let r = s.close().unwrap();
    assert_eq!((r.segments_written, r.error), (1, None));
    let names: Vec<String> = fs::read_dir(seg_dir(&e.dir))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names.len(), 1);
    assert!(names[0].is_ascii(), "{names:?}");
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn exported_rows_stay_a_prefix() {
    let e = env();
    let mut s = create(&e);
    let mut t = Instant::now();
    for round in 0..4 {
        for i in 0..3 {
            commit(&mut s, &format!("r{round}-{i}"));
            assert_prefix(&s);
        }
        s.tick(t).unwrap();
        s.tick(t + 2 * S).unwrap();
        assert_prefix(&s);
        t += 3 * S;
    }
    assert!(all_exported(&s));
    // A DB whose exported rows are not a prefix is refused, not exported around.
    commit(&mut s, "x");
    commit(&mut s, "y");
    conn(&s)
        .execute("UPDATE txs SET exported = 1 WHERE tx_id = 'y'", [])
        .unwrap();
    let err = s.close().unwrap_err();
    assert_eq!(err.code(), "DB_ERROR");
}

#[test]
fn read_back_is_uncached_and_verifies() {
    // The segment on disk is what `read_uncached` returns and decodes with the name's hash.
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    s.close().unwrap();
    let (name, seg, _) = segments(&e.dir).remove(0);
    let bytes = fsio::read_uncached(&seg_dir(&e.dir).join(name.to_string())).unwrap();
    let back = segment::decode(&bytes).unwrap();
    assert_eq!(back.hash, seg.hash);
    assert_eq!(fin_store::names::hash12(&back.hash), Some(name.hash12()));
    let _ = Arc::new(back);
}
