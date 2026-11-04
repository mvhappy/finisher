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
        .filter(|n| n.contains("rebuild"))
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
