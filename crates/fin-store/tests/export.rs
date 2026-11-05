//! Export (plan Task 7, spec §3.4 "Export cadence"): committed records go to the project folder
//! as verified, chained, write-once segments.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use fin_core::{Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::chain::{Known, SegRef, resolve};
use fin_store::names::{SegmentName, temp_name};
use fin_store::scan::{Entry, NotArrivedWhy, scan};
use fin_store::segment::{self, Header, MAX_BYTES, MAX_RECORDS, Segment};
use fin_store::storage_kind::StorageKind;
use fin_store::{FailPoint, MAIN, Store, db_path, server_id};
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
    Store::create(&e.data, &e.dir, "Demo", Rational::from_int(30), None).unwrap()
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
        args_hash: None,
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
    assert_eq!(s.tick_report(t0).unwrap().export.segments_written, 0);
    assert_eq!(s.tick_report(t0 + S).unwrap().export.segments_written, 0);
    let r = s.tick_report(t0 + 2 * S).unwrap().export;
    assert_eq!((r.segments_written, r.records, r.error), (1, 1, None));
    assert!(all_exported(&s));
    assert_eq!(
        s.tick_report(t0 + 10 * S).unwrap().export.segments_written,
        0
    );
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
    // The largest record a commit accepts still fits a segment of its own.
    let e = env();
    let mut s = create(&e);
    // Records of several hundred KiB: a handful fill 4 MiB.
    for i in 0..12 {
        commit_ops(&mut s, &format!("m{i}"), vec![sized_op(i, 256)]);
    }
    // A large tx (about 3.7 MiB: the path is in both intent and resolved) goes alone, whole.
    commit_ops(&mut s, "huge", vec![sized_op(100, 1900)]);
    commit(&mut s, "after");
    s.close().unwrap();
    let segs = segments(&e.dir);
    assert_chained(&segs);
    assert!(segs.iter().filter(|(_, g, _)| g.records.len() > 1).count() >= 2);
    for w in segs.windows(2) {
        let ((_, _, len), (_, b, _)) = (&w[0], &w[1]);
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
    assert_eq!(huge.len(), 1, "whole, in one segment");
    assert!(huge[0].2 > 3 << 20);
    for (_, _, len) in &segs {
        assert!(
            *len <= MAX_BYTES as u64,
            "every segment fits the cap: {len}"
        );
    }
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
        assert_eq!(s.tick_report(t).unwrap().export.segments_written, 0);
        assert_eq!(
            s.tick_report(t + 2 * S).unwrap().export.segments_written,
            1,
            "segment {i}"
        );
        t += 3 * S;
    }
    // 31 in the last 10 minutes: now 30 s.
    commit(&mut s, "late");
    assert_eq!(s.tick_report(t).unwrap().export.segments_written, 0);
    assert_eq!(s.tick_report(t + 2 * S).unwrap().export.segments_written, 0);
    assert_eq!(
        s.tick_report(t + 29 * S).unwrap().export.segments_written,
        0
    );
    assert_eq!(
        s.tick_report(t + 30 * S).unwrap().export.segments_written,
        1
    );
    // Ten quiet minutes later the window has emptied: back to 2 s.
    t += 11 * 60 * S;
    commit(&mut s, "quiet");
    assert_eq!(s.tick_report(t).unwrap().export.segments_written, 0);
    assert_eq!(s.tick_report(t + 2 * S).unwrap().export.segments_written, 1);
}

