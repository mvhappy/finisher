//! Rebuild from the project folder, salvage, verify and daily backups (plan Task 12, spec
//! §3.8).

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{CoreError, Project, SCHEMA_VERSION, Tx};
use fin_store::names::{SnapName, hash12};
use fin_store::snapfile::{self, SnapHeader};
use fin_store::{Generation, MAIN, Store, TxStatus, VerifyReport, db_path};
use integration_support::*;
use rusqlite::Connection;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2")
}

type Dump = (
    Vec<Vec<String>>,
    Vec<(String, i64, String)>,
    Vec<(String, i64)>,
);

/// Every `txs` row (all columns), the snapshots and the id counters of the database at `db`.
fn dump(db: &Path) -> Dump {
    let c = Connection::open(db).unwrap();
    let mut st = c
        .prepare(
            "SELECT seq, branch, tx_id, kind, hex(record_bytes), tx_hash, parent_hash, epoch,
                    exported, diff, schema_version, author, committed_at FROM txs ORDER BY seq",
        )
        .unwrap();
    let txs = st
        .query_map([], |r| {
            (0..13)
                .map(|i| {
                    let v: rusqlite::types::Value = r.get(i)?;
                    Ok(format!("{v:?}"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut st = c
        .prepare("SELECT branch, seq, json FROM snapshots ORDER BY branch, seq")
        .unwrap();
    let snaps = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut st = c
        .prepare("SELECT kind, next FROM ids ORDER BY kind")
        .unwrap();
    let ids = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    (txs, snaps, ids)
}

fn check<'a>(r: &'a VerifyReport, name: &str) -> &'a fin_store::Check {
    r.checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("no check {name}: {r:?}"))
}

fn commits(s: &mut Store, n: usize) {
    for i in 0..n {
        if i % 2 == 0 {
            commit(s, &format!("t{i}"), vec![asset(&format!("/m/{i}.mov"))]);
        } else {
            commit(s, &format!("t{i}"), vec![insert(0, "a1")]);
        }
    }
}

/// Writes a folder snapshot of `project` claiming to be the state after `seq` (`tx_hash`).
fn write_snapshot(dir: &Path, project_id: &str, seq: u64, tx_hash: &str, project: &Project) {
    let header = SnapHeader {
        format_version: snapfile::FORMAT_VERSION,
        min_reader_version: snapfile::MIN_READER_VERSION,
        project_id: project_id.to_string(),
        branch: MAIN.to_string(),
        seq,
        tx_hash: tx_hash.to_string(),
        model_schema_version: project.schema_version,
        writer_id: "test".to_string(),
        epoch: 1,
        genesis_hash: fin_store::project_file::genesis_hash(
            &fin_store::read_project_file(dir).unwrap().genesis.unwrap(),
        ),
    };
    let (bytes, hash) = snapfile::encode(&header, project);
    let name = SnapName::new(seq, hash12(&hash).unwrap()).unwrap();
    let d = dir.join(".finisher").join("snapshots").join(MAIN);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join(name.to_string()), bytes).unwrap();
}

fn tx_hash_at(db: &Path, seq: u64) -> String {
    let c = Connection::open(db).unwrap();
    c.query_row(
        "SELECT tx_hash FROM txs WHERE seq = ?1",
        [seq as i64],
        |r| r.get(0),
    )
    .unwrap()
}

fn files_starting(dir: &Path, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    v
}

#[test]
fn deleted_db_rebuilds_identically() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 205); // past a DB snapshot (every 200 revs)
    let before = a.snapshot(MAIN).unwrap();
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    let (txs, snaps, ids) = dump(&db);
    fs::remove_dir_all(db.parent().unwrap()).unwrap();

    let a = e.open("a");
    let report = a.last_rebuild().expect("rebuilt at open").clone();
    assert_eq!(report.reason, "missing");
    assert_eq!(report.records, 205);
    assert_eq!(report.salvaged, 0);
    assert_eq!(report.seeded_from_snapshot, None);
    assert!(report.verify.ok, "{:?}", report.verify);
    assert_eq!(a.snapshot(MAIN).unwrap(), before);
    drop(a);
    let (txs2, snaps2, ids2) = dump(&db);
    assert_eq!(
        txs2, txs,
        "every row, byte for byte (epoch, diff, exported)"
    );
    assert_eq!(snaps2, snaps);
    assert_eq!(ids2, ids);
}

