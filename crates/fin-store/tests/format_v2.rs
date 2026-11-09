//! Sub-project 4B format (plan Task 3): record kinds in segments whose version follows their
//! content, per-file-kind reader versions, and DB migration 0002 from a 4A database.

mod integration_support;

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{Diff, Link, Record, SCHEMA_VERSION, Tx, encode_record, upcast_record};
use fin_store::segment::{self, SegError};
use fin_store::snapfile::{self, SnapHeader};
use fin_store::{DB_VERSION, MAIN, OpenOptions, Store};
use integration_support::*;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::json;

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn expected_4a() -> serde_json::Value {
    serde_json::from_slice(&fs::read(fixture_root().join("4a/expected_v2.json")).unwrap()).unwrap()
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

/// `(id, name, parent, fork_seq, fork_hash, archived, merged)` of every `branches` row.
type BranchRow = (
    String,
    String,
    Option<String>,
    i64,
    Option<String>,
    i64,
    i64,
);

fn branch_rows(conn: &Connection) -> Vec<BranchRow> {
    let mut stmt = conn
        .prepare(
            "SELECT id, name, parent, fork_seq, fork_hash, archived, merged FROM branches
             ORDER BY id",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn main_only() -> Vec<BranchRow> {
    vec![(MAIN.into(), MAIN.into(), None, 0, None, 0, 0)]
}

/// `(tx_id, kind, origin, from_branch, merge_id, targets, name)` of every `record_links` row.
type LinkRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn link_rows(conn: &Connection) -> Vec<LinkRow> {
    let mut stmt = conn
        .prepare(
            "SELECT tx_id, kind, origin, from_branch, merge_id, targets, name FROM record_links
             ORDER BY branch, seq",
        )
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
            r.get(6)?,
        ))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn backups(db: &Path) -> Vec<String> {
    let dir = db.parent().unwrap().join("backups");
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
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
        op_schema_versions: [1, 1],
        created_at: 1,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
        branch_record: None,
        pack: false,
        replaces: vec![],
    }
}

