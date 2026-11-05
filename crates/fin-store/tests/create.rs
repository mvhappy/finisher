//! `Store::create`: atomic (no orphan database, no half-made folder), write-once project file,
//! idempotent retries by key, name validation.

use std::path::{Path, PathBuf};

use fin_core::{CoreError, Rational};
use fin_store::{MAIN, Store, StoreError, db_path, read_project_file};
use tempfile::TempDir;

fn secs(n: i64) -> Rational {
    Rational::from_int(n)
}

struct Env {
    _tmp: TempDir,
    data: PathBuf,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    Env {
        data: tmp.path().join("server-data"),
        dir: tmp.path().join("Film"),
        _tmp: tmp,
    }
}

fn code(e: StoreError) -> CoreError {
    CoreError::from(e)
}

/// Every file under `dir`, recursively, relative to it.
fn files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p.strip_prefix(dir).unwrap().to_string_lossy().into_owned());
            }
        }
    }
    out.sort();
    out
}

/// Project databases (and their builds, without `-wal`/`-shm`) under the server data.
fn databases(data: &Path) -> Vec<String> {
    files(data)
        .into_iter()
        .filter(|f| f.contains("project.fdb") && !f.ends_with("-wal") && !f.ends_with("-shm"))
        .collect()
}

#[test]
fn create_is_atomic_under_marker_failure() {
    let e = env();
    fin_store::fail_next_create_marker_for_test();
    let err = code(Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap_err());
    assert_eq!(err.code, "PROJECT_DIR_UNWRITABLE", "{err:?}");
    assert!(databases(&e.data).is_empty(), "{:?}", files(&e.data));
    // Nothing of the project in the folder either (no `.finisher/`, which would make an
    // open wait for a project file that never comes; the folder we made is gone).
    assert!(!e.dir.exists(), "{:?}", files(&e.dir));
    // A retry then succeeds and is the only project.
    let s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    assert_eq!(read_project_file(&e.dir).unwrap().id, s.project_id());
    assert_eq!(
        databases(&e.data),
        [format!("projects/{}/project.fdb", s.project_id())]
    );
}

#[test]
fn marker_failure_in_an_existing_folder_keeps_the_folder() {
    let e = env();
    std::fs::create_dir_all(&e.dir).unwrap();
    std::fs::write(e.dir.join("notes.txt"), b"mine").unwrap();
    fin_store::fail_next_create_marker_for_test();
    Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap_err();
    assert_eq!(files(&e.dir), ["notes.txt"]);
    assert!(databases(&e.data).is_empty());
}

#[test]
fn create_retry_same_key_returns_project() {
    let e = env();
    let first = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k1")).unwrap();
    let id = first.project_id().to_string();
    // The answer was lost while the first store is still open (a server that keeps it).
    let again = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k1")).unwrap();
    assert_eq!(again.project_id(), id);
    drop(again);
    drop(first);
    // And after it closed (a restarted server).
    let retried = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k1")).unwrap();
    assert_eq!(retried.project_id(), id);
    assert_eq!(retried.head(MAIN), 0);
    assert!(retried.read_only().is_none(), "{:?}", retried.read_only());
    retried.close().unwrap();
    assert_eq!(databases(&e.data).len(), 1, "{:?}", databases(&e.data));

    // Another key, or none, is another create: the folder holds a project.
    for key in [Some("k2"), None] {
        let err = code(Store::create(&e.data, &e.dir, "Demo", secs(30), key).unwrap_err());
        assert_eq!(err.code, "PROJECT_EXISTS", "{key:?}");
    }
    // The same key with another name or rate is not a retry of that create.
    for (name, rate) in [("Other", secs(30)), ("Demo", secs(25))] {
        let err = code(Store::create(&e.data, &e.dir, name, rate, Some("k1")).unwrap_err());
        assert_eq!(err.code, "IDEM_KEY_REUSED", "{name} {rate}");
        assert_eq!(err.details["projectId"], id.as_str());
    }
    assert_eq!(databases(&e.data).len(), 1);
}

#[test]
fn create_key_is_stored_in_meta() {
    let e = env();
    let s = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("key-7")).unwrap();
    let db = db_path(&e.data, s.project_id());
    drop(s);
    let conn = rusqlite::Connection::open(&db).unwrap();
    let key: String = conn
        .query_row("SELECT value FROM meta WHERE key = 'createKey'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(key, "key-7");
}