#[test]
fn prev_segment_hash_chains_across_exports_and_reopen() {
    let e = env();
    let mut s = create(&e);
    let t = Instant::now();
    commit(&mut s, "a");
    s.tick_report(t).unwrap();
    s.tick_report(t + 2 * S).unwrap();
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
        s.tick_report(t).unwrap();
        assert!(s.tick(t + 2 * S).is_err(), "{point:?} crashes the export");
        drop(s);
        let temp_bytes: Vec<Vec<u8>> = fs::read_dir(seg_dir(&e.dir))
            .unwrap()
            .map(|d| d.unwrap())
            .filter(|d| d.file_name().to_str().unwrap().starts_with(".~fin-"))
            .map(|d| fs::read(d.path()).unwrap())
            .collect();

        // Reopening imports first: a segment in place (a crash after the rename) holds our
        // tail records with the same txIds and hashes, so import marks them exported from it
        // (spec §3.4); only the crash before the rename leaves them to the export.
        let mut s = Store::open(&e.data, &e.dir).unwrap();
        let renamed = point != FailPoint::AfterTempWrite;
        assert_eq!(all_exported(&s), renamed, "{point:?}: marked by the import");
        let t = Instant::now();
        s.tick_report(t).unwrap();
        let r = s.tick_report(t + 2 * S).unwrap().export;
        assert_eq!(r.error, None, "{point:?}");
        assert_eq!(r.records, if renamed { 0 } else { 2 }, "{point:?}");
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
        if point == FailPoint::AfterTempWrite {
            // The re-export after the crash is byte-identical to the lost attempt.
            let (name, _, _) = &segments(&e.dir)[1];
            let bytes = fs::read(seg_dir(&e.dir).join(name.to_string())).unwrap();
            assert_eq!(temp_bytes, [bytes], "deterministic createdAt");
        }
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
    s.tick_report(t).unwrap();
    assert!(s.tick(t + 2 * S).is_err());
    let first = segments(&e.dir);
    assert_eq!(first.len(), 1);
    // The same Store retries with the same bytes, so the same name. (At t + 3 s no poll is
    // due, so no import marks it first: the export's read-back does.)
    let r = s.tick_report(t + 3 * S).unwrap().export;
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
        genesis_hash: s.genesis_hash(),
        pack: false,
        replaces: Vec::new(),
    };
    let (other, _) = segment::encode(&header, &[b"{\"txId\":\"x\",\"seq\":1}".as_slice()]);
    fs::write(dir.join(&planned), &other).unwrap();
    // Names are content-addressed, so another file under our name is a Finisher file under a
    // name not its own: once listed, import makes the project read-only (CONFLICT_COPY) and
    // nothing is exported. Export's own check matters while the listing cannot show it yet
    // (a directory cache), as here.
    fin_store::scan::hide_for_test(&dir.join(&planned));
    let t = Instant::now();
    s.tick_report(t).unwrap();
    let r = s.tick_report(t + 2 * S).unwrap().export;
    let err = r.error.expect("a collision is reported");
    assert_eq!(err.code, "SEGMENT_COLLISION");
    assert_eq!(
        fs::read(dir.join(&planned)).unwrap(),
        other,
        "never overwritten"
    );
    assert!(!all_exported(&s));
    // The next attempt uses another name.
    let r = s.tick_report(t + 4 * S).unwrap().export;
    assert_eq!((r.segments_written, r.error), (1, None));
    assert!(all_exported(&s));
    assert_eq!(fs::read(dir.join(&planned)).unwrap(), other);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2);
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
    s.tick_report(t).unwrap();
    assert_eq!(s.tick_report(t + 2 * S).unwrap().export.segments_written, 1);
    commit(&mut s, "b");
    let dir = seg_dir(&e.dir);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    s.tick_report(t + 3 * S).unwrap();
    let r = s.tick_report(t + 5 * S).unwrap().export;
    let err = r.error.expect("the failure is reported");
    assert_eq!(err.code, "PROJECT_DIR_UNWRITABLE");
    assert_eq!(
        s.export_error().map(|e| e.code.as_str()),
        Some("PROJECT_DIR_UNWRITABLE")
    );
    assert_eq!(exported_flags(&s), [(1, true), (2, false)]);
    // Not retried on every tick: the next attempt waits the delay again.
    assert_eq!(s.tick_report(t + 6 * S).unwrap().export.segments_written, 0);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    let r = s.tick_report(t + 7 * S).unwrap().export;
    assert_eq!((r.segments_written, r.error), (1, None));
    assert_eq!(s.export_error(), None);
    assert!(all_exported(&s));
    assert_eq!(segments(&e.dir).len(), 2);
    assert_folder_matches_db(&s, &e.dir);
}

