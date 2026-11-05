//! DB versioning: `application_id`/`user_version`, refusals that leave the file untouched,
//! pre-migration backups and migration 0001 (the sub-project 2 layout → hashed records).

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::{Op, Rational, Record, RecordKind, SCHEMA_VERSION, Tx, resolve_ops};
use fin_store::{APPLICATION_ID, DB_VERSION, MAIN, Store, StoreError, db_path};
use rusqlite::{Connection, OpenFlags, params};
use tempfile::TempDir;

const FIXTURE_ID: &str = "01m6kvqb80p491nh8nfcg5bx1s";

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sp2")
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A private copy of the SP2 fixture (the committed `.fdb` is never opened in place).
struct Sp2 {
    tmp: TempDir,
}

impl Sp2 {
    fn new() -> Sp2 {
        let tmp = tempfile::tempdir().unwrap();
        copy_dir(&fixture_dir(), tmp.path());
        Sp2 { tmp }
    }
    fn data(&self) -> PathBuf {
        self.tmp.path().join("server-data")
    }
    fn folder(&self) -> PathBuf {
        self.tmp.path().join("folder")
    }
    fn db(&self) -> PathBuf {
        db_path(&self.data(), FIXTURE_ID)
    }
    fn backups(&self) -> PathBuf {
        self.db().parent().unwrap().join("backups")
    }
    fn open(&self) -> Result<Store, StoreError> {
        Store::open(&self.data(), &self.folder())
    }
    /// Puts the untouched SP2 database back, as if the project had never been migrated.
    fn reset_db(&self) {
        let db = self.db();
        for side in ["project.fdb-wal", "project.fdb-shm"] {
            let _ = fs::remove_file(db.with_file_name(side));
        }
        fs::copy(
            fixture_dir().join(format!("server-data/projects/{FIXTURE_ID}/project.fdb")),
            &db,
        )
        .unwrap();
    }
}

