//! Store integration, open side (plan Task 11): the project file's genesis, rebuilding a
//! missing database from the folder, refusing copied folders (plan Decision 6), and the
//! project-file read retry.

mod integration_support;

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use fin_store::{MAIN, Store, db_path, read_project_file};
use integration_support::*;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2")
}

#[test]
fn sp2_project_gets_its_genesis_and_a_fresh_server_rebuilds_it_exactly() {
    let e = env();
    copy_dir(&fixture().join("folder"), &e.dir);
    copy_dir(&fixture().join("server-data"), &e.data("a"));
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture().join("expected.json")).unwrap()).unwrap();
    assert!(read_project_file(&e.dir).unwrap().genesis.is_none());

    let a = e.open("a");
    let file = read_project_file(&e.dir).unwrap();
    let g = file.genesis.expect("backfilled from the rev-0 snapshot");
    assert_eq!(
        (g.name.as_str(), g.rate.to_string().as_str()),
        ("SP2 fixture", "30")
    );
    assert_eq!(
        (file.format_version, file.min_reader_version),
        (Some(1), Some(1))
    );
    assert_eq!(file.storage.as_deref(), Some("local"));
    assert_eq!(file.name, "SP2 fixture");
    let (rev, project) = a.snapshot(MAIN).unwrap();
    assert_eq!(
        serde_json::json!({ "rev": rev, "project": project }),
        expected
    );
    // The holder exports its history; a server without a database rebuilds it from there.
    assert!(a.close().unwrap().segments_written >= 1);

    let b = e.open("b");
    let (rev, project) = b.snapshot(MAIN).unwrap();
    assert_eq!(
        serde_json::json!({ "rev": rev, "project": project }),
        expected
    );
    assert_eq!(b.status().quarantine.count, 0);
    let rows = |data: &str| -> Vec<(u64, Vec<u8>)> {
        let c = rusqlite::Connection::open(db_path(&e.data(data), b.project_id())).unwrap();
        let mut st = c
            .prepare("SELECT seq, record_bytes FROM txs ORDER BY seq")
            .unwrap();
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(rows("b"), rows("a"), "the exact records");
}

#[test]
fn deleted_db_rebuilds_from_the_folder() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    let before = a.snapshot(MAIN).unwrap();
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    fs::remove_dir_all(db.parent().unwrap()).unwrap();

    let mut a = e.open("a");
    assert_eq!(a.snapshot(MAIN).unwrap(), before);
    assert_eq!(a.read_only(), None, "this process re-takes its lease");
    let st = a.status();
    assert_eq!(st.branches[0].exported_through, 2);
    // Ids continue after the imported ones.
    let c = commit(&mut a, "t3", vec![insert(1, "a1")]);
    assert_eq!(c.bindings["$c"], "c2");
    let leftovers: Vec<_> = fs::read_dir(db.parent().unwrap())
        .unwrap()
        .map(|d| d.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("project.fdb.rebuild-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn copied_folder_is_refused() {
    // Review Focus 4 / plan Decision 6.
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    a.close().unwrap();
    let copy = e.root.join("Copy of My Project");
    copy_dir(&e.dir, &copy);

    let err = e.try_open_at("a", &copy).unwrap_err();
    assert_eq!(err.code, "PROJECT_COPY");
    assert!(err.fix.is_some());
    assert!(
        err.details["original"]
            .as_str()
            .unwrap()
            .ends_with("My Project")
    );
    // The original still opens, through another spelling of its path too.
    drop(e.open("a"));
    let spelled = e.root.join(".").join("My Project");
    drop(e.try_open_at("a", &spelled).unwrap());
    // A server with its own database may open the copy (it is a separate folder to it).
    drop(e.try_open_at("b", &copy).unwrap());

    // The original is gone: the project moved, and the database follows it.
    fs::remove_dir_all(&e.dir).unwrap();
    let moved = e.try_open_at("a", &copy).unwrap();
    assert_eq!(moved.head(MAIN), 1);
    drop(moved);
    // The old place is now the copy.
    copy_dir(&copy, &e.dir);
    assert_eq!(e.try_open_at("a", &e.dir).unwrap_err().code, "PROJECT_COPY");
}

#[test]
fn project_file_missing_briefly_is_retried() {
    let e = env();
    drop(e.create("a"));
    let file = e.dir.join("finisher.project.json");
    let aside = e.root.join("aside.json");
    fs::rename(&file, &aside).unwrap();
    let back = std::thread::spawn({
        let (aside, file) = (aside.clone(), file.clone());
        move || {
            std::thread::sleep(Duration::from_millis(300));
            fs::rename(aside, file).unwrap();
        }
    });
    let t = Instant::now();
    let s = e.open("a");
    assert!(t.elapsed() >= Duration::from_millis(250));
    back.join().unwrap();
    drop(s);

    // Missing for good: PROJECT_NOT_FOUND once the window is over.
    fin_store::project_file::set_read_retry_for_test(Duration::from_millis(200));
    fs::remove_file(&file).unwrap();
    let t = Instant::now();
    assert_eq!(
        e.try_open_at("a", &e.dir).unwrap_err().code,
        "PROJECT_NOT_FOUND"
    );
    assert!(t.elapsed() >= Duration::from_millis(200));
    // A folder that never was a sub-project 4 project is answered at once.
    let plain = e.root.join("plain");
    fs::create_dir_all(&plain).unwrap();
    let t = Instant::now();
    let err = Store::open(&e.data("a"), &plain).unwrap_err();
    assert_eq!(fin_core::CoreError::from(err).code, "PROJECT_NOT_FOUND");
    assert!(t.elapsed() < Duration::from_millis(150));
}

// ---- Review of 546b7f6: copies whose original cannot be reached; concurrent rebuilds ----

#[test]
fn a_copy_whose_original_share_is_unmounted_is_refused_until_relocated() {
    // The original lives on a share; the share is unmounted (its mount point is gone), and a
    // copy of the folder is opened: not "moved", since the original may come back.
    let mut e = env();
    let share = e.root.join("share");
    e.dir = share.join("My Project");
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    a.close().unwrap();
    let copy = e.root.join("Copy of My Project");
    copy_dir(&e.dir, &copy);
    let hidden = e.root.join("share-unmounted");
    fs::rename(&share, &hidden).unwrap();
    // `root` stands for the mount base (/Volumes, /mnt) the share was mounted under.
    fin_store::copies_mount_root_for_test(&e.root);

    let err = e.try_open_at("a", &copy).unwrap_err();
    assert_eq!(err.code, "PROJECT_ORIGINAL_UNREACHABLE", "{err:?}");
    assert!(err.fix.as_deref().unwrap().contains("relocate"));
    // Explicitly relocated: the database now belongs to the copy.
    let opts = fin_store::OpenOptions {
        relocate: true,
        ..e.opts()
    };
    drop(Store::open_with(&e.data("a"), &copy, opts).unwrap());
    drop(e.try_open_at("a", &copy).unwrap());
    // The share returns: its folder is now the copy.
    fs::rename(&hidden, &share).unwrap();
    assert_eq!(e.try_open_at("a", &e.dir).unwrap_err().code, "PROJECT_COPY");
}

#[test]
fn a_stale_copy_of_a_deleted_original_is_not_taken_for_a_move() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let copy = e.root.join("Old copy");
    copy_dir(&e.dir, &copy);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    a.close().unwrap(); // exports t2: the copy lacks the newest segment
    fs::remove_dir_all(&e.dir).unwrap();
    let err = e.try_open_at("a", &copy).unwrap_err();
    assert_eq!(err.code, "PROJECT_ORIGINAL_UNREACHABLE", "{err:?}");
}