/// A record of kind `link` at `seq` after `parent`, with no ops.
fn linked_record(tx_id: &str, seq: u64, parent: Option<String>, link: Link) -> Record {
    let intent = Tx {
        tx_id: tx_id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: seq - 1,
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops: vec![],
    };
    let mut r = Record::new_edit(
        tx_id.into(),
        seq,
        SCHEMA_VERSION,
        parent,
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

/// Appends a record of kind `link` to `main` of the open store `s`, behind its back: the
/// store's own write path for 4B kinds comes in a later task. Unexported, at the held epoch.
fn plant(s: &Store, tx_id: &str, link: Link) -> Vec<u8> {
    let conn = Connection::open(s.db_path()).unwrap();
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let (seq, tip): (i64, String) = conn
        .query_row(
            "SELECT seq, tx_hash FROM txs WHERE branch = 'main' ORDER BY seq DESC LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let seq = seq as u64 + 1;
    let record = linked_record(tx_id, seq, Some(tip), link);
    let bytes = encode_record(&record);
    let hash = format!("blake3:{}", blake3::hash(&bytes).to_hex());
    let epoch = held_epoch(s).unwrap();
    conn.execute(
        "INSERT INTO txs (seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch,
                          exported, diff, schema_version, author, committed_at)
         VALUES (?1, 'main', ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9, 'test', ?10)",
        params![
            seq as i64,
            tx_id,
            serde_json::to_value(record.kind).unwrap().as_str().unwrap(),
            bytes,
            hash,
            record.parent_hash,
            epoch as i64,
            serde_json::to_string(&Diff::default()).unwrap(),
            SCHEMA_VERSION,
            record.committed_at,
        ],
    )
    .unwrap();
    bytes
}

fn versions(s: &segment::Segment) -> (u32, u32) {
    (s.header.format_version, s.header.min_reader_version)
}

fn branch_link() -> Link {
    Link::Branch {
        branch_id: "br1".into(),
        name: "alt".into(),
        fork_seq: 0,
        fork_hash: None,
        epoch: 1,
        recovers: None,
    }
}

#[test]
fn edit_record_bytes_unchanged_from_4a() {
    let dir = fixture_root().join("4a/folder/.finisher/segments/main");
    let mut n = 0;
    for entry in fs::read_dir(dir).unwrap() {
        let seg = segment::decode(&fs::read(entry.unwrap().path()).unwrap()).unwrap();
        assert_eq!(versions(&seg), (1, 1), "4A wrote format 1");
        for line in &seg.records {
            // 4A's bytes, as 4A wrote them: schema 1, `clip.*` ops, never re-encoded.
            let raw: serde_json::Value = serde_json::from_slice(line).unwrap();
            assert_eq!(
                (&raw["schemaVersion"], &raw["intent"]["schemaVersion"]),
                (&json!(1), &json!(1))
            );
            // Read as v2 (plan 4C Task 7): the versions are stamped and the ops upgraded;
            // ids, times and bindings are as written.
            let up = upcast_record(raw.clone()).unwrap();
            assert_eq!(
                (up.schema_version, up.intent.schema_version),
                (SCHEMA_VERSION, SCHEMA_VERSION)
            );
            assert_eq!(
                (json!(up.tx_id), json!(up.committed_at)),
                (raw["txId"].clone(), raw["committedAt"].clone())
            );
            assert_eq!(serde_json::to_value(&up.bindings).unwrap(), raw["bindings"]);
            assert!(
                up.intent
                    .ops
                    .iter()
                    .chain(&up.resolved)
                    .chain(&up.inverse)
                    .all(|o| !o.is_alias())
            );
            assert_eq!(up.link().unwrap(), Link::Edit);
            n += 1;
        }
    }
    assert_eq!(n, expected_4a()["rev"].as_u64().unwrap());
}

#[test]
fn edit_only_segment_is_v1() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    commit(&mut a, "t2", vec![insert(0, "a1")]);
    assert_eq!(e.flush(&mut a), 1);
    let all = segments(&e.dir);
    assert_eq!(all.len(), 1);
    assert_eq!(versions(&all[0].2), (1, 1));
    let lines: Vec<&[u8]> = all[0].2.records.iter().map(Vec::as_slice).collect();
    assert_eq!(segment::version_for(&lines), (1, 1));
    assert!(
        String::from_utf8(fs::read(&all[0].0).unwrap())
            .unwrap()
            .starts_with(
                "{\"magic\":\"finisher-segment\",\"formatVersion\":1,\"minReaderVersion\":1,"
            )
    );
}

#[test]
fn segment_with_branch_record_is_v2() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    let edit = a.ops_since(MAIN, 0).unwrap();
    assert_eq!(edit.len(), 1);
    let branch = plant(&a, "b1", branch_link());
    assert_eq!(e.flush(&mut a), 1);
    let all = segments(&e.dir);
    assert_eq!(all.len(), 1);
    assert_eq!(versions(&all[0].2), (2, 2));
    assert_eq!(all[0].2.records[1], branch);
    assert_eq!(
        (
            segment::FORMAT_VERSION,
            segment::MIN_READER_VERSION,
            segment::READER_VERSION
        ),
        (2, 2, 2)
    );
    assert_eq!(segment::version_for(&[branch.as_slice()]), (2, 2));
    let first = all[0].2.records[0].as_slice();
    assert_eq!(segment::version_for(&[first]), (1, 1));
    assert_eq!(segment::version_for(&[first, branch.as_slice()]), (2, 2));
}