#[test]
fn corrupt_db_rebuilds_and_salvages_unexported() {
    // Also: quick_check runs on every open and its failure rebuilds; the store opens.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    e.flush(&mut a);
    for id in ["u1", "u2", "u3"] {
        commit(&mut a, id, vec![asset(&format!("/m/{id}.mov"))]);
    }
    let db = a.db_path().to_path_buf();
    drop(a); // no flush: u1–u3 exist only in this database
    // An old quarantine row, to be salvaged too.
    let copy = e.root.join("copy.fdb");
    {
        let c = Connection::open(&db).unwrap();
        c.execute(
            "INSERT INTO quarantine (tx_hash, branch, origin, reason, detail, record_bytes,
                                     added_at, seen_at, current)
             VALUES ('blake3:00', 'main', 'folder', 'STALE_EPOCH', 'seg', X'7B7D', 1, 1, 0)",
            [],
        )
        .unwrap();
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
        c.execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
    }
    // Corrupt one page of the copy (the `segments` table's root), then put it in place.
    let (root, page): (i64, i64) = {
        let c = Connection::open(&copy).unwrap();
        let root = c
            .query_row(
                "SELECT rootpage FROM sqlite_master WHERE name = 'segments'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        (
            root,
            c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap(),
        )
    };
    let mut bytes = fs::read(&copy).unwrap();
    let at = ((root - 1) * page) as usize;
    bytes[at..at + page as usize].fill(0xAB);
    for ext in ["", "-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{ext}", db.display()));
    }
    fs::write(&db, bytes).unwrap();

    let mut a = e.open("a");
    let report = a.last_rebuild().expect("rebuilt at open").clone();
    assert_eq!(report.reason, "corrupt");
    assert_eq!(report.records, 2);
    assert_eq!(
        report.salvaged, 4,
        "3 unexported records and 1 quarantine row"
    );
    let kept = report.corrupt_copy.clone().expect("old database kept");
    assert!(kept.is_file());
    assert!(
        kept.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("project.fdb.corrupt-")
    );
    assert_eq!(a.head(MAIN), 2);
    let set_aside = a.quarantined_for_test();
    let salvaged: Vec<_> = set_aside
        .iter()
        .filter(|(_, reason, origin)| reason == "SALVAGED" && origin == "salvage")
        .collect();
    assert_eq!(salvaged.len(), 3, "{set_aside:?}");
    assert_eq!(
        a.tx_status("u2").unwrap(),
        TxStatus::SetAside {
            branch: MAIN.into(),
            reason: "SALVAGED".into(),
            origin: "salvage".into()
        }
    );
    {
        let c = Connection::open(a.db_path()).unwrap();
        let (origin, reason, bytes): (String, String, Vec<u8>) = c
            .query_row(
                "SELECT origin, reason, record_bytes FROM quarantine WHERE tx_hash = 'blake3:00'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (origin.as_str(), reason.as_str(), bytes.as_slice()),
            ("salvage", "STALE_EPOCH", b"{}".as_slice())
        );
    }
    assert!(
        a.status().warnings.iter().any(|w| w.code == "DB_REBUILT"),
        "{:?}",
        a.status().warnings
    );
    // Writable, and the salvaged ids are not handed out again.
    assert_eq!(a.read_only(), None);
    let c = commit(&mut a, "t3", vec![asset("/m/t3.mov")]);
    assert_eq!(c.rev, 3);
    assert_eq!(
        c.bindings["$a"], "a5",
        "a2–a4 belong to the salvaged records"
    );
    let ok: String = Connection::open(a.db_path())
        .unwrap()
        .query_row("PRAGMA quick_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
    // Reopening does not rebuild again.
    drop(a);
    assert!(e.open("a").last_rebuild().is_none());
}

#[test]
fn rebuild_ignores_offchain_snapshot() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 4);
    let before = a.snapshot(MAIN).unwrap();
    let (_, mut bogus) = a.snapshot(MAIN).unwrap();
    bogus.name = "Bogus".into();
    let id = a.project_id().to_string();
    let db = a.db_path().to_path_buf();
    let real2 = tx_hash_at(&db, 2);
    a.close().unwrap();
    // Not on the chain: a hash no record has, and the right seq with another record's hash.
    let nowhere = format!("blake3:{}", "ab".repeat(32));
    write_snapshot(&e.dir, &id, 3, &nowhere, &bogus);
    write_snapshot(&e.dir, &id, 2, &tx_hash_at(&db, 1), &bogus);
    assert_ne!(real2, tx_hash_at(&db, 1));

    let b = e.open("b");
    let r = b.last_rebuild().unwrap();
    assert_eq!(r.seeded_from_snapshot, None);
    assert_eq!(b.snapshot(MAIN).unwrap(), before);
    assert!(b.verify().ok, "{:?}", b.verify());
}

