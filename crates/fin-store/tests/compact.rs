//! Compaction (plan Task 13, spec §3.9): packs replace segments only once they and the
//! chain check out, history stays whole, and only the lease holder ever deletes.

mod integration_support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::SCHEMA_VERSION;
use fin_store::conflict_names::Provider;
use fin_store::names::ManifestName;
use fin_store::segment::{self, Header, Segment};
use fin_store::storage_kind::StorageKind;
use fin_store::{CompactFail, MAIN, ReadOnlyReason, Store, VerifyReport};
use integration_support::*;

const GRACE: Duration = Duration::from_secs(5);

fn short_grace(s: &mut Store) {
    s.set_compaction_grace_for_test(GRACE);
}

/// `n` segments of one record each, txIds `<prefix><i>`.
fn segs(e: &Env, s: &mut Store, prefix: &str, n: usize) {
    for i in 0..n {
        let id = format!("{prefix}{i}");
        commit(s, &id, vec![asset(&format!("/m/{id}.mov"))]);
        assert_eq!(e.flush(s), 1, "one segment for {id}");
    }
}

fn compaction_dir(dir: &Path) -> PathBuf {
    dir.join(".finisher").join("compaction")
}

fn manifests(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(compaction_dir(dir))
        .map(|rd| rd.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    v.sort();
    v
}

fn manifest(dir: &Path) -> serde_json::Value {
    let all = manifests(dir);
    assert_eq!(all.len(), 1, "{all:?}");
    serde_json::from_slice(&fs::read(&all[0]).unwrap()).unwrap()
}

fn packs(dir: &Path) -> Vec<Segment> {
    segments(dir)
        .into_iter()
        .filter(|s| s.2.header.pack)
        .map(|s| s.2)
        .collect()
}

fn originals(dir: &Path) -> Vec<PathBuf> {
    segments(dir)
        .into_iter()
        .filter(|s| !s.2.header.pack)
        .map(|s| s.0)
        .collect()
}

/// Every record in the folder's segments: `txHash → bytes`.
fn records(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    segments(dir)
        .iter()
        .flat_map(|s| s.2.records.clone())
        .map(|r| (format!("blake3:{}", blake3::hash(&r).to_hex()), r))
        .collect()
}

fn snapshots(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir.join(".finisher").join("snapshots").join(MAIN))
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn assert_verifies(r: &VerifyReport) {
    assert!(
        r.ok,
        "{:#?}",
        r.checks.iter().filter(|c| !c.ok).collect::<Vec<_>>()
    );
}

/// The grace period passes (heartbeats keep beating), then one poll.
fn after_grace(e: &Env, s: &mut Store) -> fin_store::CompactReport {
    e.advance(GRACE.as_secs() + 1);
    e.poll(s).compact
}

fn freeze(s: &Store) {
    s.lease().expect("holding").fail_heartbeats(u32::MAX);
}

fn poll_until_held(e: &Env, s: &mut Store) {
    let mut waited = 0;
    while held_epoch(s).is_none() || s.read_only().is_some() {
        assert!(waited <= 120, "not writable: {:?}", s.read_only());
        e.advance(2);
        waited += 2;
        e.poll(s);
    }
}

#[test]
fn compaction_preserves_every_record_and_hash() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 5);
    let before = records(&e.dir);
    let old: Vec<Segment> = segments(&e.dir).into_iter().map(|s| s.2).collect();
    let ops = a.ops_since(MAIN, 0).unwrap();

    let r = a.compact(true).unwrap();
    assert_eq!(
        (r.manifests.len(), r.packs_written, r.replaced),
        (1, 1, 5),
        "{r:?}"
    );
    let p = packs(&e.dir);
    assert_eq!(p.len(), 1);
    let h = &p[0].header;
    assert_eq!((h.first_seq, h.last_seq, h.epoch), (1, 5, 1));
    assert_eq!(h.prev_segment_hash, None);
    assert_eq!(h.genesis_hash, a.genesis_hash());
    let mut hashes: Vec<String> = old.iter().map(|s| s.hash.clone()).collect();
    hashes.sort();
    assert_eq!(h.replaces, hashes);
    let concat: Vec<Vec<u8>> = old.iter().flat_map(|s| s.records.clone()).collect();
    assert_eq!(p[0].records, concat, "byte for byte");
    let m = manifest(&e.dir);
    assert_eq!(m["branch"], MAIN);
    assert_eq!(m["replaced"].as_array().unwrap().len(), 5);
    assert_eq!(m["kind"], "local");
    assert_eq!(snapshots(&e.dir).len(), 1, "a snapshot at the head");
    assert!(snapshots(&e.dir)[0].starts_with("000000000005-"));
    assert_verifies(&a.verify());

    // Not before the grace period.
    e.advance(2);
    assert_eq!(e.poll(&mut a).compact.deleted, 0);
    assert_eq!(originals(&e.dir).len(), 5);

    let r = after_grace(&e, &mut a);
    assert_eq!((r.finished.len(), r.deleted), (1, 5), "{r:?}");
    assert!(originals(&e.dir).is_empty());
    assert!(manifests(&e.dir).is_empty());
    assert_eq!(records(&e.dir), before, "every record, every hash");
    assert_eq!(a.ops_since(MAIN, 0).unwrap(), ops);
    assert_verifies(&a.verify());
    assert!(a.compaction_status().pending.is_empty());
    assert!(a.compaction_status().last_run_ms.is_some());
    // Packs are never re-packed.
    let r = a.compact(true).unwrap();
    assert!(r.manifests.is_empty(), "{r:?}");
}