#[test]
fn old_temp_files_are_removed() {
    // Age is observed with the monotonic clock passed to `tick`, never by comparing the
    // server's mtime with the local clock: a temp file is removed once it has been seen
    // unchanged for 20 s.
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    s.close().unwrap();
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    let dir = seg_dir(&e.dir);
    let old_mtime = dir.join(temp_name());
    let growing = dir.join(temp_name());
    let not_ours = dir.join(".~fin-notaulid.tmp");
    for p in [&old_mtime, &growing, &not_ours] {
        fs::write(p, b"partial").unwrap();
    }
    let past = SystemTime::now() - Duration::from_secs(3600);
    for p in [&old_mtime, &not_ours] {
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let t = Instant::now();
    s.tick_report(t).unwrap();
    assert!(old_mtime.exists(), "an old mtime alone proves nothing");
    s.tick_report(t + 10 * S).unwrap();
    assert!(old_mtime.exists());
    fs::write(&growing, b"partial, and more").unwrap();
    s.tick_report(t + 21 * S).unwrap();
    assert!(!old_mtime.exists(), "unchanged for 20 s: removed");
    assert!(growing.exists(), "changed: its 20 s start again");
    s.tick_report(t + 31 * S).unwrap();
    assert!(growing.exists());
    s.tick_report(t + 42 * S).unwrap();
    assert!(!growing.exists());
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
        s.tick_report(t).unwrap();
        s.tick_report(t + 2 * S).unwrap();
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

/// Every verified segment in the folder.
fn folder_refs(dir: &Path) -> Vec<SegRef> {
    scan(dir)
        .unwrap()
        .into_iter()
        .filter_map(|e| match e {
            Entry::Segment { seg, .. } => Some(SegRef::new(seg)),
            _ => None,
        })
        .collect()
}

fn tx_ids<'a>(refs: impl IntoIterator<Item = &'a SegRef>) -> Vec<String> {
    refs.into_iter()
        .flat_map(|r| r.txs().iter().map(|t| t.tx_id.clone()))
        .collect()
}

/// Copies a closed store's database to another server data dir (as if that server had
/// imported everything so far).
fn copy_db(from_data: &Path, to_data: &Path, id: &str) {
    let (from, to) = (db_path(from_data, id), db_path(to_data, id));
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    for ext in ["", "-wal", "-shm"] {
        let src = PathBuf::from(format!("{}{ext}", from.display()));
        if src.exists() {
            fs::copy(&src, format!("{}{ext}", to.display())).unwrap();
        }
    }
}

#[test]
fn stale_commit_never_outranks_a_newer_export() {
    // B commits t1 at epoch 2 and loses the lease before exporting it; C (epoch 3) exports u.
    // When B opens again, its open imports before anything can be written: t1 (committed at
    // epoch 2) loses to u at 3 and is set aside, never exported. (Before Task 11 the export
    // ran first and labelled t1 with its commit epoch 2, so it lost in the folder instead;
    // `segment_epoch_is_the_highest_commit_epoch` keeps that labelling covered.)
    let e = env();
    let c_data = e.data.with_file_name("server-c");
    let mut b = create(&e);
    b.set_test_lease(1, true);
    commit(&mut b, "t0");
    let id = b.project_id().to_string();
    b.close().unwrap();
    copy_db(&e.data, &c_data, &id);

    let mut b = Store::open(&e.data, &e.dir).unwrap();
    b.set_test_lease(2, true);
    commit(&mut b, "t1");
    b.set_test_lease(2, false);
    assert_eq!(
        b.close().unwrap().segments_written,
        0,
        "no lease, no export"
    );

    let mut c = Store::open(&c_data, &e.dir).unwrap();
    c.set_test_lease(3, true);
    commit(&mut c, "u");
    assert_eq!(c.close().unwrap().segments_written, 1);

    let mut b = Store::open(&e.data, &e.dir).unwrap();
    let r = resolve(&folder_refs(&e.dir), &known(&b));
    assert!(!r.local_tail_diverged && r.fork.is_none(), "{r:?}");
    let q = b.status().quarantine;
    assert_eq!(q.reasons.get("DIVERGED_TAIL"), Some(&1), "{q:?}");
    b.set_test_lease(4, true);
    assert_eq!(
        b.close().unwrap().segments_written,
        0,
        "t1 is never exported"
    );
    let epochs: Vec<u64> = segments(&e.dir).iter().map(|(n, _, _)| n.epoch()).collect();
    assert_eq!(epochs, [1, 3]);

    // A fresh follower applies u; nothing to set aside.
    let r = resolve(&folder_refs(&e.dir), &Known::empty(StorageKind::Local));
    assert_eq!(r.fork, None);
    assert_eq!(tx_ids(&r.apply), ["t0", "u"]);
    assert!(r.quarantine.is_empty());
}

#[test]
fn segment_epoch_is_the_highest_commit_epoch() {
    let e = env();
    let mut s = create(&e);
    s.set_test_lease(2, true);
    commit(&mut s, "a");
    s.set_test_lease(4, true);
    commit(&mut s, "b");
    s.set_test_lease(5, true);
    assert_eq!(s.close().unwrap().segments_written, 1);
    let segs = segments(&e.dir);
    assert_eq!(segs.len(), 1);
    assert_eq!((segs[0].0.epoch(), segs[0].1.header.epoch), (4, 4));
}

#[test]
fn oversized_tx_is_refused_at_commit() {
    let e = env();
    let mut s = create(&e);
    let base_rev = s.head(MAIN);
    let err = s
        .commit(Tx {
            tx_id: "huge".into(),
            author: "test".into(),
            branch: MAIN.into(),
            base_rev,
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![sized_op(1, 1100), sized_op(2, 1100)],
        })
        .unwrap_err();
    let err = fin_core::CoreError::from(err);
    assert_eq!(err.code, "TX_TOO_LARGE");
    assert_eq!(err.details["txId"], "huge");
    assert!(err.fix.is_some());
    assert_eq!(s.head(MAIN), base_rev, "nothing committed");
    commit(&mut s, "fine");
    assert_eq!(s.head(MAIN), base_rev + 1);
}

/// Plants a valid segment at `seq..=seq` on `main` holding `record`, continuing `prev`.
fn plant(s: &Store, dir: &Path, seq: u64, prev: Option<String>, record: &[u8]) -> String {
    let header = Header {
        format_version: 1,
        min_reader_version: 1,
        project_id: s.project_id().into(),
        branch: MAIN.into(),
        writer_id: "someone-else".into(),
        epoch: 0,
        first_seq: seq,
        last_seq: seq,
        prev_segment_hash: prev,
        op_schema_versions: [1, 1],
        created_at: 0,
        genesis_hash: s.genesis_hash(),
        pack: false,
        replaces: Vec::new(),
    };
    let (bytes, hash) = segment::encode(&header, &[record]);
    let name = SegmentName::new(0, seq, seq, fin_store::names::hash12(&hash).unwrap())
        .unwrap()
        .to_string();
    fs::write(seg_dir(dir).join(&name), bytes).unwrap();
    name
}

fn record_bytes(s: &Store, seq: u64) -> Vec<u8> {
    conn(s)
        .query_row(
            "SELECT record_bytes FROM txs WHERE branch = ?1 AND seq = ?2",
            params![MAIN, seq],
            |r| r.get(0),
        )
        .unwrap()
}

/// `a` exported; `b` committed, not exported. Returns the hash of `a`'s segment.
fn one_exported_one_pending(e: &Env) -> (Store, String) {
    let mut s = create(e);
    commit(&mut s, "a");
    s.close().unwrap();
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    commit(&mut s, "b");
    let prev = segments(&e.dir)[0].1.hash.clone();
    (s, prev)
}

#[test]
fn a_file_with_other_records_is_not_adopted() {
    let e = env();
    let (s, prev) = one_exported_one_pending(&e);
    let planted = plant(&s, &e.dir, 2, Some(prev), br#"{"txId":"other","seq":2}"#);
    let r = s.close().unwrap();
    assert_eq!(
        (r.segments_written, r.error),
        (1, None),
        "written, not adopted"
    );
    let s = Store::open(&e.data, &e.dir).unwrap();
    let ours: Vec<_> = segments(&e.dir)
        .into_iter()
        .filter(|(n, _, _)| n.first_seq() == 2 && n.to_string() != planted)
        .collect();
    assert_eq!(ours.len(), 1);
    assert_eq!(ours[0].1.records, [record_bytes(&s, 2)]);
}

#[test]
fn a_file_from_another_predecessor_is_not_adopted() {
    let e = env();
    let (s, _) = one_exported_one_pending(&e);
    let other_prev = format!("blake3:{}", "1".repeat(64));
    plant(&s, &e.dir, 2, Some(other_prev), &record_bytes(&s, 2));
    let r = s.close().unwrap();
    assert_eq!(
        (r.segments_written, r.error),
        (1, None),
        "written, not adopted"
    );
}

#[test]
fn a_retry_re_encodes_when_the_rows_changed() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    s.planned_segment_name(MAIN).unwrap().unwrap();
    // The rows the cached attempt was built from are not the rows any more.
    let new_bytes = String::from_utf8(record_bytes(&s, 1))
        .unwrap()
        .replace("\"author\":\"test\"", "\"author\":\"tess\"")
        .into_bytes();
    conn(&s)
        .execute(
            "UPDATE txs SET record_bytes = ?1, tx_hash = ?2 WHERE seq = 1",
            params![
                new_bytes,
                format!("blake3:{}", blake3::hash(&new_bytes).to_hex())
            ],
        )
        .unwrap();
    s.close().unwrap();
    assert_eq!(segments(&e.dir)[0].1.records, [new_bytes]);
}

#[test]
fn an_unverifiable_file_under_our_name_is_given_up_after_three_read_backs() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    let planned = s.planned_segment_name(MAIN).unwrap().unwrap();
    let dir = seg_dir(&e.dir);
    fs::create_dir_all(&dir).unwrap();
    let stuck = br#"{"magic":"finisher-segment""#;
    fs::write(dir.join(&planned), stuck).unwrap();
    let t = Instant::now();
    s.tick_report(t).unwrap();
    let mut codes = Vec::new();
    for i in 1..=3 {
        let r = s.tick_report(t + 2 * i * S).unwrap().export;
        assert_eq!(r.segments_written, 0);
        codes.push(r.error.expect("reported").code);
    }
    assert_eq!(
        codes,
        [
            "PROJECT_DIR_UNWRITABLE",
            "PROJECT_DIR_UNWRITABLE",
            "SEGMENT_COLLISION"
        ]
    );
    let r = s.tick_report(t + 8 * S).unwrap().export;
    assert_eq!((r.segments_written, r.error), (1, None), "another name");
    assert_eq!(fs::read(dir.join(&planned)).unwrap(), stuck, "untouched");
    assert!(all_exported(&s));
}