#[test]
fn empty_name_rejected() {
    let e = env();
    for name in ["", "   ", "\t\n"] {
        let err = code(Store::create(&e.data, &e.dir, name, secs(30), None).unwrap_err());
        assert_eq!(err.code, "INVALID_NAME", "{name:?}");
        assert_eq!(err.details["field"], "name");
        assert!(err.fix.is_some());
    }
    assert!(!e.dir.exists() && !e.data.exists());
}

#[test]
fn concurrent_creates_in_one_folder_make_one_project() {
    let e = env();
    let results: Vec<Result<String, CoreError>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..6)
            .map(|i| {
                let (data, dir) = (e.data.clone(), e.dir.clone());
                s.spawn(move || {
                    Store::create(&data, &dir, &format!("P{i}"), secs(30), None)
                        .map(|s| s.project_id().to_string())
                        .map_err(code)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let won: Vec<&String> = results.iter().filter_map(|r| r.as_ref().ok()).collect();
    assert_eq!(won.len(), 1, "{results:?}");
    for r in &results {
        if let Err(err) = r {
            assert_eq!(err.code, "PROJECT_EXISTS", "{err:?}");
        }
    }
    assert_eq!(read_project_file(&e.dir).unwrap().id, *won[0]);
    // The losers left no database behind.
    assert_eq!(
        databases(&e.data),
        [format!("projects/{}/project.fdb", won[0])]
    );
}

#[test]
fn concurrent_retries_with_one_key_return_one_project() {
    let e = env();
    let results: Vec<Result<String, CoreError>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let (data, dir) = (e.data.clone(), e.dir.clone());
                s.spawn(move || {
                    Store::create(&data, &dir, "Demo", secs(30), Some("same"))
                        .map(|s| s.project_id().to_string())
                        .map_err(code)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap()).collect()
    });
    let id = read_project_file(&e.dir).unwrap().id;
    for r in &results {
        assert_eq!(
            r.as_ref().map(String::as_str),
            Ok(id.as_str()),
            "{results:?}"
        );
    }
    assert_eq!(databases(&e.data), [format!("projects/{id}/project.fdb")]);
}

/// A retry that lands between the creator's project file and its lease must not take the
/// lease: the creator keeps it.
#[test]
fn a_concurrent_retry_does_not_take_the_creators_lease() {
    let e = env();
    let (creator, retry) = std::thread::scope(|s| {
        let creator = s.spawn(|| {
            fin_store::delay_next_create_start_for_test(std::time::Duration::from_millis(400));
            Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k")).unwrap()
        });
        let retry = s.spawn(|| {
            while !e.dir.join("finisher.project.json").exists() {
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k")).unwrap()
        });
        (creator.join().unwrap(), retry.join().unwrap())
    });
    assert_eq!(retry.project_id(), creator.project_id());
    assert_eq!(creator.read_only(), None);
    assert_eq!(
        retry.read_only(),
        Some(fin_store::ReadOnlyReason::LeaseHeld)
    );
}

#[test]
fn create_key_survives_a_rebuild() {
    let e = env();
    let mut s = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k1")).unwrap();
    let id = s.project_id().to_string();
    s.rebuild().unwrap();
    s.close().unwrap();
    let again = Store::create(&e.data, &e.dir, "Demo", secs(30), Some("k1")).unwrap();
    assert_eq!(again.project_id(), id);
}

/// Stored rationals are read leniently: a hand-edited `"rate": "30/1"` is 30, not an
/// unreadable project.
#[test]
fn a_hand_edited_noncanonical_rate_still_opens() {
    let e = env();
    let s = Store::create(&e.data, &e.dir, "Demo", secs(30), None).unwrap();
    let id = s.project_id().to_string();
    s.close().unwrap();
    let path = e.dir.join("finisher.project.json");
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"rate\": \"30\""), "{text}");
    std::fs::write(
        &path,
        text.replace("\"rate\": \"30\"", "\"rate\": \"30/1\""),
    )
    .unwrap();
    let file = read_project_file(&e.dir).unwrap();
    assert_eq!(file.genesis.unwrap().rate, secs(30));
    // A database rebuilt from that file is the same project (same genesis hash).
    std::fs::remove_file(db_path(&e.data, &id)).unwrap();
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(s.snapshot(MAIN).unwrap().1.rate, secs(30));
    assert!(s.read_only().is_none(), "{:?}", s.read_only());
}