#[test]
fn fresh_reader_during_grace_window_sees_no_fork() {
    let e = env();
    let mut a = e.create("a");
    segs(&e, &mut a, "t", 4);
    a.compact(true).unwrap();
    let mut g = e.open("g");
    assert_eq!(g.head(MAIN), 4);
    assert_eq!(g.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    assert_eq!(g.read_only(), Some(ReadOnlyReason::LeaseHeld));
    assert!(g.status().waiting.is_empty() && g.status().pending.is_empty());
    assert_verifies(&g.verify());

    // The spec's 10 minutes on a local folder.
    assert_eq!(
        fin_store::compaction_grace(StorageKind::Local),
        Duration::from_secs(600)
    );
    e.advance(9 * 60);
    assert_eq!(e.poll(&mut a).compact.deleted, 0);
    assert_eq!(originals(&e.dir).len(), 4);
    assert_eq!(a.compaction_status().pending[0].state, "grace");
    e.poll(&mut g);
    assert_verifies(&g.verify());
    e.advance(61);
    assert_eq!(e.poll(&mut a).compact.deleted, 4);
    e.poll(&mut g);
    assert_eq!(g.head(MAIN), 4);
    assert_verifies(&g.verify());
}

#[test]
fn fresh_reader_after_deletion_resolves_through_pack() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 4);
    a.compact(true).unwrap();
    after_grace(&e, &mut a);
    assert!(originals(&e.dir).is_empty());
    // A segment exported after the compaction continues from the pack.
    segs(&e, &mut a, "u", 1);
    let next = &segments(&e.dir)[1].2;
    assert_eq!(
        next.header.prev_segment_hash.as_deref(),
        Some(packs(&e.dir)[0].hash.as_str())
    );

    let g = e.open("g");
    assert_eq!(g.head(MAIN), 5);
    assert_eq!(g.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    assert_eq!(g.read_only(), Some(ReadOnlyReason::LeaseHeld));
    assert_verifies(&g.verify());
    assert_verifies(&a.verify());
}