#[test]
fn reads_v1_and_v2_segments() {
    let e = env();
    let mut a = e.create("a");
    commit(&mut a, "t1", vec![asset("/m/a.mov")]);
    assert_eq!(e.flush(&mut a), 1);
    plant(
        &a,
        "k1",
        Link::Checkpoint {
            name: "cut".into(),
            recovers: None,
        },
    );
    assert_eq!(e.flush(&mut a), 1);
    let all = segments(&e.dir);
    let got: Vec<(u32, u32)> = all.iter().map(|s| versions(&s.2)).collect();
    assert_eq!(got, [(1, 1), (2, 2)]);

    // A follower imports both, learning the checkpoint's link.
    let b = e.open("b");
    assert_eq!(b.head(MAIN), 2);
    // A checkpoint changes nothing.
    assert_eq!(b.snapshot(MAIN).unwrap().1, a.snapshot(MAIN).unwrap().1);
    let conn = read_only(b.db_path());
    assert_eq!(
        link_rows(&conn),
        [(
            "k1".to_string(),
            "checkpoint".to_string(),
            None,
            None,
            None,
            None,
            Some("cut".to_string())
        )]
    );
    let kind: String = conn
        .query_row("SELECT kind FROM txs WHERE tx_id = 'k1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kind, "checkpoint");
    // Its committed form carries the link; edits carry none.
    let txs = b.ops_since(MAIN, 0).unwrap();
    assert_eq!(txs[0].link, None);
    assert_eq!(
        txs[1].link.as_deref(),
        Some(&Link::Checkpoint {
            name: "cut".into(),
            recovers: None
        })
    );
}

#[test]
fn pack_version_is_highest_of_contents() {
    let pack_versions = |with_checkpoint: bool| {
        let e = env();
        let mut a = e.create("a");
        for i in 0..2 {
            commit(
                &mut a,
                &format!("t{i}"),
                vec![asset(&format!("/m/{i}.mov"))],
            );
            assert_eq!(e.flush(&mut a), 1);
        }
        if with_checkpoint {
            plant(
                &a,
                "k1",
                Link::Checkpoint {
                    name: "cut".into(),
                    recovers: None,
                },
            );
            assert_eq!(e.flush(&mut a), 1);
        }
        let r = a.compact(true).unwrap();
        assert_eq!(r.packs_written, 1, "{r:?}");
        let all = segments(&e.dir);
        let (packs, plain): (Vec<_>, Vec<_>) = all.iter().partition(|s| s.2.header.pack);
        let plain: Vec<(u32, u32)> = plain.iter().map(|s| versions(&s.2)).collect();
        (versions(&packs[0].2), plain)
    };
    assert_eq!(pack_versions(false), ((1, 1), vec![(1, 1), (1, 1)]));
    assert_eq!(pack_versions(true), ((2, 2), vec![(1, 1), (1, 1), (2, 2)]));
}

#[test]
fn v2_segment_is_newer_for_a_v1_reader() {
    let record = encode_record(&linked_record("b1", 1, None, branch_link()));
    let (bytes, hash) = segment::encode(&header((2, 2)), &[&record]);
    assert_eq!(segment::decode(&bytes).unwrap().hash, hash);
    assert_eq!(
        segment::decode_with_reader(&bytes, 1),
        Err(SegError::Newer { min_reader: 2 })
    );
    // A v1 file stays readable by a v1 reader.
    let edit = encode_record(&linked_record("e1", 1, None, Link::Edit));
    let (v1, _) = segment::encode(&header((1, 1)), &[&edit]);
    assert!(segment::decode_with_reader(&v1, 1).is_ok());
    // Above our reader version is newer for us too.
    let (v3, _) = segment::encode(&header((3, 3)), &[&record]);
    assert_eq!(segment::decode(&v3), Err(SegError::Newer { min_reader: 3 }));
}

#[test]
fn snapshot_reader_version_is_its_own() {
    assert_eq!(segment::READER_VERSION, 2);
    assert_eq!(
        (
            snapfile::FORMAT_VERSION,
            snapfile::MIN_READER_VERSION,
            snapfile::READER_VERSION
        ),
        (1, 1, 1)
    );
    let project = fin_core::new_project(
        "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        "Demo".into(),
        fin_core::Rational::from_int(25),
    )
    .unwrap();
    let header = |v: u32| SnapHeader {
        format_version: v,
        min_reader_version: v,
        project_id: "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        branch: MAIN.into(),
        seq: 3,
        tx_hash: format!("blake3:{}", "ab".repeat(32)),
        model_schema_version: 1,
        writer_id: "01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
        epoch: 1,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
    };
    let (bytes, _) = snapfile::encode(&header(2), &project);
    assert_eq!(
        snapfile::decode(&bytes),
        Err(SegError::Newer { min_reader: 2 })
    );
    let (bytes, _) = snapfile::encode(&header(1), &project);
    assert!(snapfile::decode(&bytes).is_ok());
}

/// Opens the copied 4A fixture (`fixture_4a`) at `data`/`folder`, relocated, on `e`'s clock.
fn open_4a(e: &Env, data: &Path, folder: &Path) -> Store {
    let opts = OpenOptions {
        relocate: true,
        ..e.opts()
    };
    Store::open_with(data, folder, opts).unwrap()
}

#[test]
fn migration_0002_from_4a_fixture() {
    let e = env();
    let (data, folder) = fixture_4a(&e.root);
    let expected = expected_4a();
    let id = expected["project"]["id"].as_str().unwrap().to_string();
    let db = fin_store::db_path(&data, &id);
    let before = read_only(&db);
    let v: i64 = before
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v, 1, "the fixture is a 4A database");
    drop(before);

    let store = open_4a(&e, &data, &folder);
    let (rev, project) = store.snapshot(MAIN).unwrap();
    assert_eq!(rev, expected["rev"].as_u64().unwrap());
    assert_eq!(
        serde_json::to_value(&project).unwrap(),
        current(&expected["project"])
    );
    let conn = read_only(store.db_path());
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    // 0003, 0004 and 0005 (the version bumps) run after 0002.
    assert_eq!((v, DB_VERSION), (5, 5));
    assert_eq!(meta(&conn, "formatVersion").as_deref(), Some("5"));
    assert_eq!(meta(&conn, "minReaderVersion").as_deref(), Some("5"));
    assert_eq!(branch_rows(&conn), main_only());
    assert!(link_rows(&conn).is_empty());
    let names = backups(store.db_path());
    assert!(
        names.iter().any(|n| n.starts_with("pre-migrate-1-5-")),
        "{names:?}"
    );
    // The records 4A wrote are kept byte for byte.
    let kinds: Vec<String> = conn
        .prepare("SELECT kind FROM txs ORDER BY seq")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(kinds.iter().all(|k| k == "edit"), "{kinds:?}");
}

