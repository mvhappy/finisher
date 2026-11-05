//! Review of fc84abf (plan Task 12): a rebuild never drops what the old database had, says
//! when salvage was incomplete, survives a crash mid-swap, rebuilds a damaged header, and
//! verify does not cry wolf on sync folders.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fin_core::CoreError;
use fin_store::{MAIN, Store, TxStatus, VerifyReport};
use integration_support::*;
use rusqlite::Connection;

fn check<'a>(r: &'a VerifyReport, name: &str) -> &'a fin_store::Check {
    r.checks.iter().find(|c| c.name == name).unwrap()
}

fn checkpoint(db: &Path) {
    let c = Connection::open(db).unwrap();
    c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    drop(c);
    for ext in ["-wal", "-shm"] {
        let _ = fs::remove_file(format!("{}{ext}", db.display()));
    }
}

fn page_of(db: &Path, table: &str) -> (usize, usize) {
    let c = Connection::open(db).unwrap();
    let root: i64 = c
        .query_row(
            "SELECT rootpage FROM sqlite_master WHERE name = ?1",
            [table],
            |r| r.get(0),
        )
        .unwrap();
    let ps: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0)).unwrap();
    (root as usize, ps as usize)
}

fn damage(db: &Path, from: usize, to: usize) {
    let mut b = fs::read(db).unwrap();
    b[from..to].fill(0xAB);
    fs::write(db, b).unwrap();
}

/// Damages the `segments` table's root page: `quick_check` fails, txs stay readable.
fn damage_segments_table(db: &Path) {
    checkpoint(db);
    let (root, ps) = page_of(db, "segments");
    damage(db, (root - 1) * ps, root * ps);
}

fn names(dir: &Path, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    v.sort();
    v
}

fn damage_last_segment(dir: &Path) {
    let segs = segments(dir);
    let last = &segs.last().unwrap().0;
    let mut b = fs::read(last).unwrap();
    let n = b.len();
    b[n / 2] ^= 0xFF;
    fs::write(last, b).unwrap();
}

#[test]
fn manual_rebuild_refuses_to_lose_exported_records() {
    let e = env();
    let mut a = e.create("a");
    for (i, id) in ["t1", "t2", "t3"].into_iter().enumerate() {
        commit(&mut a, id, vec![asset(&format!("/m/{i}.mov"))]);
        e.flush(&mut a);
    }
    damage_last_segment(&e.dir);
    let err: CoreError = a.rebuild().unwrap_err().into();
    assert_eq!(err.code, "REBUILD_WOULD_LOSE", "{err:?}");
    assert_eq!(err.details["missing"], 1);
    assert_eq!(err.details["exportedThrough"], 3);
    assert_eq!(err.details["rebuiltThrough"], 2);
    assert!(err.fix.is_some());
    // Nothing changed; the store still works.
    assert_eq!(a.head(MAIN), 3);
    assert!(matches!(a.tx_status("t3").unwrap(), TxStatus::Committed(_)));
    let dir = a.db_path().parent().unwrap().to_path_buf();
    assert!(names(&dir, "project.fdb.corrupt-").is_empty());
    assert!(names(&dir, "project.fdb.rebuild-").is_empty());
    commit(&mut a, "t4", vec![asset("/m/t4.mov")]);
}

#[test]
fn corrupt_rebuild_salvages_exported_records_the_folder_lacks() {
    let e = env();
    let mut a = e.create("a");
    for (i, id) in ["t1", "t2", "t3"].into_iter().enumerate() {
        commit(&mut a, id, vec![asset(&format!("/m/{i}.mov"))]);
        e.flush(&mut a);
    }
    let db = a.db_path().to_path_buf();
    drop(a);
    damage_last_segment(&e.dir);
    damage_segments_table(&db);
    let a = e.open("a");
    let r = a.last_rebuild().unwrap();
    assert_eq!(r.reason, "corrupt");
    assert_eq!(r.records, 2);
    assert_eq!(r.salvaged, 1);
    assert!(r.salvage_complete);
    assert_eq!(
        a.tx_status("t3").unwrap(),
        TxStatus::SetAside {
            branch: MAIN.into(),
            reason: "MISSING_FROM_FOLDER".into(),
            origin: "salvage".into()
        }
    );
}

