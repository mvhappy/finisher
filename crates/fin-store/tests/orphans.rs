//! The orphan sweep (`fin_store::orphans`): a crash between create's database rename and its
//! project-file write leaves a marked database in server data. The sweep removes it once the
//! marker is old, the database is not open and its folder shows the create never committed;
//! anything it cannot be sure of (a folder that is missing, as when unmounted) is kept.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use fin_core::Rational;
use fin_store::orphans::{MIN_AGE, sweep};
use fin_store::{Store, read_project_file};
use tempfile::TempDir;

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

fn create(e: &Env, dir: &Path) -> Result<Store, fin_store::StoreError> {
    Store::create(&e.data, dir, "Demo", Rational::from_int(30), None)
}

/// The project ids with a directory in server data.
fn project_dirs(data: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(data.join("projects"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

/// Makes every create marker in `data` look `MIN_AGE` + 1 s old.
fn age_markers(data: &Path) {
    let then = SystemTime::now() - MIN_AGE - Duration::from_secs(1);
    for id in project_dirs(data) {
        let m = data.join("projects").join(id).join("create.pending");
        if let Ok(f) = fs::File::options().write(true).open(&m) {
            f.set_modified(then).unwrap();
        }
    }
}

/// A create that "crashes" after its database rename: the id of the orphan it leaves.
fn crashed_create(e: &Env, dir: &Path) -> String {
    fin_store::crash_next_create_after_db_rename_for_test();
    let err = create(e, dir).unwrap_err();
    assert_eq!(err.code(), "TEST_FAULT");
    let ids = project_dirs(&e.data);
    let id = ids.last().unwrap().clone();
    let pd = e.data.join("projects").join(&id);
    assert!(pd.join("project.fdb").is_file(), "the database is in place");
    assert!(pd.join("create.pending").is_file(), "marked");
    assert!(!dir.join("finisher.project.json").exists());
    id
}

#[test]
fn a_crash_between_the_db_rename_and_the_project_file_is_swept() {
    let e = env();
    let id = crashed_create(&e, &e.dir);
    // Young: a create that may still be running is never touched.
    assert!(sweep(&e.data, MIN_AGE).is_empty());
    assert_eq!(project_dirs(&e.data), std::slice::from_ref(&id));
    age_markers(&e.data);
    assert_eq!(sweep(&e.data, MIN_AGE), [id]);
    assert!(project_dirs(&e.data).is_empty());
    // The folder is free for a new create.
    let s = create(&e, &e.dir).unwrap();
    assert_eq!(project_dirs(&e.data), [s.project_id().to_string()]);
}

#[test]
fn a_finished_create_leaves_no_marker_and_is_never_swept() {
    let e = env();
    let s = create(&e, &e.dir).unwrap();
    let id = s.project_id().to_string();
    let pd = e.data.join("projects").join(&id);
    assert!(!pd.join("create.pending").exists());
    s.close().unwrap();
    assert!(sweep(&e.data, Duration::ZERO).is_empty());
    assert_eq!(project_dirs(&e.data), [id]);
}

#[test]
fn a_marker_left_after_the_commit_point_only_loses_the_marker() {
    // A crash after the project file but before the marker went: a real project.
    let e = env();
    let s = create(&e, &e.dir).unwrap();
    let id = s.project_id().to_string();
    s.close().unwrap();
    let pd = e.data.join("projects").join(&id);
    let abs = std::path::absolute(&e.dir).unwrap();
    fs::write(pd.join("create.pending"), abs.to_str().unwrap()).unwrap();
    age_markers(&e.data);
    assert!(sweep(&e.data, MIN_AGE).is_empty());
    assert!(pd.join("project.fdb").is_file());
    assert!(!pd.join("create.pending").exists(), "the marker is cleared");
    // Opening the project clears one too.
    fs::write(pd.join("create.pending"), abs.to_str().unwrap()).unwrap();
    Store::open(&e.data, &e.dir).unwrap().close().unwrap();
    assert!(!pd.join("create.pending").exists());
}

#[test]
fn an_unreachable_folder_is_never_swept() {
    // The folder is gone (an unmounted share, an offline drive): nothing proves the create
    // failed, so the database stays, however old its marker.
    let e = env();
    let id = crashed_create(&e, &e.dir);
    let away = e.dir.with_file_name("Film (unmounted)");
    fs::rename(&e.dir, &away).unwrap();
    age_markers(&e.data);
    assert!(sweep(&e.data, MIN_AGE).is_empty());
    assert_eq!(project_dirs(&e.data), std::slice::from_ref(&id));
    // Back again, still without a project file: now it is an orphan.
    fs::rename(&away, &e.dir).unwrap();
    assert_eq!(sweep(&e.data, MIN_AGE), [id]);
}

#[test]
fn a_folder_with_finisher_dir_but_no_project_file_is_kept() {
    // `.finisher/` comes after the project file: the file may only be late to show.
    let e = env();
    let id = crashed_create(&e, &e.dir);
    fs::create_dir(e.dir.join(".finisher")).unwrap();
    age_markers(&e.data);
    assert!(sweep(&e.data, MIN_AGE).is_empty());
    assert_eq!(project_dirs(&e.data), [id]);
}

#[test]
fn a_folder_another_create_won_is_swept() {
    let e = env();
    let orphan = crashed_create(&e, &e.dir);
    let winner = create(&e, &e.dir).unwrap();
    assert_eq!(read_project_file(&e.dir).unwrap().id, winner.project_id());
    age_markers(&e.data);
    assert_eq!(sweep(&e.data, MIN_AGE), [orphan]);
    assert_eq!(project_dirs(&e.data), [winner.project_id().to_string()]);
}

#[test]
fn a_database_in_use_is_never_swept() {
    // A marked database another store holds open (a create still running in another
    // process holds its lock from before the marker).
    let e = env();
    let s = create(&e, &e.dir).unwrap();
    let id = s.project_id().to_string();
    let pd = e.data.join("projects").join(&id);
    let other = e.dir.with_file_name("Elsewhere");
    fs::create_dir(&other).unwrap();
    let abs = std::path::absolute(&other).unwrap();
    fs::write(pd.join("create.pending"), abs.to_str().unwrap()).unwrap();
    age_markers(&e.data);
    assert!(
        sweep(&e.data, MIN_AGE).is_empty(),
        "locked by the open store"
    );
    assert!(pd.join("project.fdb").is_file());
    drop(s);
}

#[test]
fn the_first_open_of_a_process_sweeps() {
    let e = env();
    let id = crashed_create(&e, &e.dir);
    age_markers(&e.data);
    // Another project, created in a new server-data path for this process: its create sweeps.
    let other = e.dir.with_file_name("Other");
    let data2 = e.data.with_file_name("server-data-2");
    fs::rename(&e.data, &data2).unwrap();
    let s = Store::create(&data2, &other, "Other", Rational::from_int(30), None).unwrap();
    assert!(
        !project_dirs(&data2).contains(&id),
        "swept on the first create"
    );
    drop(s);
}