#[test]
fn concurrent_opens_without_a_database_share_one() {
    for round in 0..12 {
        let e = env();
        let mut a = e.create("a");
        commit(&mut a, "t1", vec![asset("/m/a.mov")]);
        e.flush(&mut a);
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let opened: Vec<Store> = (0..2)
            .map(|_| {
                let (data, dir, opts, b) = (e.data("b"), e.dir.clone(), e.opts(), barrier.clone());
                std::thread::spawn(move || {
                    b.wait();
                    Store::open_with(&data, &dir, opts).unwrap()
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|h| h.join().unwrap())
            .collect();
        let [mut b1, b2] = <[Store; 2]>::try_from(opened).unwrap();
        commit(&mut a, "t2", vec![insert(0, "a1")]);
        e.flush(&mut a);
        e.poll(&mut b1);
        assert_eq!(b1.head(MAIN), 2, "round {round}");
        assert_eq!(
            b2.head(MAIN),
            2,
            "round {round}: both stores use one database"
        );
    }
}

// ---- Re-review: directory identity, stale WAL, renamed parents, newer project file ----

#[test]
fn a_hard_linked_copy_is_a_copy() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    a.close().unwrap();
    let copy = e.root.join("Snapshot of My Project");
    link_dir(&e.dir, &copy);
    assert_eq!(e.try_open_at("a", &copy).unwrap_err().code, "PROJECT_COPY");
}

#[test]
fn a_stale_wal_is_not_replayed_onto_a_rebuilt_database() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    let expected = a.snapshot(MAIN).unwrap();
    let db = a.db_path().to_path_buf();
    a.close().unwrap();
    // An old database's WAL, left behind when project.fdb itself was deleted.
    let wal = PathBuf::from(format!("{}-wal", db.display()));
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.query_row("PRAGMA wal_autocheckpoint=0", [], |_| Ok(()))
            .unwrap();
        for i in 0..50 {
            c.execute(
                "INSERT INTO meta (key, value) VALUES (?1, 'x')",
                [format!("junk{i}")],
            )
            .unwrap();
        }
        c.execute("DELETE FROM txs", []).unwrap();
        fs::copy(&wal, e.root.join("old.wal")).unwrap();
    }
    fs::remove_file(&db).unwrap();
    let _ = fs::remove_file(PathBuf::from(format!("{}-shm", db.display())));
    fs::copy(e.root.join("old.wal"), &wal).unwrap();

    let a = e.open("a");
    assert_eq!(a.snapshot(MAIN).unwrap(), expected);
    let c = rusqlite::Connection::open(&db).unwrap();
    let ok: String = c
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap();
    assert_eq!(ok, "ok");
    let junk: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM meta WHERE key LIKE 'junk%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(junk, 0, "the old WAL was not replayed");
}

#[test]
fn renaming_the_projects_parent_folder_is_a_move() {
    let mut e = env();
    let parent = e.root.join("Projects");
    e.dir = parent.join("My Project");
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    a.close().unwrap();
    let renamed = e.root.join("Projects 2025");
    fs::rename(&parent, &renamed).unwrap();
    let moved = e.try_open_at("a", &renamed.join("My Project")).unwrap();
    assert_eq!(moved.head(MAIN), 1);
}

#[test]
fn a_project_file_with_a_newer_schema_makes_read_only_at_the_next_poll() {
    let e = env();
    let mut a = e.create("a");
    edit_project_file(&e.dir, |f| {
        f.insert("schemaVersion".into(), 99.into());
    });
    e.poll(&mut a);
    assert_eq!(a.read_only(), Some(fin_store::ReadOnlyReason::NewerFormat));
}