fn hash(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

fn read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn pragma(conn: &Connection, name: &str) -> i64 {
    conn.query_row(&format!("PRAGMA {name}"), [], |r| r.get(0))
        .unwrap()
}

fn columns(conn: &Connection, table: &str) -> Vec<String> {
    let mut stmt = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .unwrap();
    stmt.query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn schema(conn: &Connection) -> Vec<(String, String)> {
    let mut stmt = conn
        .prepare("SELECT name, COALESCE(sql, '') FROM sqlite_master ORDER BY name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn meta(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .ok()
}

/// Every pre-migration backup file name, sorted.
fn backup_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match fs::read_dir(dir) {
        Ok(rd) => rd
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("pre-migrate-"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    names
}

/// A fresh project (at `DB_VERSION`), closed, with its database path.
fn fresh() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("server-data");
    let dir = tmp.path().join("Проект 1");
    let store = Store::create(&data, &dir, "Demo", Rational::from_int(30), None).unwrap();
    let db = store.db_path().to_path_buf();
    drop(store);
    (tmp, data, dir, db)
}

fn code(r: Result<Store, StoreError>) -> String {
    r.map(|_| ()).unwrap_err().code().to_string()
}

#[test]
fn sp2_fixture_migrates_to_identical_state() {
    let sp2 = Sp2::new();
    let expected: serde_json::Value =
        serde_json::from_slice(&fs::read(sp2.tmp.path().join("expected.json")).unwrap()).unwrap();
    let store = sp2.open().unwrap();
    let (rev, head) = store.snapshot(MAIN).unwrap();
    assert_eq!(rev, expected["rev"].as_u64().unwrap());
    assert_eq!(serde_json::to_value(&head).unwrap(), expected["project"]);
    drop(store);

    let conn = read_only(&sp2.db());
    assert_eq!(pragma(&conn, "user_version"), i64::from(DB_VERSION));
    assert_eq!(pragma(&conn, "application_id"), i64::from(APPLICATION_ID));
    assert!(!columns(&conn, "txs").contains(&"json".to_string()));
    // Migration 0002 runs after 0001.
    assert_eq!(meta(&conn, "formatVersion").as_deref(), Some("2"));
    assert_eq!(meta(&conn, "minReaderVersion").as_deref(), Some("2"));
    assert!(
        meta(&conn, "lastOpenedBy")
            .unwrap()
            .starts_with("finisher/")
    );
    let mut stmt = conn
        .prepare(
            "SELECT seq, branch, tx_id, kind, record_bytes, tx_hash, parent_hash, epoch, exported,
                    schema_version, author, committed_at FROM txs ORDER BY seq",
        )
        .unwrap();
    let rows: Vec<_> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, i64>(7)?,
                r.get::<_, i64>(8)?,
                r.get::<_, i64>(9)?,
                r.get::<_, String>(10)?,
                r.get::<_, i64>(11)?,
            ))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(rows.len() as u64, rev);
    let mut parent: Option<String> = None;
    for (i, row) in rows.iter().enumerate() {
        let (
            seq,
            branch,
            tx_id,
            kind,
            bytes,
            tx_hash,
            parent_hash,
            epoch,
            exported,
            sv,
            author,
            at,
        ) = row;
        assert_eq!(*seq, i as i64 + 1);
        assert_eq!(branch, MAIN);
        assert_eq!(kind, "edit");
        assert_eq!(*tx_hash, hash(bytes), "seq {seq}");
        assert_eq!(*parent_hash, parent, "seq {seq}");
        assert_eq!((*epoch, *exported), (0, 0));
        let record: Record = serde_json::from_slice(bytes).unwrap();
        // Exactly the bytes a fresh commit would write: re-encoding is the identity.
        assert_eq!(fin_core::encode_record(&record), *bytes);
        assert_eq!(record.tx_id, *tx_id);
        assert_eq!(record.seq, *seq as u64);
        assert_eq!(record.kind, RecordKind::Edit);
        assert_eq!(record.schema_version, SCHEMA_VERSION);
        assert_eq!(i64::from(record.schema_version), *sv);
        assert_eq!(record.parent_hash, parent);
        assert_eq!(record.author, "fixture");
        assert_eq!(record.author, *author);
        assert_eq!(record.committed_at, *at);
        assert!(*at > 1_700_000_000_000, "committed_at is unix millis");
        assert_eq!(record.intent.tx_id, *tx_id);
        assert_eq!(
            record.resolved,
            resolve_ops(&record.intent.ops, &record.bindings).unwrap()
        );
        assert!(!record.inverse.is_empty());
        parent = Some(tx_hash.clone());
    }
    assert_eq!(rows[0].6, None);
}

#[test]
fn commit_after_migration_continues_the_chain() {
    let sp2 = Sp2::new();
    let mut store = sp2.open().unwrap();
    let (rev, _) = store.snapshot(MAIN).unwrap();
    let c = store
        .commit(Tx {
            tx_id: "01JD0000000000000000000001".into(),
            author: "test".into(),
            branch: MAIN.into(),
            base_rev: rev,
            schema_version: SCHEMA_VERSION,
            args_hash: None,
            ops: vec![Op::AssetAdd {
                id: "$a".into(),
                path: "media/b.mov".into(),
                duration: Rational::from_int(5),
                video: None,
                audio: None,
                fingerprint: "fp".into(),
            }],
        })
        .unwrap();
    assert_eq!(c.rev, rev + 1);
    drop(store);
    let conn = read_only(&sp2.db());
    let tail = |seq: u64| -> (Vec<u8>, String, Option<String>) {
        conn.query_row(
            "SELECT record_bytes, tx_hash, parent_hash FROM txs WHERE seq = ?1",
            params![seq as i64],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
    };
    let (_, prev_hash, _) = tail(rev);
    let (bytes, tx_hash, parent_hash) = tail(rev + 1);
    assert_eq!(parent_hash, Some(prev_hash.clone()));
    assert_eq!(tx_hash, hash(&bytes));
    let record: Record = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(record.parent_hash, Some(prev_hash));
    assert_eq!(record.seq, rev + 1);
    assert_eq!(record.author, "test");
    assert_eq!(record.bindings.get("$a").map(String::as_str), Some("a2"));
    assert_eq!(fin_core::encode_record(&record), bytes);
    // Reopening replays the whole history again.
    let store = sp2.open().unwrap();
    assert_eq!(store.head(MAIN), rev + 1);
}

#[test]
fn fresh_and_migrated_schemas_match() {
    let (_tmp, _data, _dir, fresh_db) = fresh();
    let sp2 = Sp2::new();
    drop(sp2.open().unwrap());
    let a = read_only(&fresh_db);
    let b = read_only(&sp2.db());
    assert_eq!(schema(&a), schema(&b));
    assert_eq!(pragma(&a, "application_id"), i64::from(APPLICATION_ID));
    assert_eq!(pragma(&a, "user_version"), i64::from(DB_VERSION));
    for table in ["segments", "lease_state", "quarantine"] {
        assert!(!columns(&a, table).is_empty(), "{table}");
    }
    assert_eq!(meta(&a, "formatVersion").as_deref(), Some("2"));
    assert_eq!(meta(&a, "minReaderVersion").as_deref(), Some("2"));
    let created_by = meta(&a, "createdBy").unwrap();
    assert_eq!(
        created_by,
        format!("finisher/{}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(meta(&a, "lastOpenedBy"), Some(created_by));
}

#[test]
fn migration_backs_up_first() {
    let sp2 = Sp2::new();
    assert!(backup_names(&sp2.backups()).is_empty());
    drop(sp2.open().unwrap());
    let names = backup_names(&sp2.backups());
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].starts_with("pre-migrate-0-2-"), "{names:?}");
    assert!(names[0].ends_with(".fdb"), "{names:?}");
    // The backup is the database as it was before the first step.
    let conn = read_only(&sp2.backups().join(&names[0]));
    assert_eq!(pragma(&conn, "user_version"), 0);
    assert!(columns(&conn, "txs").contains(&"json".to_string()));
    let n: i64 = conn
        .query_row("SELECT COUNT(*) FROM txs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 205);
    // Opening an up-to-date database takes no backup.
    drop(sp2.open().unwrap());
    assert_eq!(backup_names(&sp2.backups()).len(), 1);
}

#[test]
fn fresh_project_has_no_backups() {
    let (_tmp, data, dir, db) = fresh();
    drop(Store::open(&data, &dir).unwrap());
    assert!(backup_names(&db.parent().unwrap().join("backups")).is_empty());
}

fn sidecars(db: &Path) -> Vec<String> {
    ["project.fdb-wal", "project.fdb-shm", "project.fdb-journal"]
        .into_iter()
        .filter(|side| db.with_file_name(side).exists())
        .map(String::from)
        .collect()
}

/// A fresh project whose database is switched to `journal` mode and then changed by `sql`,
/// opened once per mode: the refusal must leave the file byte-identical and create no
/// `-wal`/`-shm`. In `DELETE` mode a read-write open would rewrite header bytes 18–19 (WAL).
fn refused(sql: &str) -> Vec<fin_core::CoreError> {
    ["DELETE", "WAL"]
        .into_iter()
        .map(|journal| {
            let (_tmp, data, dir, db) = fresh();
            let conn = Connection::open(&db).unwrap();
            let mode: String = conn
                .query_row(&format!("PRAGMA journal_mode = {journal}"), [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert!(mode.eq_ignore_ascii_case(journal));
            conn.execute_batch(sql).unwrap();
            drop(conn);
            assert_eq!(sidecars(&db), Vec::<String>::new(), "{journal}");
            let before = fs::read(&db).unwrap();
            let err = Store::open(&data, &dir).unwrap_err();
            assert!(fs::read(&db).unwrap() == before, "{journal}: file changed");
            assert_eq!(sidecars(&db), Vec::<String>::new(), "{journal}");
            assert!(backup_names(&db.parent().unwrap().join("backups")).is_empty());
            fin_core::CoreError::from(err)
        })
        .collect()
}

#[test]
fn newer_db_is_refused() {
    for e in refused("PRAGMA user_version = 99") {
        assert_eq!(e.code, "DB_TOO_NEW");
        assert_eq!(e.details["dbVersion"], 99);
        assert_eq!(e.details["supported"], DB_VERSION);
        assert!(e.fix.is_some());
    }
}

#[test]
fn min_reader_too_new_is_refused() {
    for e in refused("UPDATE meta SET value = '99' WHERE key = 'minReaderVersion'") {
        assert_eq!(e.code, "DB_TOO_NEW");
        assert_eq!(e.details["minReaderVersion"], 99);
    }
}

#[test]
fn foreign_application_id_is_refused() {
    // Another application's database that happens to have our tables: refused untouched.
    for e in refused(
        "PRAGMA application_id = 1234; UPDATE meta SET value = 'someone-else' WHERE key = 'projectId'",
    ) {
        assert_eq!(e.code, "NOT_A_PROJECT_DB");
    }
    // Our own database whose 4 application-id header bytes are damaged (its meta still says
    // it is this project's): rebuilt, the old file kept (plan Task 12 re-review).
    let (_tmp, data, dir, db) = fresh();
    Connection::open(&db)
        .unwrap()
        .execute_batch("PRAGMA application_id = 1234")
        .unwrap();
    let s = Store::open(&data, &dir).unwrap();
    assert_eq!(s.last_rebuild().unwrap().reason, "corrupt");
}

#[test]
fn foreign_sqlite_is_refused() {
    let (_tmp, data, dir, db) = fresh();
    for side in ["project.fdb-wal", "project.fdb-shm"] {
        let _ = fs::remove_file(db.with_file_name(side));
    }
    fs::remove_file(&db).unwrap();
    let conn = Connection::open(&db).unwrap();
    conn.execute_batch("CREATE TABLE notes (body TEXT); INSERT INTO notes VALUES ('hi');")
        .unwrap();
    drop(conn);
    let before = fs::read(&db).unwrap();
    let e = Store::open(&data, &dir).unwrap_err();
    assert_eq!(e.code(), "NOT_A_PROJECT_DB", "{e}");
    assert!(fs::read(&db).unwrap() == before, "file changed");
    assert_eq!(sidecars(&db), Vec::<String>::new());

    // Not SQLite at all, at our own database path: damage, so it is rebuilt from the folder
    // and the file kept as it was (plan Task 12 review).
    let junk = b"definitely not a database, just some bytes in a file";
    fs::write(&db, junk).unwrap();
    let s = Store::open(&data, &dir).unwrap();
    let r = s.last_rebuild().expect("rebuilt");
    assert_eq!(r.reason, "corrupt");
    assert_eq!(fs::read(r.corrupt_copy.as_ref().unwrap()).unwrap(), junk);
}

/// A database another connection holds open in WAL mode (its `-wal` has frames not yet in the
/// main file) is read through the WAL: a refusal written only to the WAL is still seen.
#[test]
fn refusal_sees_uncheckpointed_wal() {
    let (_tmp, data, dir, db) = fresh();
    let holder = Connection::open(&db).unwrap();
    holder
        .execute_batch("PRAGMA wal_autocheckpoint = 0; PRAGMA user_version = 99;")
        .unwrap();
    assert!(db.with_file_name("project.fdb-wal").exists());
    assert_eq!(code(Store::open(&data, &dir)), "DB_TOO_NEW");
    drop(holder);
}

#[test]
fn corrupt_record_bytes_are_invalid_format() {
    let sp2 = Sp2::new();
    drop(sp2.open().unwrap());
    Connection::open(sp2.db())
        .unwrap()
        .execute(
            "UPDATE txs SET record_bytes = X'7B6E6F74206A736F6E' WHERE seq = 204",
            [],
        )
        .unwrap();
    let e = fin_core::CoreError::from(sp2.open().unwrap_err());
    assert_eq!(e.code, "INVALID_FORMAT");
    assert_eq!(e.details["seq"], 204);
    assert_eq!(e.details["kind"], "record");
}

#[test]
fn failed_step_rolls_back() {
    let sp2 = Sp2::new();
    let before = schema(&read_only(&sp2.db()));
    fin_store::fail_next_migration_step();
    assert_eq!(code(sp2.open()), "TEST_FAULT");
    let conn = read_only(&sp2.db());
    assert_eq!(pragma(&conn, "user_version"), 0);
    assert_eq!(pragma(&conn, "application_id"), 0);
    assert_eq!(schema(&conn), before);
    assert!(columns(&conn, "txs").contains(&"json".to_string()));
    assert_eq!(meta(&conn, "formatVersion"), None);
    drop(conn);
    // Without the fault, the same database migrates.
    let store = sp2.open().unwrap();
    assert_eq!(store.head(MAIN), 205);
}

#[test]
fn pre_migration_backups_keep_three() {
    let sp2 = Sp2::new();
    let mut seen = Vec::new();
    for _ in 0..4 {
        sp2.reset_db();
        let before = backup_names(&sp2.backups());
        drop(sp2.open().unwrap());
        let after = backup_names(&sp2.backups());
        let new: Vec<_> = after
            .iter()
            .filter(|n| !before.contains(n))
            .cloned()
            .collect();
        assert_eq!(new.len(), 1, "{before:?} -> {after:?}");
        seen.push(new[0].clone());
    }
    let mut newest = seen[1..].to_vec();
    newest.sort();
    assert_eq!(backup_names(&sp2.backups()), newest);
}

/// Backups dated in the future (a clock that was ahead) never cause the one just taken to be
/// pruned; names that are not strict pre-migration backups are never touched.
#[test]
fn prune_never_deletes_the_new_backup() {
    let sp2 = Sp2::new();
    let dir = sp2.backups();
    fs::create_dir_all(&dir).unwrap();
    let future = [
        "pre-migrate-0-1-20990101T000000000Z.fdb",
        "pre-migrate-0-1-20990102T000000000Z.fdb",
        "pre-migrate-0-1-20990103T000000000Z.fdb",
    ];
    let foreign = [
        "pre-migrate-0-1-junk.fdb",
        "pre-migrate-0-1-19990101T000000000Z.fdb.bak",
        "pre-migrate-x-1-19990101T000000000Z.fdb",
        "pre-migrate-0-1-1999010T1000000000Z.fdb",
        "notes.txt",
    ];
    for name in future.iter().chain(&foreign) {
        fs::write(dir.join(name), b"").unwrap();
    }
    drop(sp2.open().unwrap());
    let left = backup_names(&dir);
    let new: Vec<_> = left
        .iter()
        .filter(|n| !future.contains(&n.as_str()) && !foreign.contains(&n.as_str()))
        .collect();
    assert_eq!(new.len(), 1, "{left:?}");
    // The new one plus the two newest others.
    assert!(!dir.join(future[0]).exists(), "{left:?}");
    assert!(dir.join(future[1]).exists() && dir.join(future[2]).exists());
    for name in foreign {
        assert!(dir.join(name).exists(), "{name}");
    }
}

/// Once the new backup is durable, failing to prune an old one does not fail the migration.
#[test]
fn prune_failure_does_not_abort_migration() {
    let sp2 = Sp2::new();
    let dir = sp2.backups();
    // A directory with a backup's name cannot be removed as a file.
    fs::create_dir_all(dir.join("pre-migrate-0-1-20000101T000000000Z.fdb")).unwrap();
    for name in [
        "pre-migrate-0-1-20000102T000000000Z.fdb",
        "pre-migrate-0-1-20000103T000000000Z.fdb",
    ] {
        fs::write(dir.join(name), b"").unwrap();
    }
    let store = sp2.open().unwrap();
    assert_eq!(store.head(MAIN), 205);
    assert!(dir.join("pre-migrate-0-1-20000101T000000000Z.fdb").is_dir());
}

/// Temp files a crashed backup left behind are removed at the next migration once they are
/// older than ten minutes; younger ones may belong to a backup in progress.
#[test]
fn stale_backup_temps_are_cleaned() {
    let sp2 = Sp2::new();
    let dir = sp2.backups();
    fs::create_dir_all(&dir).unwrap();
    let old = dir.join(".~fin-01JC0000000000000000000000.tmp");
    let young = dir.join(".~fin-01JC0000000000000000000001.tmp");
    fs::write(&old, b"partial").unwrap();
    fs::write(&young, b"partial").unwrap();
    let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(hour_ago)
        .unwrap();
    drop(sp2.open().unwrap());
    assert!(!old.exists());
    assert!(young.exists());
}

/// Migration 0001 is frozen at schema v1: its output for the fixture is pinned. The final
/// `tx_hash` covers every record's bytes through the `parentHash` chain.
#[test]
fn migration_0001_output_is_pinned() {
    let sp2 = Sp2::new();
    drop(sp2.open().unwrap());
    let conn = read_only(&sp2.db());
    let last: String = conn
        .query_row(
            "SELECT tx_hash FROM txs ORDER BY seq DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(last, GOLDEN_LAST_TX_HASH);
}

const GOLDEN_LAST_TX_HASH: &str =
    "blake3:4e6755bc3ae846cec660a6bef924520123073bca9042aa95d3f35f39409c2139";

/// Today the frozen migration and a 4A commit agree byte for byte: each SP2 row, rebuilt
/// through the current `fin-core` types and `encode_record`, gives the migrated bytes.
#[test]
fn migration_0001_matches_encode_record() {
    let sp2 = Sp2::new();
    let old = read_only(&sp2.db());
    let mut stmt = old
        .prepare("SELECT rev, json, inverse, bindings, committed_at FROM txs ORDER BY rev")
        .unwrap();
    let expected: Vec<Vec<u8>> = {
        let mut parent = None;
        stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
            ))
        })
        .unwrap()
        .map(|row| {
            let (rev, json, inverse, bindings, committed_at) = row.unwrap();
            let intent: Tx = serde_json::from_str(&json).unwrap();
            let bindings: std::collections::BTreeMap<String, String> =
                serde_json::from_str(&bindings).unwrap();
            let resolved = resolve_ops(&intent.ops, &bindings).unwrap();
            let record = Record::new_edit(
                intent.tx_id.clone(),
                rev as u64,
                intent.schema_version,
                parent.clone(),
                intent.author.clone(),
                committed_at,
                intent,
                resolved,
                serde_json::from_str(&inverse).unwrap(),
                bindings,
            );
            let bytes = fin_core::encode_record(&record);
            parent = Some(hash(&bytes));
            bytes
        })
        .collect()
    };
    drop(stmt);
    drop(old);
    drop(sp2.open().unwrap());
    let conn = read_only(&sp2.db());
    let mut stmt = conn
        .prepare("SELECT record_bytes FROM txs ORDER BY seq")
        .unwrap();
    let got: Vec<Vec<u8>> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(got.len(), expected.len());
    for (i, (g, e)) in got.iter().zip(&expected).enumerate() {
        assert_eq!(
            String::from_utf8_lossy(g),
            String::from_utf8_lossy(e),
            "seq {}",
            i + 1
        );
    }
}

/// The read-only check builds a `file:` URI: spaces, non-ASCII, `#` and `%` in the server
/// data path still open the right file, and nothing is created beside it.
#[test]
fn check_handles_awkward_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("Sërver data #1 %41 Мой");
    let dir = tmp.path().join("Проект 1");
    let db = Store::create(&data, &dir, "Demo", Rational::from_int(30), None)
        .unwrap()
        .db_path()
        .to_path_buf();
    Connection::open(&db)
        .unwrap()
        .execute_batch("PRAGMA user_version = 99")
        .unwrap();
    assert_eq!(sidecars(&db), Vec::<String>::new());
    assert_eq!(code(Store::open(&data, &dir)), "DB_TOO_NEW");
    assert_eq!(sidecars(&db), Vec::<String>::new());
}