#[test]
fn export_error_clears_when_nothing_is_left() {
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    // A file where the segments directory should be: the export fails.
    fs::create_dir_all(e.dir.join(".finisher").join("segments")).unwrap();
    fs::write(seg_dir(&e.dir), b"not a dir").unwrap();
    let t = Instant::now();
    s.tick_report(t).unwrap();
    assert!(s.tick_report(t + 2 * S).unwrap().export.error.is_some());
    assert!(s.export_error().is_some());
    // Something else exported the records (Task 8's import will).
    conn(&s).execute("UPDATE txs SET exported = 1", []).unwrap();
    s.tick_report(t + 3 * S).unwrap();
    assert_eq!(s.export_error(), None);
}

#[test]
fn an_oversized_stored_record_stops_the_export_instead_of_being_written() {
    // Migration 0001 brings sub-project 2 history in without the commit-time size check.
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    commit(&mut s, "b");
    commit(&mut s, "c");
    // A real record, padded past the limit (it still replays).
    let huge = String::from_utf8(record_bytes(&s, 2)).unwrap().replacen(
        "\"author\":\"test\"",
        &format!("\"author\":\"{}\"", "x".repeat(MAX_BYTES)),
        1,
    );
    assert!(huge.len() > MAX_BYTES);
    conn(&s)
        .execute(
            "UPDATE txs SET record_bytes = ?1 WHERE seq = 2",
            params![huge.as_bytes()],
        )
        .unwrap();
    let r = s.close().unwrap();
    assert_eq!(
        (r.segments_written, r.records),
        (1, 1),
        "the records before it go out"
    );
    let err = r.error.expect("reported");
    assert_eq!(err.code, "RECORD_TOO_LARGE");
    assert_eq!(
        (err.details["txId"].as_str(), err.details["seq"].as_u64()),
        (Some("b"), Some(2))
    );
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(exported_flags(&s), [(1, true), (2, false), (3, false)]);
    for (_, _, len) in segments(&e.dir) {
        assert!(len <= MAX_BYTES as u64);
    }
}

#[cfg(unix)]
#[test]
fn an_unwritable_unreadable_folder_never_escalates_to_a_collision() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("skipped: root ignores directory permissions");
        return;
    }
    let e = env();
    let mut s = create(&e);
    commit(&mut s, "a");
    let dir = seg_dir(&e.dir);
    fs::create_dir_all(&dir).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o000)).unwrap();
    let t = Instant::now();
    s.tick_report(t).unwrap();
    let mut codes = Vec::new();
    for i in 1..=5 {
        codes.push(
            s.tick_report(t + 2 * i * S)
                .unwrap()
                .export
                .error
                .expect("reported")
                .code,
        );
    }
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(codes, vec!["PROJECT_DIR_UNWRITABLE"; 5]);
    let r = s.tick_report(t + 12 * S).unwrap().export;
    assert_eq!((r.segments_written, r.error), (1, None));
}
