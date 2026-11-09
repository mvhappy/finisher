//! Format 3 (sub-project 5 spec §8.3; plan 5E Decision 4, Review Focus 3): a segment holding a
//! `drop` record is format 3 / min reader 3 and every other keeps its version, so a 4C reader
//! meets `NEWER_FORMAT` only where a `drop` is; database migration 0004 adds the `drops` index;
//! and a project the 4C store wrote opens, migrates and rebuilds to identical state.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{Link, Record, SCHEMA_VERSION, Tx, encode_record};
use fin_store::segment::{self, SegError};
use fin_store::{BranchCreate, DB_VERSION, DropReq, MAIN, OpenOptions, Store};
use integration_support::diverge::all_segments;
use integration_support::*;
use rusqlite::{Connection, OpenFlags};

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

fn versions(s: &segment::Segment) -> (u32, u32) {
    (s.header.format_version, s.header.min_reader_version)
}

fn feature(s: &mut Store, key: &str, name: &str, from: &str) -> String {
    s.create_branch(BranchCreate {
        tx_id: key.into(),
        author: "test".into(),
        name: Some(name.into()),
        from: from.into(),
        at: None,
        dry_run: false,
        args_hash: None,
    })
    .unwrap()
    .branch
    .id
}

fn on(s: &mut Store, id: &str, branch: &str) {
    let base = s.head(branch);
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: branch.into(),
        base_rev: base,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops: vec![insert(0, "a1")],
    })
    .unwrap();
}

fn drop_req(id: &str, from: &str, into: &str, origin: &str) -> DropReq {
    DropReq {
        tx_id: id.into(),
        author: "test".into(),
        into: into.into(),
        from: from.into(),
        drops: vec![origin.into()],
        undrops: vec![],
        dry_run: false,
        args_hash: None,
    }
}

/// `(branch dir, versions)` of every segment file, in file order.
fn all_versions(dir: &Path) -> Vec<(String, (u32, u32))> {
    all_segments(dir)
        .iter()
        .map(|p| {
            let seg = segment::decode(&fs::read(p).unwrap()).unwrap();
            (seg.header.branch.clone(), versions(&seg))
        })
        .collect()
}

#[test]
fn segment_with_drop_is_v3() {
    assert_eq!(
        (
            segment::FORMAT_VERSION,
            segment::MIN_READER_VERSION,
            segment::DROP_VERSION,
            segment::READER_VERSION
        ),
        (2, 2, 3, 3)
    );
    assert_eq!(fin_core::record_format(fin_core::RecordKind::Drop), (3, 3));
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/a.mov")]);
    let br1 = feature(&mut a, "b1", "feature", MAIN);
    on(&mut a, "e1", &br1);
    assert!(e.flush(&mut a) >= 1);
    let had = all_segments(&e.dir).len();
    // A drop on main (edit-only so far) and one on a branch (format 2 so far).
    a.drop(drop_req("d1", &br1, MAIN, "e1")).unwrap();
    let br2 = feature(&mut a, "b2", "child", &br1);
    on(&mut a, "x1", &br2);
    a.drop(drop_req("d2", &br2, &br1, "x1")).unwrap();
    e.flush(&mut a);
    let files = all_segments(&e.dir);
    assert!(files.len() > had);
    let with_drop: Vec<(String, (u32, u32))> = files
        .iter()
        .filter_map(|p| {
            let seg = segment::decode(&fs::read(p).unwrap()).unwrap();
            let lines: Vec<&[u8]> = seg.records.iter().map(Vec::as_slice).collect();
            let has = seg
                .records
                .iter()
                .any(|r| String::from_utf8_lossy(r).contains("\"kind\":\"drop\""));
            assert_eq!(
                segment::version_for_branch(&seg.header.branch, &lines),
                versions(&seg)
            );
            has.then(|| (seg.header.branch.clone(), versions(&seg)))
        })
        .collect();
    assert_eq!(
        with_drop,
        [(br1.clone(), (3, 3)), (MAIN.to_string(), (3, 3))]
    );
    // The bytes alone decide it.
    let edit = encode_record(&record("e", 1, Link::Edit));
    let dropped = encode_record(&record(
        "d",
        2,
        Link::new_drop("br1", vec!["e1".into()], vec![]).unwrap(),
    ));
    assert_eq!(segment::version_for(&[edit.as_slice()]), (1, 1));
    assert_eq!(segment::version_for(&[edit.as_slice(), &dropped]), (3, 3));
    assert_eq!(
        segment::version_for_branch("br1", &[edit.as_slice()]),
        (2, 2)
    );
    assert_eq!(
        segment::version_for_branch("br1", &[edit.as_slice(), &dropped]),
        (3, 3)
    );
}