#[test]
fn salvage_failure_is_reported_loudly() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    for id in ["u1", "u2", "u3"] {
        commit(&mut a, id, vec![asset(&format!("/m/{id}.mov"))]);
    }
    let db = a.db_path().to_path_buf();
    drop(a);
    checkpoint(&db);
    let (root, ps) = page_of(&db, "txs");
    damage(&db, (root - 1) * ps, root * ps);
    let a = e.open("a");
    let r = a.last_rebuild().unwrap().clone();
    assert!(!r.salvage_complete);
    assert!(!r.salvage_errors.is_empty());
    let kept = r.corrupt_copy.unwrap();
    let w = a
        .status()
        .warnings
        .into_iter()
        .find(|w| w.code == "SALVAGE_INCOMPLETE")
        .expect("loud warning");
    assert!(
        w.message
            .contains(&*kept.file_name().unwrap().to_string_lossy()),
        "{}",
        w.message
    );
}

#[test]
fn salvage_resumes_past_a_bad_page() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/t0.mov")]);
    e.flush(&mut a);
    for i in 0..300 {
        commit(
            &mut a,
            &format!("u{i:03}"),
            vec![asset(&format!("/m/{i}.mov"))],
        );
    }
    let db = a.db_path().to_path_buf();
    drop(a);
    checkpoint(&db);
    // A table-leaf page holding u150 (not the root).
    const PAT: &[u8] = b"\"txId\":\"u150\"";
    let (root, ps) = page_of(&db, "txs");
    let bytes = fs::read(&db).unwrap();
    let page = (0..bytes.len() / ps)
        .find(|p| {
            let pg = &bytes[p * ps..(p + 1) * ps];
            p + 1 != root && pg[0] == 0x0D && pg.windows(PAT.len()).any(|w| w == PAT)
        })
        .expect("a leaf page with u150");
    damage(&db, page * ps, (page + 1) * ps);
    let a = e.open("a");
    let r = a.last_rebuild().unwrap();
    assert!(!r.salvage_complete);
    assert!(r.salvaged > 250 && r.salvaged < 300, "{}", r.salvaged);
    assert!(matches!(
        a.tx_status("u299").unwrap(),
        TxStatus::SetAside { .. }
    ));
    assert!(matches!(
        a.tx_status("u000").unwrap(),
        TxStatus::SetAside { .. }
    ));
}

#[test]
fn a_crash_between_the_renames_still_salvages_the_kept_copy() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    for id in ["u1", "u2"] {
        commit(&mut a, id, vec![asset(&format!("/m/{id}.mov"))]);
    }
    let db = a.db_path().to_path_buf();
    drop(a);
    checkpoint(&db);
    let dir = db.parent().unwrap().to_path_buf();
    fs::rename(&db, dir.join("project.fdb.corrupt-20261105T000000000Z")).unwrap();
    // And an abandoned build of an earlier attempt.
    fs::write(dir.join("project.fdb.rebuild-01abc"), b"half a database").unwrap();
    let a = e.open("a");
    let r = a.last_rebuild().unwrap();
    assert_eq!(r.reason, "missing");
    assert_eq!(r.salvaged, 2);
    assert!(matches!(
        a.tx_status("u1").unwrap(),
        TxStatus::SetAside { .. }
    ));
    assert!(names(&dir, "project.fdb.rebuild-").is_empty(), "swept");
    // The database that recorded the salvage is lost too: the kept copy is salvaged again.
    drop(a);
    fs::remove_file(&db).unwrap();
    let a = e.open("a");
    assert_eq!(a.last_rebuild().unwrap().salvaged, 2);
    assert!(matches!(
        a.tx_status("u1").unwrap(),
        TxStatus::SetAside { .. }
    ));
    // A manual rebuild of that database does not salvage the copy a second time: only the
    // old database's own quarantine rows come along.
    let mut a = a;
    let r = a.rebuild().unwrap();
    assert_eq!(r.salvaged, 2, "the two quarantine rows, not four");
}

#[test]
fn an_orphan_wal_is_set_aside_not_deleted() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    fs::remove_file(&db).unwrap();
    fs::write(
        format!("{}-wal", db.display()),
        b"frames of a lost database",
    )
    .unwrap();
    let a = e.open("a");
    assert_eq!(a.head(MAIN), 1);
    let dir = db.parent().unwrap();
    let kept = names(dir, "project.fdb.orphan-");
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert!(kept[0].ends_with("-wal"));
}

#[test]
fn a_damaged_header_page_is_rebuilt_not_refused() {
    for (label, from, to) in [("schema page body", 100usize, 4096usize), ("magic", 0, 16)] {
        let e = env();
        let mut a = e.create("a");
        commit(&mut a, "t1", vec![asset("/m/a.mov")]);
        let db = a.db_path().to_path_buf();
        a.close().unwrap();
        checkpoint(&db);
        damage(&db, from, to);
        let damaged = fs::read(&db).unwrap();
        let s = e
            .try_open_at("a", &e.dir)
            .unwrap_or_else(|err| panic!("{label}: {err:?}"));
        let r = s.last_rebuild().expect(label);
        assert_eq!(r.reason, "corrupt", "{label}");
        assert_eq!(s.head(MAIN), 1, "{label}");
        assert_eq!(
            fs::read(r.corrupt_copy.as_ref().unwrap()).unwrap(),
            damaged,
            "{label}: the old file is kept as it was"
        );
    }
}

