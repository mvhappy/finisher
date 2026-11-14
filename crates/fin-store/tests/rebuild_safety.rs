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

/// Plan 4B Task 11: exported records of a branch whose segments are gone from the folder are
/// what a manual rebuild refuses to lose, and what a corrupt rebuild salvages as
/// `MISSING_FROM_FOLDER`; the branch itself comes back from its `branch` record on `main`.
#[test]
fn salvage_marks_missing_branch_rows() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let id = a
        .create_branch(fin_store::BranchCreate {
            tx_id: "b1".into(),
            author: "test".into(),
            name: Some("feature".into()),
            from: MAIN.into(),
            at: None,
            dry_run: false,
            args_hash: None,
        })
        .unwrap()
        .branch
        .id;
    e.flush(&mut a);
    for c in ["c1", "c2"] {
        let base = a.head(&id);
        a.commit(fin_core::Tx {
            tx_id: c.into(),
            author: "test".into(),
            branch: id.clone(),
            base_rev: base,
            schema_version: fin_core::SCHEMA_VERSION,
            args_hash: None,
            via: None,
            ops: vec![insert(0, "a1")],
        })
        .unwrap();
        e.flush(&mut a);
    }
    let branch_dir = e.dir.join(".finisher").join("segments").join(&id);
    fs::remove_dir_all(&branch_dir).unwrap();
    let err: CoreError = a.rebuild().unwrap_err().into();
    assert_eq!(err.code, "REBUILD_WOULD_LOSE", "{err:?}");
    assert_eq!(err.details["missing"], 2);
    assert_eq!(a.head(&id), 3, "nothing changed");

    let db = a.db_path().to_path_buf();
    drop(a);
    damage_segments_table(&db);
    let a = e.open("a");
    let r = a.last_rebuild().unwrap();
    assert_eq!(r.reason, "corrupt");
    assert_eq!(r.missing_from_folder, 2);
    for c in ["c1", "c2"] {
        assert_eq!(
            a.tx_status(c).unwrap(),
            TxStatus::SetAside {
                branch: id.clone(),
                reason: "MISSING_FROM_FOLDER".into(),
                origin: "salvage".into()
            }
        );
    }
    assert_eq!(a.head(&id), 1, "the branch is back, at its fork");
    assert_eq!(a.branches(true).unwrap()[1].name, "feature");
    assert!(check(&r.verify, "branch_index").ok, "{:#?}", r.verify);
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

fn plant_damaged(dir: &Path, seq: u64) -> PathBuf {
    let bad = seg_dir(dir).join(format!(
        "{:08}-{:012}-{:012}-{}.fseg",
        1, seq, seq, "0123456789ab"
    ));
    fs::write(&bad, b"{\"magic\":\"finisher-segment\"}\nnot a segment\n").unwrap();
    bad
}

fn age(path: &Path, secs: u64) {
    let f = fs::File::options().write(true).open(path).unwrap();
    f.set_modified(std::time::SystemTime::now() - Duration::from_secs(secs))
        .unwrap();
}

#[test]
fn on_local_storage_a_damaged_segment_is_corrupt_at_once() {
    // Writes there are atomic (temp + rename): a damaged canonical file is not arriving.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    plant_damaged(&e.dir, 2);
    drop(a);
    let fresh = e.open("a"); // a one-shot verify, no observation history
    let s = check(&fresh.verify(), "segment_checksums").clone();
    assert!(!s.ok, "{s:?}");
    assert_eq!(s.details["corrupt"][0]["code"], "SEGMENT_CORRUPT");
}

#[test]
fn on_a_sync_folder_damage_is_corrupt_once_settled() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    a.set_test_storage_kind(fin_store::storage_kind::StorageKind::Sync(
        fin_store::conflict_names::Provider::Dropbox,
    ));
    let bad = plant_damaged(&e.dir, 2);
    // Just written: may still be syncing.
    assert!(check(&a.verify(), "segment_checksums").ok);
    e.clock.advance(Duration::from_secs(30));
    assert!(check(&a.verify(), "segment_checksums").ok);
    e.clock.advance(Duration::from_secs(31));
    let s = check(&a.verify(), "segment_checksums").clone();
    assert!(!s.ok, "seen unchanged for 61 s: {s:?}");
    // A change restarts the clock.
    fs::write(&bad, b"{\"magic\":\"finisher-segment\"}\nstill not\n").unwrap();
    assert!(check(&a.verify(), "segment_checksums").ok);
    // No history (a fresh observation) of a file whose mtime is older than 60 s: corrupt.
    let old = plant_damaged(&e.dir, 3);
    age(&old, 3600);
    let s = check(&a.verify(), "segment_checksums").clone();
    assert_eq!(s.details["corrupt"].as_array().unwrap().len(), 1, "{s:?}");
}

