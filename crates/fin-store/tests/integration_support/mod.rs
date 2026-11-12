//! Shared helpers for the store integration tests (plan Task 11): several servers (one
//! server-data dir each) on one project folder, all on one [`FakeClock`]. The file server's
//! clock follows the fake clock, so 45 s of lease staleness pass without waiting.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use fin_core::{CommittedTx, CoreError, Op, Rational, SCHEMA_VERSION, Tx, VideoInfo};
use fin_store::lease::{FakeClock, LeaseState, MonoClock};
use fin_store::names::SegmentName;
use fin_store::segment::{self, Segment};
use fin_store::{MAIN, OpenOptions, Store, TickReport};
use tempfile::TempDir;

pub mod diverge;

pub struct Env {
    _tmp: TempDir,
    pub root: PathBuf,
    pub dir: PathBuf,
    pub clock: Arc<FakeClock>,
}

pub fn env() -> Env {
    env_named("My Project")
}

pub fn env_named(name: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    Env {
        dir: root.join(name),
        root,
        clock: FakeClock::new(),
        _tmp: tmp,
    }
}

impl Env {
    pub fn data(&self, server: &str) -> PathBuf {
        self.root.join(format!("server-{server}"))
    }

    pub fn opts(&self) -> OpenOptions {
        OpenOptions {
            clock: Some(self.clock.clone()),
            poll: Some(Duration::from_millis(2)),
            ..OpenOptions::default()
        }
    }

    /// Server `server` creates the project (and holds the lease, epoch 1).
    pub fn create(&self, server: &str) -> Store {
        Store::create_with(
            &self.data(server),
            &self.dir,
            "Demo",
            Rational::from_int(30),
            None,
            self.opts(),
        )
        .unwrap()
    }

    pub fn try_open_at(&self, server: &str, dir: &Path) -> Result<Store, CoreError> {
        Store::open_with(&self.data(server), dir, self.opts()).map_err(CoreError::from)
    }

    pub fn open(&self, server: &str) -> Store {
        self.try_open_at(server, &self.dir).unwrap()
    }

    /// Advances the fake clock by `secs` in 1 s steps, giving heartbeat threads time to beat.
    pub fn advance(&self, secs: u64) {
        for _ in 0..secs {
            self.clock.advance(Duration::from_secs(1));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// One tick that polls (whatever the cadence), at the fake clock's now.
    pub fn poll(&self, s: &mut Store) -> TickReport {
        s.poll_now_for_test();
        s.tick_report(self.clock.now()).unwrap()
    }

    /// Exports what `s` has pending: a poll starts the 2 s delay, 3 s later a poll writes.
    /// Returns the segments written.
    pub fn flush(&self, s: &mut Store) -> u32 {
        let first = self.poll(s).export.segments_written;
        self.advance(3);
        first + self.poll(s).export.segments_written
    }
}

pub fn asset(path: &str) -> Op {
    fin_core::AssetAdd::media(
        "$a",
        path,
        Rational::from_int(600),
        Some(VideoInfo {
            width: 1920,
            height: 1080,
            rate: Rational::from_int(30),
        }),
        None,
        "fp",
    )
    .into()
}

pub fn insert(at: i64, asset: &str) -> Op {
    fin_core::ItemInsert {
        id: "$c".into(),
        track: "t1".into(),
        at: Rational::from_int(at),
        duration: Rational::from_int(1),
        mode: fin_core::InsertMode::Ripple,
        body: fin_core::ItemBody::Clip {
            asset: asset.into(),
            source_in: Rational::from_int(0),
            speed: fin_core::Rational::from_int(1),
        },
    }
    .into()
}

/// `item.delete` of `item`, ripple.
pub fn delete_item(item: &str) -> Op {
    Op::from(fin_core::ItemDelete {
        item: item.into(),
        mode: fin_core::DeleteMode::Ripple,
    })
}

/// `item.remove` of `id`, ripple (the inverse of an insert).
pub fn remove_item(id: &str) -> Op {
    Op::from(fin_core::ItemRemove {
        id: id.into(),
        mode: fin_core::DeleteMode::Ripple,
    })
}

/// `item.trim` of `item`'s `edge` by `delta`, ripple.
pub fn trim_item(item: &str, edge: fin_core::Edge, delta: Rational) -> Op {
    Op::from(fin_core::ItemTrim {
        item: item.into(),
        edge,
        delta,
        mode: fin_core::TrimMode::Ripple,
        alone: false,
    })
}

pub fn try_commit(s: &mut Store, id: &str, ops: Vec<Op>) -> Result<CommittedTx, CoreError> {
    s.commit(Tx {
        tx_id: id.into(),
        author: "test".into(),
        branch: MAIN.into(),
        base_rev: s.head(MAIN),
        schema_version: SCHEMA_VERSION,
        args_hash: None,
        via: None,
        ops,
    })
    .map_err(CoreError::from)
}

pub fn commit(s: &mut Store, id: &str, ops: Vec<Op>) -> CommittedTx {
    try_commit(s, id, ops).unwrap()
}

/// The `details.reason` of a refused commit.
pub fn refused(s: &mut Store, id: &str) -> String {
    let e = try_commit(s, id, vec![asset("/m/refused.mov")]).unwrap_err();
    assert_eq!(e.code, "PROJECT_READ_ONLY", "{e:?}");
    assert!(e.fix.is_some());
    e.details["reason"].as_str().unwrap().to_string()
}

pub fn held_epoch(s: &Store) -> Option<u64> {
    match s.lease_state() {
        LeaseState::Held { epoch } => Some(epoch),
        _ => None,
    }
}

pub fn seg_dir(dir: &Path) -> PathBuf {
    dir.join(".finisher").join("segments").join(MAIN)
}

/// The canonical segment files on `main`, by first seq.
pub fn segments(dir: &Path) -> Vec<(PathBuf, SegmentName, Segment)> {
    let mut out: Vec<_> = fs::read_dir(seg_dir(dir))
        .map(|rd| {
            rd.filter_map(|e| {
                let p = e.ok()?.path();
                let name = SegmentName::parse(p.file_name()?.to_str()?)?;
                let seg = segment::decode(&fs::read(&p).ok()?).ok()?;
                Some((p, name, seg))
            })
            .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|(_, n, _)| n.first_seq());
    out
}

/// The txIds of every record in the folder's segments, in seq order.
pub fn folder_tx_ids(dir: &Path) -> Vec<String> {
    segments(dir)
        .iter()
        .flat_map(|(_, _, s)| s.records.clone())
        .map(|r| {
            let v: serde_json::Value = serde_json::from_slice(&r).unwrap();
            v["txId"].as_str().unwrap().to_string()
        })
        .collect()
}

/// Copies `from` to `to` recursively.
pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            fs::copy(e.path(), target).unwrap();
        }
    }
}