#[test]
fn segment_without_drop_keeps_version() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t0", vec![asset("/m/a.mov")]);
    e.flush(&mut a);
    let br1 = feature(&mut a, "b1", "feature", MAIN);
    on(&mut a, "e1", &br1);
    e.flush(&mut a);
    a.drop(drop_req("d1", &br1, MAIN, "e1")).unwrap();
    e.flush(&mut a);
    // After a drop, segments without one are written at their 4C versions again.
    on(&mut a, "m1", MAIN);
    on(&mut a, "e2", &br1);
    e.flush(&mut a);
    let branch: Vec<(u32, u32)> = all_versions(&e.dir)
        .into_iter()
        .filter(|(b, _)| *b == br1)
        .map(|(_, v)| v)
        .collect();
    assert_eq!(branch, [(2, 2), (2, 2)]);
    let main: Vec<(u32, u32)> = segments(&e.dir).iter().map(|s| versions(&s.2)).collect();
    // t0 alone; the branch record; the drop; m1 alone.
    assert_eq!(main, [(1, 1), (2, 2), (3, 3), (1, 1)]);
    // A pack is the highest of what it holds.
    let r = a.compact(true).unwrap();
    assert!(r.packs_written >= 1, "{r:?}");
    let packs: Vec<(u32, u32)> = segments(&e.dir)
        .iter()
        .filter(|s| s.2.header.pack)
        .map(|s| versions(&s.2))
        .collect();
    assert_eq!(packs, [(3, 3)]);
}

/// A record of kind `link` at `seq` on `main`, with no ops.
fn record(tx_id: &str, seq: u64, link: Link) -> Record {
    let intent = Tx {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: seq - 1,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        ops: vec![],
    };
    let mut r = Record::new_edit(
        tx_id.into(),
        seq,
        SCHEMA_VERSION,
        None,
        "test".into(),
        1_800_000_000_000,
        intent,
        vec![],
        vec![],
        Default::default(),
    );
    r.set_link(link);
    r
}

fn header(version: (u32, u32)) -> segment::Header {
    segment::Header {
        format_version: version.0,
        min_reader_version: version.1,
        project_id: "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        branch: MAIN.into(),
        writer_id: "01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
        epoch: 1,
        first_seq: 1,
        last_seq: 1,
        prev_segment_hash: None,
        op_schema_versions: [2, 2],
        created_at: 1,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
        branch_record: None,
        pack: false,
        replaces: vec![],
    }
}

#[test]
fn v2_reader_gets_newer_format() {
    let drop = encode_record(&record(
        "d1",
        1,
        Link::new_drop("br1", vec!["e1".into()], vec![]).unwrap(),
    ));
    let lines = [drop.as_slice()];
    let (bytes, hash) = segment::encode(&header(segment::version_for(&lines)), &lines);
    assert_eq!(segment::decode(&bytes).unwrap().hash, hash);
    // The 4C reader (version 2) and the 4A one: newer, never misread.
    for reader in [2, 1] {
        assert_eq!(
            segment::decode_with_reader(&bytes, reader),
            Err(SegError::Newer { min_reader: 3 })
        );
    }
    // A format 2 file stays readable by a 4C reader.
    let edit = encode_record(&record("e1", 1, Link::Edit));
    let (v2, _) = segment::encode(&header((2, 2)), &[&edit]);
    assert!(segment::decode_with_reader(&v2, 2).is_ok());
    // Above our reader version is newer for us too.
    let (v4, _) = segment::encode(&header((4, 4)), &[&drop]);
    assert_eq!(segment::decode(&v4), Err(SegError::Newer { min_reader: 4 }));
}