#[test]
fn import_refuses_segments_of_another_genesis() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let mut b = e.open("b");
    assert_eq!(b.head(MAIN), 1);
    // A segment of the same project id written under another genesis.
    let (_, name, seg) = segments(&e.dir).pop().unwrap();
    let mut h = seg.header.clone();
    h.genesis_hash = format!("blake3:{}", "ee".repeat(32));
    h.first_seq = 2;
    h.last_seq = 2;
    h.prev_segment_hash = Some(seg.hash.clone());
    let rec = b"{\"txId\":\"x\",\"seq\":2}".to_vec();
    let (bytes, hash) = fin_store::segment::encode(&h, &[rec.as_slice()]);
    let n = fin_store::names::SegmentName::new(1, 2, 2, fin_store::names::hash12(&hash).unwrap())
        .unwrap();
    let _ = name;
    fs::write(seg_dir(&e.dir).join(n.to_string()), bytes).unwrap();
    let r = e.poll(&mut b);
    let err = r.imports[0].error.clone().expect("an import error");
    assert_eq!(err.code, "GENESIS_MISMATCH", "{err:?}");
    assert_eq!(err.details["files"].as_array().unwrap().len(), 1);
    assert_eq!(b.read_only(), Some(fin_store::ReadOnlyReason::ImportError));
    assert_eq!(b.head(MAIN), 1);
}

#[test]
fn damaged_application_id_of_our_own_database_is_rebuilt() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    checkpoint(&db);
    damage(&db, 68, 72);
    let s = e.open("a");
    assert_eq!(s.last_rebuild().unwrap().reason, "corrupt");
    assert_eq!(s.head(MAIN), 1);
}

#[test]
fn an_incompletely_salvaged_copy_is_kept_and_reported_until_acknowledged() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    commit(&mut a, "u1", vec![asset("/m/u1.mov")]);
    let db = a.db_path().to_path_buf();
    drop(a);
    checkpoint(&db);
    let (root, ps) = page_of(&db, "txs");
    damage(&db, (root - 1) * ps, root * ps);
    let mut a = e.open("a");
    let kept = a.last_rebuild().unwrap().corrupt_copy.clone().unwrap();
    let kept_name = kept.file_name().unwrap().to_string_lossy().into_owned();
    let dir = db.parent().unwrap().to_path_buf();
    // Healthy manual rebuilds keep their own copies, apart from the damaged ones.
    for _ in 0..4 {
        a.rebuild().unwrap();
    }
    assert!(
        kept.is_file(),
        "never pruned while its salvage is incomplete"
    );
    let replaced: Vec<String> = names(&dir, "project.fdb.replaced-")
        .into_iter()
        .filter(|n| !n.ends_with("-wal") && !n.ends_with("-shm"))
        .collect();
    assert_eq!(replaced.len(), 3, "{replaced:?}");
    // The warning survives a reopen.
    drop(a);
    let mut a = e.open("a");
    let warned = |s: &Store| {
        s.status()
            .warnings
            .iter()
            .any(|w| w.code == "SALVAGE_INCOMPLETE" && w.message.contains(&kept_name))
    };
    assert!(warned(&a));
    assert!(a.acknowledge_salvage(&kept_name).unwrap());
    assert!(!warned(&a));
    assert!(!a.acknowledge_salvage("project.fdb.corrupt-nope").unwrap());
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

/// Review #36: a row whose columns do not decode is skipped alone; the healthy row right
/// after it is salvaged too (the page search used to resume past it).
#[test]
fn salvage_keeps_the_healthy_row_after_an_undecodable_one() {
    let e = env();
    let mut a = e.create("a");
    for (i, id) in ["t1", "t2", "t3", "t4"].into_iter().enumerate() {
        commit(&mut a, id, vec![asset(&format!("/m/{i}.mov"))]);
    }
    let db = a.db_path().to_path_buf();
    drop(a);
    let segs = seg_dir(&e.dir);
    if segs.exists() {
        fs::remove_dir_all(&segs).unwrap();
    }
    let c = Connection::open(&db).unwrap();
    c.execute("UPDATE txs SET exported = 0", []).unwrap();
    // Two bad rows, each followed by a healthy one.
    c.execute(
        "UPDATE txs SET record_bytes = 123 WHERE tx_id IN ('t1', 't3')",
        [],
    )
    .unwrap();
    drop(c);
    let r = fin_store::rebuild_project(&e.data("a"), &e.dir).unwrap();
    assert!(!r.salvage_complete);
    assert_eq!(r.salvage_errors.len(), 2, "{:?}", r.salvage_errors);
    assert_eq!(r.salvaged, 2, "{r:?}");
    let a = e.open("a");
    for id in ["t2", "t4"] {
        assert!(
            matches!(a.tx_status(id).unwrap(), TxStatus::SetAside { ref origin, .. } if origin == "salvage"),
            "{id}: {:?}",
            a.tx_status(id)
        );
    }
}
