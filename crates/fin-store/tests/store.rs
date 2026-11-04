//! `fin-store` behaviour: create/open, commit, rebase-on-commit, idempotency, crash safety.

use std::path::{Path, PathBuf};

use fin_core::{CoreError, Op, Project, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::{MAIN, Store, StoreError, db_path, read_project_file, replay};
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
    let data = tmp.path().join("server-data");
    let dir = tmp.path().join("My Project");
    Env {
        data,
        dir,
        _tmp: tmp,
    }
}

fn create(e: &Env) -> Store {
    Store::create(&e.data, &e.dir, "Demo", secs(30)).unwrap()
}

fn tx(id: &str, base_rev: u64, ops: Vec<Op>) -> Tx {
    Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev,
        schema_version: SCHEMA_VERSION,
        ops,
    }
}

fn asset(id: &str) -> Op {
    Op::AssetAdd {
        id: id.into(),
        path: "/media/a.mov".into(),
        duration: secs(60),
        video: Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: secs(30),
        }),
        audio: None,
        fingerprint: "fp".into(),
    }
}

fn insert(id: &str, at: i64, asset: &str, duration: i64) -> Op {
    Op::ClipInsert {
        id: id.into(),
        track: "t1".into(),
        at: secs(at),
        asset: asset.into(),
        source_in: secs(0),
        duration: secs(duration),
    }
}

fn clips(p: &Project) -> Vec<(String, Rational, Rational)> {
    p.timeline.tracks[0]
        .items
        .iter()
        .map(|c| (c.id.clone(), c.start, c.duration))
        .collect()
}

/// Store with asset `a1` committed at rev 1.
fn with_asset(e: &Env) -> Store {
    let mut s = create(e);
    s.commit(tx("tx-asset", 0, vec![asset("$a")])).unwrap();
    s
}

fn code(e: StoreError) -> CoreError {
    CoreError::from(e)
}

#[test]
fn create_then_open_round_trips() {
    let e = env();
    let s = create(&e);
    let id = s.project_id().to_string();
    assert_eq!(id.len(), 26);
    assert!(
        id.bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase()),
        "{id}"
    );
    assert_eq!(s.db_path(), db_path(&e.data, &id));
    assert!(s.db_path().is_file());
    assert!(
        !e.dir.join("project.fdb").exists(),
        "the db never lives in the project folder"
    );
    let file = read_project_file(&e.dir).unwrap();
    assert_eq!(
        (file.id.as_str(), file.name.as_str(), file.schema_version),
        (id.as_str(), "Demo", SCHEMA_VERSION)
    );
    let (rev, project) = s.snapshot(MAIN).unwrap();
    drop(s);

    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(s.project_id(), id);
    assert_eq!(s.head(MAIN), 0);
    assert_eq!(s.snapshot(MAIN).unwrap(), (rev, project));
}

#[test]
fn create_refuses_an_existing_project() {
    let e = env();
    create(&e);
    let err = code(Store::create(&e.data, &e.dir, "Again", secs(30)).unwrap_err());
    assert_eq!(err.code, "PROJECT_EXISTS");
}