#[test]
fn drop_in_older_format_is_corrupt() {
    // A drop needs format 3: a segment labelled 1 or 2 that holds one is damaged (a 4C reader
    // would otherwise read it as if the drop were not there), on main and on a branch.
    let drop = encode_record(&record(
        "d1",
        1,
        Link::new_drop("br1", vec!["e1".into()], vec![]).unwrap(),
    ));
    let lines = [drop.as_slice()];
    for version in [(1, 1), (2, 2)] {
        let (bytes, _) = segment::encode(&header(version), &lines);
        assert!(
            matches!(segment::decode(&bytes), Err(SegError::Corrupt(ref w)) if w.contains("drop")),
            "{version:?}: {:?}",
            segment::decode(&bytes)
        );
    }
    let mut branch = header((2, 2));
    branch.branch = "br2".into();
    branch.branch_record = Some(format!("blake3:{}", "1".repeat(64)));
    let (bytes, _) = segment::encode(&branch, &lines);
    assert!(matches!(
        segment::decode(&bytes),
        Err(SegError::Corrupt(ref w)) if w.contains("drop")
    ));
}

fn expected_4c() -> serde_json::Value {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/4c/expected.json");
    serde_json::from_slice(&fs::read(p).unwrap()).unwrap()
}

fn assert_matches_4c(store: &Store, expected: &serde_json::Value, what: &str) {
    for (branch, want) in expected["branches"].as_object().unwrap() {
        let (rev, project) = store.snapshot(branch).unwrap();
        assert_eq!(rev, want["rev"].as_u64().unwrap(), "{what}: {branch} rev");
        assert_eq!(
            serde_json::to_value(&project).unwrap(),
            want["project"],
            "{what}: {branch} project"
        );
        let stacks = store.undo_stacks(branch, "fixture").unwrap();
        assert_eq!(
            serde_json::to_value(&stacks).unwrap(),
            expected["stacks"][branch],
            "{what}: {branch} stacks"
        );
    }
    assert_eq!(
        serde_json::to_value(store.branches(true).unwrap()).unwrap(),
        expected["views"],
        "{what}: branches"
    );
    let v = store.verify();
    assert!(v.ok, "{what}: {v:#?}");
}

fn bytes_of(files: &[PathBuf]) -> Vec<Vec<u8>> {
    files.iter().map(|p| fs::read(p).unwrap()).collect()
}

#[test]
fn fixture_4c_opens_identically() {
    let e = env();
    let (data, folder) = fixture_4c(&e.root);
    let expected = expected_4c();
    let id = expected["branches"]["main"]["project"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let db = fin_store::db_path(&data, &id);
    let v: i64 = read_only(&db)
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 3, "the fixture is a 4C database");
    let files = all_segments(&folder);
    let before = bytes_of(&files);
    assert_eq!(
        all_versions(&folder),
        [("br1".to_string(), (2, 2)), (MAIN.to_string(), (2, 2))]
    );

    let opts = OpenOptions {
        relocate: true,
        ..e.opts()
    };
    let mut store = Store::open_with(&data, &folder, opts).unwrap();
    assert_matches_4c(&store, &expected, "migrated");
    let conn = read_only(store.db_path());
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!((v, DB_VERSION), (4, 4));
    assert_eq!(meta(&conn, "formatVersion").as_deref(), Some("4"));
    assert_eq!(meta(&conn, "minReaderVersion").as_deref(), Some("4"));
    let rows: i64 = conn
        .query_row("SELECT COUNT(*) FROM drops", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    drop(conn);
    assert_eq!(bytes_of(&files), before, "no segment rewritten");

    // A rebuild from the folder alone gives the same state.
    store.rebuild().unwrap();
    assert_matches_4c(&store, &expected, "rebuilt");
    assert_eq!(bytes_of(&files), before);

    // The upgraded project takes drops; only their segment is format 3.
    store.set_test_lease(2, true);
    on(&mut store, "z1", "br1");
    store.drop(drop_req("dz", "br1", MAIN, "z1")).unwrap();
    assert_eq!(
        store.dropped(MAIN).unwrap().into_iter().collect::<Vec<_>>(),
        ["z1"]
    );
    e.flush(&mut store);
    assert_eq!(bytes_of(&files), before);
    let mut got: Vec<(u32, u32)> = all_versions(&folder).into_iter().map(|v| v.1).collect();
    got.sort();
    assert_eq!(got, [(2, 2), (2, 2), (2, 2), (3, 3)]);
    let v = store.verify();
    assert!(v.ok, "{v:#?}");
}