#[test]
fn interrupted_at_each_phase_is_safe() {
    use CompactFail::*;
    for point in [
        AfterSnapshot,
        AfterPack,
        AfterManifest,
        AfterDelete,
        BeforeManifestDelete,
    ] {
        let e = env();
        let mut a = e.create("a");
        short_grace(&mut a);
        segs(&e, &mut a, "t", 4);
        let expected = records(&e.dir);
        if matches!(point, AfterSnapshot | AfterPack | AfterManifest) {
            a.fail_next_compaction_at(point);
            let err = fin_core::CoreError::from(a.compact(true).unwrap_err());
            assert_eq!(err.code, "TEST_FAULT", "{point:?}");
        } else {
            a.compact(true).unwrap();
            e.advance(GRACE.as_secs() + 1);
            a.fail_next_compaction_at(point);
            let err = fin_core::CoreError::from(a.compact(false).unwrap_err());
            assert_eq!(err.code, "TEST_FAULT", "{point:?}");
        }
        // The process dies here. A fresh reader sees the whole history, no fork.
        drop(a);
        let mut g = e.open("g");
        assert_eq!(g.head(MAIN), 4, "{point:?}");
        assert_eq!(g.read_only(), Some(ReadOnlyReason::LeaseHeld), "{point:?}");
        assert_verifies(&g.verify());
        let now = records(&e.dir);
        assert!(
            expected.iter().all(|(h, b)| now.get(h) == Some(b)),
            "{point:?}"
        );

        // The holder restarts and finishes (or redoes) the compaction.
        let mut a = e.open("a");
        short_grace(&mut a);
        assert_eq!(a.read_only(), None, "{point:?}");
        a.compact(true).unwrap();
        let r = after_grace(&e, &mut a);
        assert_eq!(r.finished.len(), 1, "{point:?}: {r:?}");
        assert!(originals(&e.dir).is_empty(), "{point:?}");
        assert!(manifests(&e.dir).is_empty(), "{point:?}");
        assert_eq!(records(&e.dir), expected, "{point:?}");
        assert_verifies(&a.verify());
        e.poll(&mut g);
        assert_eq!(g.head(MAIN), 4);
        assert_verifies(&g.verify());
        // A crash before the manifest left packs that the redo adopted, not doubled.
        assert_eq!(packs(&e.dir).len(), 1, "{point:?}");
    }
}

#[test]
fn follower_never_compacts() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 4);
    let mut f = e.open("f");
    short_grace(&mut f);
    let r = f.compact(true).unwrap();
    assert!(r.skipped.is_some(), "{r:?}");
    assert!(manifests(&e.dir).is_empty() && packs(&e.dir).is_empty());

    a.compact(true).unwrap();
    e.advance(GRACE.as_secs() + 1);
    for _ in 0..3 {
        e.poll(&mut f);
        assert!(f.compact(true).unwrap().skipped.is_some());
    }
    assert_eq!(originals(&e.dir).len(), 4, "a follower deletes nothing");
    assert!(f.compaction_status().pending.is_empty());
    assert_eq!(e.poll(&mut a).compact.deleted, 4);
    e.poll(&mut f);
    assert_eq!(f.head(MAIN), 4);
}

/// A segment holding `record` (seq `seq`) after `prev`, at `epoch`, written into the folder.
fn plant(dir: &Path, s: &Store, prev: &Segment, epoch: u64, seq: u64, record: Vec<u8>) -> PathBuf {
    let header = Header {
        format_version: segment::FORMAT_VERSION,
        min_reader_version: segment::MIN_READER_VERSION,
        project_id: s.project_id().to_string(),
        branch: MAIN.to_string(),
        writer_id: "01jstalewriter000000000000".to_string(),
        epoch,
        first_seq: seq,
        last_seq: seq,
        prev_segment_hash: Some(prev.hash.clone()),
        op_schema_versions: [SCHEMA_VERSION, SCHEMA_VERSION],
        created_at: 0,
        genesis_hash: s.genesis_hash(),
        pack: false,
        replaces: vec![],
    };
    let (bytes, hash) = segment::encode(&header, &[&record]);
    let name = fin_store::names::SegmentName::new(
        epoch,
        seq,
        seq,
        fin_store::names::hash12(&hash).unwrap(),
    )
    .unwrap();
    let path = seg_dir(dir).join(name.to_string());
    fs::write(&path, bytes).unwrap();
    path
}