#[test]
fn create_under_a_file_is_project_dir_unwritable() {
    let e = env();
    std::fs::create_dir_all(&e.dir).unwrap();
    let file = e.dir.join("not-a-dir");
    std::fs::write(&file, b"x").unwrap();
    let dir = file.join("film");
    let err = code(Store::create(&e.data, &dir, "Film", secs(30)).unwrap_err());
    assert_eq!(err.code, "PROJECT_DIR_UNWRITABLE");
    assert_eq!(err.details["path"], dir.to_string_lossy().as_ref());
    assert!(
        err.details["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty())
    );
    assert!(err.fix.is_some());
}

#[test]
fn create_rejects_a_bad_rate_without_touching_disk() {
    let e = env();
    let err = code(Store::create(&e.data, &e.dir, "Demo", secs(0)).unwrap_err());
    assert_eq!(err.code, "INVALID_RANGE");
    assert!(!e.dir.exists() && !e.data.exists());
}

#[test]
fn open_without_db_is_project_db_missing() {
    let e = env();
    let s = create(&e);
    let db = s.db_path().to_path_buf();
    drop(s);
    std::fs::remove_dir_all(db.parent().unwrap()).unwrap();
    let err = code(Store::open(&e.data, &e.dir).unwrap_err());
    assert_eq!(err.code, "PROJECT_DB_MISSING");
}

#[test]
fn open_without_project_file_is_project_not_found() {
    let e = env();
    assert_eq!(
        code(Store::open(&e.data, &e.dir).unwrap_err()).code,
        "PROJECT_NOT_FOUND"
    );
}

#[test]
fn open_rejects_an_id_that_is_not_a_ulid() {
    let e = env();
    std::fs::create_dir_all(&e.dir).unwrap();
    std::fs::write(
        e.dir.join("finisher.project.json"),
        r#"{"id":"../../etc","name":"x","schemaVersion":1}"#,
    )
    .unwrap();
    assert_eq!(
        code(Store::open(&e.data, &e.dir).unwrap_err()).code,
        "PROJECT_INVALID"
    );
}

#[test]
fn commit_returns_rev_1_and_bindings() {
    let e = env();
    let mut s = create(&e);
    let c = s
        .commit(tx("tx-1", 0, vec![asset("$a"), insert("$c", 0, "$a", 5)]))
        .unwrap();
    assert_eq!(c.rev, 1);
    assert_eq!(c.bindings.get("$a").map(String::as_str), Some("a1"));
    assert_eq!(c.bindings.get("$c").map(String::as_str), Some("c1"));
    assert_eq!(c.tx.base_rev, 0);
    assert_eq!(s.head(MAIN), 1);
    assert_eq!(
        clips(&s.snapshot(MAIN).unwrap().1),
        vec![("c1".to_string(), secs(0), secs(5))]
    );
    assert_eq!(s.ops_since(MAIN, 0).unwrap(), vec![c]);
}

#[test]
fn commit_stale_base_rebases() {
    let e = env();
    let mut s = with_asset(&e);
    s.commit(tx("tx-base", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    // Two writers both read rev 2 and insert at the end of c1 (5s) on t1.
    let a = s
        .commit(tx("tx-a", 2, vec![insert("$x", 5, "a1", 3)]))
        .unwrap();
    let b = s
        .commit(tx("tx-b", 2, vec![insert("$y", 5, "a1", 2)]))
        .unwrap();
    assert_eq!((a.rev, b.rev), (3, 4));
    assert_eq!(b.tx.base_rev, 3, "the stored tx is the rebased one");
    match &b.tx.ops[0] {
        Op::ClipInsert { at, .. } => assert_eq!(*at, secs(8)),
        op => panic!("unexpected {op:?}"),
    }
    let (rev, p) = s.snapshot(MAIN).unwrap();
    assert_eq!(rev, 4);
    assert_eq!(
        clips(&p),
        vec![
            ("c1".to_string(), secs(0), secs(5)),
            ("c2".to_string(), secs(5), secs(3)),
            ("c3".to_string(), secs(8), secs(2))
        ]
    );
}

#[test]
fn conflicting_stale_tx_is_conflict_with_both_tx_ids() {
    let e = env();
    let mut s = with_asset(&e);
    s.commit(tx("tx-base", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    s.commit(tx("tx-del", 2, vec![Op::ClipDelete { clip: "c1".into() }]))
        .unwrap();
    let before = s.snapshot(MAIN).unwrap();
    let err = code(
        s.commit(tx("tx-late", 2, vec![Op::ClipDelete { clip: "c1".into() }]))
            .unwrap_err(),
    );
    assert_eq!(err.code, "CONFLICT");
    assert_eq!(err.details["againstTx"], "tx-del");
    assert_eq!(err.details["txId"], "tx-late");
    assert_eq!(s.snapshot(MAIN).unwrap(), before);
    assert_eq!(s.head(MAIN), 3);
}

#[test]
fn duplicate_tx_id_returns_the_committed_tx() {
    let e = env();
    let mut s = with_asset(&e);
    let first = s
        .commit(tx("tx-1", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    let again = s
        .commit(tx("tx-1", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    assert_eq!(again, first);
    assert_eq!(s.head(MAIN), 2);
    drop(s);
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(
        s.commit(tx("tx-1", 0, vec![asset("$other")])).unwrap(),
        first,
        "dedupe survives reopen"
    );
    assert_eq!(s.head(MAIN), 2);
}

#[test]
fn empty_tx_id_or_ops_is_invalid_op() {
    let e = env();
    let mut s = with_asset(&e);
    let committed = s.commit(tx("tx-1", 1, vec![asset("$b")])).unwrap();
    for (t, field) in [
        (tx("", 2, vec![asset("$c")]), "txId"),
        (tx("  ", 2, vec![asset("$c")]), "txId"),
        (tx("tx-2", 2, vec![]), "ops"),
        // Even for a committed txId: an empty retry is malformed, not a replay.
        (tx("tx-1", 1, vec![]), "ops"),
    ] {
        let err = code(s.commit(t).unwrap_err());
        assert_eq!(
            (err.code.as_str(), &err.details["field"]),
            ("INVALID_OP", &serde_json::json!(field))
        );
        assert!(err.fix.is_some());
    }
    assert_eq!(s.head(MAIN), committed.rev);
}

#[test]
fn base_rev_ahead_of_head_is_rejected() {
    let e = env();
    let mut s = create(&e);
    assert_eq!(
        code(s.commit(tx("tx-1", 5, vec![asset("$a")])).unwrap_err()).code,
        "BASE_REV_AHEAD"
    );
}

#[test]
fn unknown_branch_is_branch_not_found() {
    let e = env();
    let mut s = create(&e);
    let mut t = tx("tx-1", 0, vec![asset("$a")]);
    t.branch = "feature".into();
    assert_eq!(code(s.commit(t).unwrap_err()).code, "BRANCH_NOT_FOUND");
    assert_eq!(
        code(s.snapshot("feature").unwrap_err()).code,
        "BRANCH_NOT_FOUND"
    );
}

#[test]
fn reopen_after_uncommitted_tx() {
    let e = env();
    let mut s = with_asset(&e);
    s.commit(tx("tx-1", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    let before = s.snapshot(MAIN).unwrap();
    s.fail_next_commit_after_apply();
    let err = code(
        s.commit(tx("tx-crash", 2, vec![insert("$x", 5, "a1", 3)]))
            .unwrap_err(),
    );
    assert_eq!(err.code, "TEST_FAULT");
    assert_eq!(
        s.snapshot(MAIN).unwrap(),
        before,
        "in-memory head untouched"
    );
    drop(s);

    let mut s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(
        s.snapshot(MAIN).unwrap(),
        before,
        "head is the last fully committed tx"
    );
    assert_eq!(s.ops_since(MAIN, 2).unwrap(), vec![]);
    // The crashed tx neither landed nor consumed its id; it can be retried under the same txId.
    let c = s
        .commit(tx("tx-crash", 2, vec![insert("$x", 5, "a1", 3)]))
        .unwrap();
    assert_eq!((c.rev, c.bindings["$x"].as_str()), (3, "c2"));
}

#[test]
fn failed_commit_does_not_consume_ids() {
    let e = env();
    let mut s = with_asset(&e);
    // Op 0 binds an id, op 1 fails validation: the whole tx is rejected.
    let bad = tx(
        "tx-bad",
        1,
        vec![insert("$c", 0, "a1", 5), insert("$d", 0, "a404", 5)],
    );
    assert!(s.commit(bad).is_err());
    let c = s
        .commit(tx("tx-good", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    assert_eq!(c.bindings["$c"], "c1");
}

#[test]
fn ids_are_never_reused_after_reopen() {
    let e = env();
    let mut s = with_asset(&e);
    s.commit(tx("tx-1", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    s.commit(tx("tx-2", 2, vec![Op::ClipDelete { clip: "c1".into() }]))
        .unwrap();
    drop(s);
    let mut s = Store::open(&e.data, &e.dir).unwrap();
    let c = s
        .commit(tx("tx-3", 3, vec![asset("$b"), insert("$c", 0, "$b", 5)]))
        .unwrap();
    assert_eq!(
        (c.bindings["$b"].as_str(), c.bindings["$c"].as_str()),
        ("a2", "c2")
    );
}

#[test]
fn ops_since_zero_replays_to_the_snapshot() {
    let e = env();
    let mut s = with_asset(&e);
    s.commit(tx(
        "tx-1",
        1,
        vec![insert("$c", 0, "a1", 5), insert("$d", 5, "a1", 4)],
    ))
    .unwrap();
    s.commit(tx(
        "tx-2",
        2,
        vec![Op::ClipTrim {
            clip: "c1".into(),
            edge: fin_core::Edge::End,
            delta: secs(-1),
        }],
    ))
    .unwrap();
    s.commit(tx("tx-3", 2, vec![insert("$e", 0, "a1", 2)]))
        .unwrap(); // stale: rebased
    s.commit(tx("tx-4", 4, vec![Op::ClipDelete { clip: "c2".into() }]))
        .unwrap();
    let history = s.ops_since(MAIN, 0).unwrap();
    assert_eq!(history.len() as u64, s.head(MAIN));
    let head = s.snapshot(MAIN).unwrap().1;
    let zero =
        fin_core::new_project(s.project_id().to_string(), head.name.clone(), head.rate).unwrap();
    let replayed = history.iter().try_fold(zero, |p, c| replay(&p, c)).unwrap();
    assert_eq!(replayed, head);
    drop(s);
    assert_eq!(
        Store::open(&e.data, &e.dir)
            .unwrap()
            .snapshot(MAIN)
            .unwrap()
            .1,
        head
    );
}

#[test]
fn reopen_replays_past_a_periodic_snapshot() {
    let e = env();
    let mut s = with_asset(&e);
    for i in 1..=(fin_store::SNAPSHOT_EVERY + 5) {
        s.commit(tx(
            &format!("tx-{i}"),
            i,
            vec![insert(&format!("$c{i}"), 0, "a1", 1)],
        ))
        .unwrap();
    }
    let expected = s.snapshot(MAIN).unwrap();
    drop(s);
    let s = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(s.snapshot(MAIN).unwrap(), expected);
}

#[test]
fn ukrainian_name_and_dir_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("дані сервера");
    let dir: &Path = &tmp.path().join("Проєкти").join("Мій фільм — ґанок");
    let s = Store::create(&data, dir, "Літній відпочинок у Львові", secs(25)).unwrap();
    let id = s.project_id().to_string();
    drop(s);
    let raw = std::fs::read_to_string(dir.join("finisher.project.json")).unwrap();
    assert!(raw.contains("Літній відпочинок у Львові"), "{raw}");
    let s = Store::open(&data, dir).unwrap();
    assert_eq!(s.project_id(), id);
    assert_eq!(
        s.snapshot(MAIN).unwrap().1.name,
        "Літній відпочинок у Львові"
    );
}

#[test]
fn two_stores_on_one_project_interleave_commits() {
    let e = env();
    let mut a = with_asset(&e);
    let mut b = Store::open(&e.data, &e.dir).unwrap();
    assert_eq!(b.head(MAIN), 1);

    let a1 = a
        .commit(tx("tx-a1", 1, vec![insert("$x", 0, "a1", 5)]))
        .unwrap();
    assert_eq!(b.head(MAIN), 2, "B sees A's commit");
    assert_eq!(b.snapshot(MAIN).unwrap(), a.snapshot(MAIN).unwrap());

    // B still thinks in terms of rev 1: its tx is rebased over A's (the committed insert at the
    // same point stays first), not rejected.
    let b1 = b
        .commit(tx("tx-b1", 1, vec![insert("$y", 0, "a1", 2)]))
        .unwrap();
    let a2 = a
        .commit(tx("tx-a2", 3, vec![insert("$z", 7, "a1", 1)]))
        .unwrap();
    let b2 = b
        .commit(tx("tx-b2", 4, vec![Op::ClipDelete { clip: "c3".into() }]))
        .unwrap();
    assert_eq!([a1.rev, b1.rev, a2.rev, b2.rev], [2, 3, 4, 5]);
    assert_eq!(
        [&a1.bindings["$x"], &b1.bindings["$y"], &a2.bindings["$z"]],
        ["c1", "c2", "c3"],
        "ids are unique across stores"
    );

    let (rev, p) = b.snapshot(MAIN).unwrap();
    assert_eq!(rev, 5);
    assert_eq!(a.snapshot(MAIN).unwrap(), (rev, p.clone()));
    assert_eq!(
        clips(&p),
        vec![
            ("c1".to_string(), secs(0), secs(5)),
            ("c2".to_string(), secs(5), secs(2)),
        ]
    );
    assert_eq!(a.ops_since(MAIN, 1).unwrap(), b.ops_since(MAIN, 1).unwrap());
}

#[test]
fn same_tx_id_via_two_stores_is_idempotent() {
    let e = env();
    let mut a = with_asset(&e);
    let mut b = Store::open(&e.data, &e.dir).unwrap();
    let first = a
        .commit(tx("tx-same", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    let again = b
        .commit(tx("tx-same", 1, vec![insert("$c", 0, "a1", 5)]))
        .unwrap();
    assert_eq!(again, first);
    assert_eq!((a.head(MAIN), b.head(MAIN)), (2, 2));
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// The committed sub-project 2 fixture opens (snapshot at rev 200 + tail) and replays from rev 0
/// (every tx, through `apply_committed`) to the state it was generated with.
#[test]
fn sp2_fixture_opens_and_replays_to_expected_state() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2");
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixture, tmp.path());
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read(tmp.path().join("expected.json")).unwrap()).unwrap();
    let store = Store::open(&tmp.path().join("server-data"), &tmp.path().join("folder")).unwrap();
    let (rev, head) = store.snapshot(MAIN).unwrap();
    assert_eq!(rev, expected["rev"].as_u64().unwrap());
    assert_eq!(serde_json::to_value(&head).unwrap(), expected["project"]);

    let history = store.ops_since(MAIN, 0).unwrap();
    assert_eq!(history.len() as u64, rev);
    let zero = fin_core::new_project(head.id.clone(), head.name.clone(), head.rate).unwrap();
    let replayed = history.iter().try_fold(zero, |p, c| replay(&p, c)).unwrap();
    assert_eq!(replayed, head);
}