#[test]
fn seeded_rebuild_equals_full_replay() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 4);
    let (rev, at4) = a.snapshot(MAIN).unwrap();
    assert_eq!(rev, 4);
    commits_from(&mut a, 4, 3);
    let id = a.project_id().to_string();
    let hash4 = tx_hash_at(a.db_path(), 4);
    a.close().unwrap();

    // Full replay (no snapshot in the folder).
    let full = e.open("full");
    assert_eq!(full.last_rebuild().unwrap().seeded_from_snapshot, None);
    let _ = full.ops_since(MAIN, 0).unwrap();
    let full_db = full.db_path().to_path_buf();
    drop(full);

    // Seeded from the snapshot at seq 4.
    write_snapshot(&e.dir, &id, 4, &hash4, &at4);
    let seeded = e.open("seeded");
    assert_eq!(seeded.last_rebuild().unwrap().seeded_from_snapshot, Some(4));
    assert_eq!(seeded.head(MAIN), 7);
    let v = seeded.verify();
    assert!(
        v.ok,
        "a seeded database verifies before its diffs are filled: {v:?}"
    );
    assert_eq!(check(&v, "diffs_match").details["notStored"], 4);
    // Reading history fills the diffs the seed skipped; then both databases are the same.
    let all = seeded.ops_since(MAIN, 0).unwrap();
    assert_eq!(all.len(), 7);
    let seeded_db = seeded.db_path().to_path_buf();
    drop(seeded);
    assert_eq!(dump(&seeded_db), dump(&full_db));

    // A snapshot on the chain whose state is wrong seeds (it is trusted), and verify finds it.
    let mut wrong = at4.clone();
    wrong.name = "Wrong".into();
    let snaps = e.dir.join(".finisher").join("snapshots").join(MAIN);
    fs::remove_dir_all(&snaps).unwrap();
    write_snapshot(&e.dir, &id, 4, &hash4, &wrong);
    let lied = e.open("lied");
    assert_eq!(lied.last_rebuild().unwrap().seeded_from_snapshot, Some(4));
    let v = lied.verify();
    assert!(!v.ok);
    assert!(!check(&v, "replay_matches_live").ok, "{v:?}");
}

/// A rebuild never reads snapshots through a linked branch directory (spec §3.6.1).
#[cfg(unix)]
#[test]
fn rebuild_never_seeds_through_a_linked_snapshot_dir() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 4);
    let (_, at4) = a.snapshot(MAIN).unwrap();
    commits_from(&mut a, 4, 3);
    let id = a.project_id().to_string();
    let hash4 = tx_hash_at(a.db_path(), 4);
    a.close().unwrap();
    write_snapshot(&e.dir, &id, 4, &hash4, &at4);
    let snaps = e.dir.join(".finisher").join("snapshots").join(MAIN);
    let outside = tempfile::tempdir().unwrap();
    let moved = outside.path().join("main");
    fs::rename(&snaps, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &snaps).unwrap();
    let s = e.open("seeded");
    assert_eq!(s.last_rebuild().unwrap().seeded_from_snapshot, None);
    assert_eq!(s.head(MAIN), 7);
}

fn commits_from(s: &mut Store, from: usize, n: usize) {
    for i in from..from + n {
        commit(s, &format!("t{i}"), vec![asset(&format!("/m/{i}.mov"))]);
    }
}