#[test]
fn manual_rebuild_error_leaves_a_working_store() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    fin_store::fail_next_rebuild_swap_for_test();
    let err: CoreError = a.rebuild().unwrap_err().into();
    assert_eq!(err.code, "TEST_FAULT");
    assert!(a.db_path().is_file());
    assert_eq!(a.head(MAIN), 1);
    commit(&mut a, "t2", vec![asset("/m/b.mov")]);
    assert_eq!(a.head(MAIN), 2);
}

#[test]
fn rebuild_refuses_while_another_store_has_the_database_open() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let other = e.open("a");
    let err: CoreError = a.rebuild().unwrap_err().into();
    assert_eq!(err.code, "DB_IN_USE", "{err:?}");
    let err: CoreError = fin_store::rebuild_project(&e.data("a"), &e.dir)
        .unwrap_err()
        .into();
    assert_eq!(err.code, "DB_IN_USE");
    drop(other);
    assert_eq!(a.rebuild().unwrap().records, 1);
    drop(a);
    // Without any store open.
    let r = fin_store::rebuild_project(&e.data("a"), &e.dir).unwrap();
    assert_eq!((r.reason.as_str(), r.records), ("manual", 1));
}

#[test]
fn verify_on_a_sync_folder_does_not_fail_on_placeholders() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    commit(&mut a, "t2", vec![asset("/m/b.mov")]);
    e.flush(&mut a);
    let segs = segments(&e.dir);
    fin_store::scan::mark_placeholder(&segs[0].0);
    let v = a.verify();
    assert!(v.ok, "{v:?}");
    let s = check(&v, "segment_checksums");
    assert_eq!(s.details["unchecked"].as_array().unwrap().len(), 1, "{s:?}");
}

#[test]
fn a_damaged_segment_is_corrupt_only_once_seen_unchanged_for_60s() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let bad = seg_dir(&e.dir).join(format!(
        "{:08}-{:012}-{:012}-{}.fseg",
        1, 2, 2, "0123456789ab"
    ));
    fs::write(&bad, b"{\"magic\":\"finisher-segment\"}\nnot a segment\n").unwrap();
    // An old mtime alone is not enough.
    let f = fs::File::options().write(true).open(&bad).unwrap();
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(3600))
        .unwrap();
    drop(f);
    let s = check(&a.verify(), "segment_checksums").clone();
    assert!(s.ok, "first sighting: {s:?}");
    e.clock.advance(Duration::from_secs(30));
    assert!(check(&a.verify(), "segment_checksums").ok);
    e.clock.advance(Duration::from_secs(31));
    let s = check(&a.verify(), "segment_checksums").clone();
    assert!(!s.ok, "{s:?}");
    assert_eq!(s.details["corrupt"][0]["code"], "SEGMENT_CORRUPT");
    // A change restarts the clock.
    fs::write(&bad, b"{\"magic\":\"finisher-segment\"}\nstill not\n").unwrap();
    assert!(check(&a.verify(), "segment_checksums").ok);
}

#[test]
fn a_backup_is_a_new_incarnation() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let path: PathBuf = a.daily_backup(1_793_793_600_000).unwrap().unwrap();
    let inc: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT value FROM meta WHERE key = 'incarnation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!inc.is_empty());
    assert_ne!(inc, a.incarnation());
}

#[test]
fn segments_bind_the_genesis_and_a_rebuild_refuses_another() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    let seg = &segments(&e.dir)[0].2;
    assert!(seg.header.genesis_hash.starts_with("blake3:"));
    edit_project_file(&e.dir, |f| {
        f.get_mut("genesis").unwrap()["rate"] = "25".into();
    });
    // verify (with the old database) reports it against the folder too.
    let s: Store = e.open("a");
    let g = check(&s.verify(), "genesis").clone();
    assert!(!g.ok);
    assert_eq!(g.details["segmentsDisagree"], 1, "{g:?}");
    drop(s);
    fs::remove_dir_all(db.parent().unwrap()).unwrap();
    let err = e.try_open_at("a", &e.dir).unwrap_err();
    assert_eq!(err.code, "GENESIS_MISMATCH", "{err:?}");
}