/// `record` with fields replaced.
fn edited(
    record: &[u8],
    f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> Vec<u8> {
    let mut v: serde_json::Value = serde_json::from_slice(record).unwrap();
    f(v.as_object_mut().unwrap());
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn stale_and_malformed_segments_are_never_deleted() {
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(2, true);
    short_grace(&mut a);
    segs(&e, &mut a, "t", 3);
    let s = segments(&e.dir);
    // A lower-epoch line continuing from t1 (stale), and a file that claims to follow t2's
    // segment but continues t0 (malformed).
    let stale = plant(
        &e.dir,
        &a,
        &s[1].2,
        1,
        3,
        edited(&s[2].2.records[0], |m| {
            m.insert("txId".into(), "stale".into());
        }),
    );
    let bad = plant(
        &e.dir,
        &a,
        &s[2].2,
        2,
        4,
        edited(&s[1].2.records[0], |m| {
            m.insert("txId".into(), "bad".into());
            m.insert("seq".into(), 4.into());
        }),
    );
    e.poll(&mut a);
    let aside = a.quarantined_for_test();
    let mut reasons: Vec<&str> = aside.iter().map(|q| q.1.as_str()).collect();
    reasons.sort();
    assert_eq!(reasons, ["MALFORMED", "STALE_EPOCH"]);
    assert_eq!(a.read_only(), None);
    segs(&e, &mut a, "u", 2);

    let r = a.compact(true).unwrap();
    assert_eq!(r.replaced, 5, "{r:?}");
    let listed = manifest(&e.dir)["replaced"].to_string();
    for p in [&stale, &bad] {
        assert!(!listed.contains(p.file_name().unwrap().to_str().unwrap()));
    }
    let r = after_grace(&e, &mut a);
    assert_eq!(r.deleted, 5, "{r:?}");
    assert!(
        stale.exists() && bad.exists(),
        "set-aside segments are kept"
    );
    assert_eq!(
        a.quarantined_for_test(),
        aside,
        "still set aside, still current"
    );
    let g = e.open("g");
    assert_eq!(g.head(MAIN), 5);
    assert_eq!(g.quarantined_for_test().len(), 2);
}

#[test]
fn pack_epoch_is_the_highest_it_replaces() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 2);
    a.set_test_lease(3, true);
    segs(&e, &mut a, "u", 2);
    let epochs: Vec<u64> = segments(&e.dir).iter().map(|s| s.1.epoch()).collect();
    assert_eq!(epochs, [1, 1, 3, 3]);
    a.compact(true).unwrap();
    let all = segments(&e.dir);
    let pack = all.iter().find(|s| s.2.header.pack).unwrap();
    assert_eq!((pack.2.header.epoch, pack.1.epoch()), (3, 3));
    assert_verifies(&a.verify());
    after_grace(&e, &mut a);
    assert_eq!(packs(&e.dir).len(), 1);
    assert_verifies(&a.verify());
    assert_verifies(&e.open("g").verify());
}

#[test]
fn follower_inside_the_packed_range_resumes_through_the_pack() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    // Segments of two records each.
    for i in 0..3 {
        commit(
            &mut a,
            &format!("t{i}a"),
            vec![asset(&format!("/m/{i}a.mov"))],
        );
        commit(
            &mut a,
            &format!("t{i}b"),
            vec![asset(&format!("/m/{i}b.mov"))],
        );
        e.flush(&mut a);
    }
    let mut f = e.open("f");
    assert_eq!(f.head(MAIN), 6);
    for i in 3..5 {
        commit(
            &mut a,
            &format!("t{i}a"),
            vec![asset(&format!("/m/{i}a.mov"))],
        );
        commit(
            &mut a,
            &format!("t{i}b"),
            vec![asset(&format!("/m/{i}b.mov"))],
        );
        e.flush(&mut a);
    }
    a.compact(true).unwrap();
    after_grace(&e, &mut a);
    assert_eq!(segments(&e.dir).len(), 1, "only the pack is left");

    e.poll(&mut f);
    assert_eq!(f.head(MAIN), 10);
    assert_eq!(f.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());
    assert_eq!(f.read_only(), Some(ReadOnlyReason::LeaseHeld));
    assert_verifies(&f.verify());
}