/// Runs `f(k, &items[k])` for every item on up to 8 threads. For the arrival-order tests, whose
/// cases are independent (a folder copy and a server of their own each) and CPU-bound. Every
/// item runs; a panic fails the call once all have finished (its message prints as usual).
pub fn par_each<T: Sync>(items: &[T], f: impl Fn(usize, &T) + Sync) {
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8)
        .min(items.len());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(k) else { break };
                    f(k, item);
                }
            });
        }
    });
}

/// Edits `finisher.project.json` in `dir`.
pub fn edit_project_file(
    dir: &Path,
    f: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) {
    let path = dir.join("finisher.project.json");
    let mut v: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    f(v.as_object_mut().unwrap());
    fs::write(&path, serde_json::to_vec_pretty(&v).unwrap()).unwrap();
}

/// Copies `from` to `to` recursively with hard links, as `cp -al`, rsnapshot or
/// `rsync --link-dest` do: new directories, the same files.
pub fn link_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            link_dir(&e.path(), &target);
        } else {
            fs::hard_link(e.path(), target).unwrap();
        }
    }
}

/// The scrubbed 4A fixture (`tests/fixtures/4a`), laid out in `tmp`: returns
/// `(server_data, folder)` with the folder copied and `project.fdb` placed at
/// `<server_data>/projects/<id>/project.fdb`. The fixture's recorded folder is gone, so open it
/// with `OpenOptions { relocate: true, .. }`.
pub fn fixture_4a(tmp: &Path) -> (PathBuf, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/4a");
    let folder = tmp.join("folder");
    copy_dir(&src.join("folder"), &folder);
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(folder.join("finisher.project.json")).unwrap()).unwrap();
    let server_data = tmp.join("server-data");
    let db = fin_store::db_path(&server_data, v["id"].as_str().unwrap());
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::copy(src.join("project.fdb"), &db).unwrap();
    (server_data, folder)
}

/// The scrubbed 4B fixture (`tests/fixtures/4b`), laid out in `tmp`: returns
/// `(server_data, folder)` with the folder copied and `project.fdb` placed at
/// `<server_data>/projects/<id>/project.fdb`. The fixture's recorded folder is gone, so open it
/// with `OpenOptions { relocate: true, .. }`.
pub fn fixture_4b(tmp: &Path) -> (PathBuf, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/4b");
    let folder = tmp.join("folder");
    copy_dir(&src.join("folder"), &folder);
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(folder.join("finisher.project.json")).unwrap()).unwrap();
    let server_data = tmp.join("server-data");
    let db = fin_store::db_path(&server_data, v["id"].as_str().unwrap());
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::copy(src.join("project.fdb"), &db).unwrap();
    (server_data, folder)
}

/// The scrubbed 4C fixture (`tests/fixtures/4c`, plan 5E Task 2), laid out as
/// [`fixture_4b`]: open it with `OpenOptions { relocate: true, .. }`.
pub fn fixture_4c(tmp: &Path) -> (PathBuf, PathBuf) {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/4c");
    let folder = tmp.join("folder");
    copy_dir(&src.join("folder"), &folder);
    let v: serde_json::Value =
        serde_json::from_slice(&fs::read(folder.join("finisher.project.json")).unwrap()).unwrap();
    let server_data = tmp.join("server-data");
    let db = fin_store::db_path(&server_data, v["id"].as_str().unwrap());
    fs::create_dir_all(db.parent().unwrap()).unwrap();
    fs::copy(src.join("project.fdb"), &db).unwrap();
    (server_data, folder)
}