#[test]
fn verify_reports_each_check() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 3);
    e.flush(&mut a);
    let v = a.verify();
    let names: Vec<&str> = v.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "integrity_check",
            "segment_checksums",
            "chain_continuity",
            "epoch_order",
            "replay_matches_live",
            "diffs_match",
            "genesis",
            "record_size",
            "branch_index",
        ]
    );
    assert!(v.ok, "{v:?}");
    assert!(v.checks.iter().all(|c| c.ok));

    // A stored diff that replay does not produce.
    {
        let c = Connection::open(a.db_path()).unwrap();
        c.execute(
            "UPDATE txs SET diff = '{\"changes\":[],\"shifts\":[]}' WHERE seq = 2",
            [],
        )
        .unwrap();
    }
    let v = a.verify();
    assert!(!v.ok);
    let d = check(&v, "diffs_match");
    assert!(!d.ok);
    assert_eq!(d.details["mismatches"][0]["seq"], 2, "{d:?}");
    for c in &v.checks {
        if c.name != "diffs_match" {
            assert!(c.ok, "{c:?}");
        }
    }

    // A damaged segment on local storage (written atomically there): corrupt at once (the
    // sync-folder rule is in rebuild_safety.rs).
    let bad = seg_dir(&e.dir).join(format!(
        "{:08}-{:012}-{:012}-{}.fseg",
        1, 4, 4, "0123456789ab"
    ));
    fs::write(&bad, b"{\"magic\":\"finisher-segment\"}\nnot a segment\n").unwrap();
    let v = a.verify();
    let s = check(&v, "segment_checksums");
    assert!(!s.ok);
    assert_eq!(s.details["corrupt"][0]["code"], "SEGMENT_CORRUPT", "{s:?}");
    fs::remove_file(&bad).unwrap();

    // A segment that is momentarily unreadable (a Windows sharing violation, or deleted by
    // compaction while another client holds it open: spec §3.13 item 15) is not corrupt: it is
    // listed as unsettled, and the check passes.
    let held = seg_dir(&e.dir)
        .read_dir()
        .unwrap()
        .map(|d| d.unwrap().path())
        .min()
        .unwrap();
    fin_store::scan::lock_for_test(&held, true);
    let v = a.verify();
    fin_store::scan::lock_for_test(&held, false);
    let s = check(&v, "segment_checksums");
    assert!(s.ok, "{s:?}");
    assert_eq!(s.details["corrupt"], serde_json::json!([]), "{s:?}");
    assert_eq!(s.details["unsettled"][0]["reason"], "locked", "{s:?}");
    for c in &v.checks {
        assert!(c.ok || c.name == "diffs_match", "{c:?}");
    }

    // Never fatal: the store still opens.
    drop(a);
    let a = e.open("a");
    assert_eq!(a.head(MAIN), 3);
}

#[test]
fn verify_reports_a_genesis_that_does_not_match_rev_0() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 2);
    assert!(check(&a.verify(), "genesis").ok);
    edit_project_file(&e.dir, |f| {
        f.get_mut("genesis").unwrap()["rate"] = "25".into();
    });
    let v = a.verify();
    let g = check(&v, "genesis");
    assert!(!g.ok, "{g:?}");
    assert!(!v.ok);
}

#[test]
fn verify_reports_records_too_large_for_a_segment() {
    // Only a migrated sub-project 2 record can be this large; planted here.
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 2);
    {
        let c = Connection::open(a.db_path()).unwrap();
        let bytes: Vec<u8> = c
            .query_row("SELECT record_bytes FROM txs WHERE seq = 2", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut big = bytes.clone();
        big.extend(std::iter::repeat_n(
            b' ',
            fin_store::segment::MAX_RECORD_BYTES,
        ));
        c.execute("UPDATE txs SET record_bytes = ?1 WHERE seq = 2", [big])
            .unwrap();
    }
    let v = a.verify();
    let r = check(&v, "record_size");
    assert!(!r.ok);
    assert_eq!(r.details["tooLarge"][0]["seq"], 2, "{r:?}");
}

#[test]
fn a_rebuilt_database_refuses_callers_of_the_old_one() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let old: Generation = a.generation_token(MAIN);
    assert_eq!(old.n, 0);
    e.flush(&mut a);
    let report = a.rebuild().unwrap();
    assert_eq!(report.reason, "manual");
    assert_eq!(report.records, 1);
    let new = a.generation_token(MAIN);
    assert_eq!(new.n, 0, "generations restart in a new database");
    assert_ne!(new.incarnation, old.incarnation);
    let tx = |id: &str| Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: 1,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops: vec![asset(&format!("/m/{id}.mov"))],
    };
    let err: CoreError = a
        .commit_at_generation(tx("x1"), Some(&old))
        .unwrap_err()
        .into();
    assert_eq!(err.code, "HISTORY_REWOUND", "{err:?}");
    assert_eq!(err.details["reason"], "REBUILT");
    assert_eq!(err.details["rev"], 0);
    let ok = a.commit_at_generation(tx("x2"), Some(&new)).unwrap();
    assert_eq!(ok.rev, 2);
    let (g, _) = a.ops_since_at(MAIN, 0).unwrap();
    assert_eq!(g, new);
}