/// A (epoch 1) exports `c0..c2`; F and B open. A pauses and B takes over (epoch 2). A, not
/// knowing, writes `l0..l1` (epoch 1). With `inside`, A compacts after that, so its pack holds
/// the late records and the fork falls inside the pack; otherwise A compacted `c0..c2` before
/// and the fork falls right after the pack. F applies A's late line; then B, which does not
/// see A's late files yet, writes `w0` on `c2`. Every server ends on B's line.
fn rewind_after_compaction(inside: bool) {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "c", 3);
    if !inside {
        a.compact(true).unwrap();
        after_grace(&e, &mut a);
    }
    let mut f = e.open("f");
    let mut b = e.open("b");
    short_grace(&mut b);
    freeze(&a);
    poll_until_held(&e, &mut b);
    assert_eq!(held_epoch(&b), Some(2));

    // A's late writes land after the takeover.
    a.set_test_lease(1, true);
    let before: Vec<PathBuf> = segments(&e.dir).into_iter().map(|s| s.0).collect();
    segs(&e, &mut a, "l", 2);
    if inside {
        a.compact(true).unwrap();
        after_grace(&e, &mut a);
        assert_eq!(segments(&e.dir).len(), 1, "one pack: c0..c2 and l0..l1");
    }
    e.poll(&mut f);
    assert_eq!(f.head(MAIN), 5, "F applied the late line");
    let late: Vec<PathBuf> = segments(&e.dir)
        .into_iter()
        .map(|s| s.0)
        .filter(|p| !before.contains(p))
        .collect();
    for p in &late {
        fin_store::scan::hide_for_test(p);
    }
    commit(&mut b, "w0", vec![insert(0, "a1")]);
    e.flush(&mut b);
    for p in &late {
        fin_store::scan::unhide_for_test(p);
    }

    e.poll(&mut f);
    e.poll(&mut b);
    assert_eq!(f.head(MAIN), 4, "F rewound to B's line");
    assert_eq!(f.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert_eq!(f.read_only(), Some(ReadOnlyReason::LeaseHeld));
    assert_eq!(b.read_only(), None);
    let mut g = e.open("g");
    assert_eq!(g.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    let aside = |s: &Store| {
        s.quarantined_for_test()
            .into_iter()
            .map(|q| q.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(aside(&g), aside(&f));
    assert_eq!(aside(&f).len(), 2);
    assert_verifies(&f.verify());
    assert_verifies(&g.verify());

    // B goes on, and compacts its own line, which resolves through A's pack.
    segs(&e, &mut b, "u", 2);
    let r = b.compact(true).unwrap();
    assert_eq!(r.replaced, 3, "w0, u0, u1 (B's line only): {r:?}");
    after_grace(&e, &mut b);
    e.poll(&mut f);
    e.poll(&mut g);
    for s in [&f, &g] {
        assert_eq!(s.head(MAIN), 6);
        assert_eq!(s.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
        assert_verifies(&s.verify());
    }
    let h = e.open("h");
    assert_eq!(h.snapshot(MAIN).unwrap(), b.snapshot(MAIN).unwrap());
    assert_eq!(aside(&h), aside(&f));
    assert_verifies(&b.verify());
}

#[test]
fn rewind_with_the_fork_inside_a_pack() {
    rewind_after_compaction(true);
}

#[test]
fn rewind_with_the_fork_right_after_a_pack() {
    rewind_after_compaction(false);
}

#[test]
fn another_servers_manifest_is_finished_after_takeover() {
    let e = env();
    let mut a = e.create("a");
    segs(&e, &mut a, "t", 4);
    a.compact(true).unwrap();
    let mut b = e.open("b");
    short_grace(&mut b);
    freeze(&a);
    // Far more than B's grace passes before B holds the lease: it still counts from its own
    // first sighting.
    poll_until_held(&e, &mut b);
    let st = b.compaction_status();
    assert_eq!(st.pending.len(), 1, "{st:?}");
    assert_eq!(st.pending[0].writer_id.as_deref(), Some(a.server_id()));
    assert_eq!(originals(&e.dir).len(), 4);
    e.advance(2);
    assert_eq!(e.poll(&mut b).compact.deleted, 0);
    let r = after_grace(&e, &mut b);
    assert_eq!((r.finished.len(), r.deleted), (1, 4), "{r:?}");
    assert!(manifests(&e.dir).is_empty());
    assert_verifies(&b.verify());
    assert_eq!(e.open("g").head(MAIN), 4);
}

#[test]
fn lease_lost_between_deletes_stops() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 4);
    a.compact(true).unwrap();
    e.advance(GRACE.as_secs() + 1);
    let lease_file = e.dir.join(".finisher").join("lease.json");
    a.on_compaction_delete_for_test(Box::new(move |_| {
        let _ = fs::remove_file(&lease_file);
    }));
    let r = a.compact(false).unwrap();
    assert!(r.skipped.is_some() && r.finished.is_empty(), "{r:?}");
    assert_eq!(
        originals(&e.dir).len(),
        3,
        "one delete, then the fence stopped it"
    );
    assert_eq!(manifests(&e.dir).len(), 1);
    assert_eq!(e.open("g").head(MAIN), 4);
}

#[test]
fn sync_folder_grace_is_24_hours() {
    assert_eq!(
        fin_store::compaction_grace(StorageKind::Sync(Provider::Dropbox)),
        Duration::from_secs(24 * 3600)
    );
    assert_eq!(
        fin_store::compaction_grace(StorageKind::Network),
        Duration::from_secs(600)
    );
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(1, true);
    a.set_test_storage_kind(StorageKind::Sync(Provider::Dropbox));
    segs(&e, &mut a, "t", 3);
    a.compact(true).unwrap();
    assert_eq!(manifest(&e.dir)["kind"], "sync:dropbox");
    e.clock.advance(Duration::from_secs(23 * 3600));
    assert_eq!(a.compact(false).unwrap().deleted, 0);
    // The writer's sync folder counts even where this server sees a local one.
    a.set_test_storage_kind(StorageKind::Local);
    assert_eq!(a.compact(false).unwrap().deleted, 0);
    a.set_test_storage_kind(StorageKind::Sync(Provider::Dropbox));
    e.clock.advance(Duration::from_secs(2 * 3600));
    let r = a.compact(false).unwrap();
    assert_eq!((r.finished.len(), r.deleted), (1, 3), "{r:?}");
}

#[test]
fn more_than_200_segments_trigger_compaction() {
    let e = env();
    let mut a = e.create("a");
    a.set_test_lease(1, true);
    short_grace(&mut a);
    // Fixed lease: the clock may jump (the export delay grows to 30 s under churn).
    let flush = |a: &mut Store| {
        e.poll(a);
        e.clock.advance(Duration::from_secs(31));
        e.poll(a);
    };
    for i in 0..=fin_store::COMPACT_TRIGGER {
        commit(
            &mut a,
            &format!("t{i}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
        flush(&mut a);
        if i < fin_store::COMPACT_TRIGGER {
            assert!(manifests(&e.dir).is_empty(), "segment {i}");
        }
    }
    e.poll(&mut a);
    assert_eq!(manifests(&e.dir).len(), 1);
    assert_eq!(originals(&e.dir).len(), fin_store::COMPACT_TRIGGER + 1);
    e.clock.advance(GRACE + Duration::from_secs(1));
    let r = e.poll(&mut a).compact;
    assert_eq!(r.deleted as usize, fin_store::COMPACT_TRIGGER + 1, "{r:?}");
    assert_eq!(packs(&e.dir).len(), 1);
    assert_eq!(
        packs(&e.dir)[0].records.len(),
        fin_store::COMPACT_TRIGGER + 1
    );
    assert_verifies(&a.verify());
}

#[test]
fn a_manifest_that_contradicts_the_folder_is_abandoned() {
    let e = env();
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 3);
    a.compact(true).unwrap();
    segs(&e, &mut a, "u", 1);
    let newest = originals(&e.dir).into_iter().last().unwrap();
    // A hand-made manifest naming a segment that no pack replaces.
    let mut m = manifest(&e.dir);
    m["replaced"] = serde_json::json!([newest.file_name().unwrap().to_str().unwrap()]);
    let forged = compaction_dir(&e.dir).join(ManifestName::new().to_string());
    fs::write(&forged, serde_json::to_vec(&m).unwrap()).unwrap();
    e.poll(&mut a);
    let r = after_grace(&e, &mut a);
    assert_eq!(r.finished.len(), 1, "the real one: {r:?}");
    assert_eq!(r.abandoned.len(), 1, "{r:?}");
    assert!(
        newest.exists(),
        "nothing named by an abandoned manifest is deleted"
    );
    assert!(forged.exists());
    let st = a.status();
    assert!(
        st.warnings.iter().any(|w| w.code == "COMPACTION_ABANDONED"),
        "{:?}",
        st.warnings
    );
    assert_eq!(st.compaction.pending[0].state, "abandoned");
    // Checked again on demand: still abandoned, still nothing deleted.
    assert!(a.compact(true).unwrap().manifests.is_empty());
    e.advance(GRACE.as_secs() + 1);
    let r = a.compact(true).unwrap();
    assert_eq!(r.abandoned.len(), 1, "{r:?}");
    assert!(newest.exists());
}

#[test]
fn a_conflict_copy_of_a_manifest_makes_the_project_read_only() {
    let e = env();
    let mut a = e.create("a");
    segs(&e, &mut a, "t", 2);
    a.compact(true).unwrap();
    let m = manifests(&e.dir).remove(0);
    let stem = m.file_stem().unwrap().to_str().unwrap();
    let copy =
        compaction_dir(&e.dir).join(format!("{stem} (Bob's conflicted copy 2026-11-04).json"));
    fs::copy(&m, &copy).unwrap();
    let other = compaction_dir(&e.dir).join("notes.txt");
    fs::write(&other, b"x").unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(ReadOnlyReason::ConflictCopy));
    assert_eq!(a.status().foreign.len(), 2);
    fs::remove_file(&copy).unwrap();
    e.poll(&mut a);
    assert_eq!(a.read_only(), None, "other files there are only listed");
    assert!(ManifestName::parse(m.file_name().unwrap().to_str().unwrap()).is_some());
    assert!(
        ManifestName::parse("01JABCDEFGHJKMNPQRSTVWXYZ0.json").is_none(),
        "lowercase only"
    );
}

#[test]
fn a_follower_knows_a_moved_folder_by_its_packs() {
    let mut e = env();
    let parent = e.root.join("Projects");
    e.dir = parent.join("My Project");
    let mut a = e.create("a");
    short_grace(&mut a);
    segs(&e, &mut a, "t", 3);
    let mut f = e.open("f");
    a.compact(true).unwrap();
    after_grace(&e, &mut a);
    e.poll(&mut f);
    assert_eq!(f.head(MAIN), 3);
    drop(f);
    drop(a);
    // The follower's newest segment is gone: the pack it knows stands for it.
    let renamed = e.root.join("Projects 2026");
    fs::rename(&parent, &renamed).unwrap();
    let moved = e.try_open_at("f", &renamed.join("My Project")).unwrap();
    assert_eq!(moved.head(MAIN), 3);
}