#[test]
fn sp2_fixture_migrates_through_0002() {
    const SP2_ID: &str = "01m6kvqb80p491nh8nfcg5bx1s";
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&fixture_root().join("sp2"), tmp.path());
    let data = tmp.path().join("server-data");
    let store = Store::open(&data, &tmp.path().join("folder")).unwrap();
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(tmp.path().join("expected_v2.json")).unwrap()).unwrap();
    let (rev, head) = store.snapshot(MAIN).unwrap();
    assert_eq!(rev, expected["rev"].as_u64().unwrap());
    assert_eq!(
        serde_json::to_value(&head).unwrap(),
        current(&expected["project"])
    );
    drop(store);
    let db = fin_store::db_path(&data, SP2_ID);
    let conn = read_only(&db);
    let v: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    // 0003, 0004 and 0005 (the version bumps) run after 0002.
    assert_eq!(v, 5);
    assert_eq!(meta(&conn, "formatVersion").as_deref(), Some("5"));
    assert_eq!(meta(&conn, "minReaderVersion").as_deref(), Some("5"));
    assert_eq!(branch_rows(&conn), main_only());
    assert!(link_rows(&conn).is_empty());
    let names = backups(&db);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("pre-migrate-0-5-"), "{names:?}");
}

#[test]
fn fresh_project_has_main_branch_row() {
    let e = env();
    let a = e.create("a");
    let conn = read_only(a.db_path());
    assert_eq!(branch_rows(&conn), main_only());
    assert_eq!(meta(&conn, "formatVersion").as_deref(), Some("5"));
    assert_eq!(meta(&conn, "minReaderVersion").as_deref(), Some("5"));
}

#[test]
fn v1_segment_written_before_upgrade_is_adopted() {
    let e = env();
    let (data, folder) = fixture_4a(&e.root);
    let expected = expected_4a();
    let db = fin_store::db_path(&data, expected["project"]["id"].as_str().unwrap());
    let files_before: Vec<PathBuf> = segments(&folder).into_iter().map(|s| s.0).collect();
    assert_eq!(files_before.len(), 1);
    // A 4A crash between the segment's rename and its marking: the rows are unexported and
    // the `segments` row (written in the same transaction as the marks) is missing.
    {
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("UPDATE txs SET exported = 0; DELETE FROM segments;")
            .unwrap();
    }
    let mut s = open_4a(&e, &data, &folder);
    e.flush(&mut s);
    e.flush(&mut s);
    let files_after: Vec<PathBuf> = segments(&folder).into_iter().map(|s| s.0).collect();
    assert_eq!(
        files_after, files_before,
        "the v1 file is adopted, not rewritten"
    );
    let unexported: i64 = read_only(s.db_path())
        .query_row("SELECT COUNT(*) FROM txs WHERE exported = 0", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(unexported, 0);
    assert_eq!(s.head(MAIN), expected["rev"].as_u64().unwrap());
}

#[test]
fn v1_segment_with_a_4b_kind_is_corrupt() {
    let record = encode_record(&linked_record("b1", 1, None, branch_link()));
    let (bytes, _) = segment::encode(&header((1, 1)), &[&record]);
    assert!(matches!(segment::decode(&bytes), Err(SegError::Corrupt(_))));
    assert!(matches!(
        segment::decode_with_reader(&bytes, 1),
        Err(SegError::Corrupt(_))
    ));
}

/// A v2 golden project as this Finisher reads it: upcast to the current model (the fixtures
/// stay as written; spec §5.12.2).
fn current(v: &serde_json::Value) -> serde_json::Value {
    serde_json::to_value(fin_core::upcast_snapshot(v.clone()).unwrap()).unwrap()
}