#[test]
fn manual_rebuild_flushes_first_and_keeps_the_old_database() {
    let e = env();
    let mut a = e.create("a");
    commits(&mut a, 3);
    let before = a.snapshot(MAIN).unwrap();
    let report = a.rebuild().unwrap();
    assert_eq!(report.salvaged, 0, "pending records were exported first");
    assert_eq!(report.records, 3);
    assert_eq!(a.snapshot(MAIN).unwrap(), before);
    let dir = a.db_path().parent().unwrap().to_path_buf();
    let kept: Vec<String> = files_starting(&dir, "project.fdb.replaced-")
        .into_iter()
        .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(files_starting(&dir, "project.fdb.rebuild-").is_empty());
    commit(&mut a, "t9", vec![asset("/m/t9.mov")]);
    assert_eq!(a.head(MAIN), 4);
    assert!(a.verify().ok);
}

#[test]
fn sp2_fixture_rebuilds_and_verifies() {
    let e = env();
    copy_dir(&fixture().join("folder"), &e.dir);
    copy_dir(&fixture().join("server-data"), &e.data("a"));
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture().join("expected_v2.json")).unwrap()).unwrap();
    let mut a = e.open("a");
    let v = a.verify();
    assert!(v.ok, "{v:?}");
    e.flush(&mut a);
    let report = a.rebuild().unwrap();
    assert!(report.verify.ok, "{:?}", report.verify);
    let (rev, project) = a.snapshot(MAIN).unwrap();
    assert_eq!(
        serde_json::json!({ "rev": rev, "project": project }),
        current_state(&expected)
    );
    let v = a.verify();
    assert!(v.ok, "{v:?}");
}

#[test]
fn daily_backup_once_per_day_keeps_seven() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let dir = a.db_path().parent().unwrap().join("backups");
    // Every poll checks; today's is taken once.
    e.poll(&mut a);
    e.poll(&mut a);
    assert_eq!(
        files_starting(&dir, "daily-").len(),
        1,
        "tick takes today's"
    );
    for f in files_starting(&dir, "daily-") {
        fs::remove_file(dir.join(f)).unwrap();
    }
    // Midday UTC on consecutive days: one local calendar day each, in any time zone
    // within ±11 h.
    let day0: i64 = 1_793_793_600_000; // 2026-11-04T12:00:00Z
    let mut made = vec![];
    for d in 0..10 {
        let now = day0 + d * 86_400_000;
        let first = a.daily_backup(now).unwrap();
        let path = first.expect("first backup of the day");
        assert!(path.is_file());
        made.push(path.file_name().unwrap().to_string_lossy().into_owned());
        assert_eq!(a.daily_backup(now + 3_600_000).unwrap(), None, "once a day");
    }
    fs::write(dir.join("daily-notes.txt"), b"keep me").unwrap();
    fs::write(dir.join("daily-2020-01-01.fdb.tmp"), b"not a backup name").unwrap();
    a.daily_backup(day0 + 10 * 86_400_000).unwrap().unwrap();
    let left = files_starting(&dir, "daily-");
    let backups: Vec<&String> = left.iter().filter(|n| n.ends_with(".fdb")).collect();
    assert_eq!(backups.len(), 7, "{left:?}");
    assert!(left.contains(&"daily-notes.txt".to_string()));
    assert!(left.contains(&"daily-2020-01-01.fdb.tmp".to_string()));
    assert!(!backups.contains(&&made[0]), "oldest pruned");
    // Each backup is a complete project database.
    let c = Connection::open(dir.join(backups[6])).unwrap();
    let n: i64 = c
        .query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 1);
    assert!(db_path(&e.data("a"), a.project_id()).is_file());
}

/// A v2 golden project as this Finisher reads it: upcast to the current model (the fixtures
/// stay as written; spec §5.12.2).
fn current(v: &serde_json::Value) -> serde_json::Value {
    serde_json::to_value(fin_core::upcast_snapshot(v.clone()).unwrap()).unwrap()
}

/// A v2 golden `{rev, project}` with its project upcast to the current model.
fn current_state(v: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "rev": v["rev"], "project": current(&v["project"]) })
}
