//! Helpers of the walker tests (plan 5D Task 4).
#![allow(dead_code)]

use std::sync::Arc;

use fin_library::{
    CancelToken, DirDone, Excludes, KnownDir, KnownDirs, RawRel, RootState, ScanBatch, ScanEntry,
    Source, Timing, WalkError, WalkMode, WalkOpts, WalkSummary, walk,
};
use serde_json::{Value, json};

/// A raw chain from a `/`-joined UTF-8 path (`""` is the root).
pub fn rel(p: &str) -> RawRel {
    if p.is_empty() {
        return RawRel::root();
    }
    RawRel::new(p.split('/').map(|c| c.as_bytes().to_vec()).collect()).unwrap()
}

pub fn opts(threads: usize) -> WalkOpts {
    let mut o = WalkOpts::new(Excludes::defaults(true));
    o.threads = threads;
    o
}

/// Scaled deadlines: 60 s → `first_stat_ms`, 30 s → `entry_ms`.
pub fn timing(first_stat_ms: u64, entry_ms: u64) -> Timing {
    Timing {
        first_stat: std::time::Duration::from_millis(first_stat_ms),
        entry: std::time::Duration::from_millis(entry_ms),
        flush: std::time::Duration::from_secs(5),
    }
}

/// Walks and collects every batch.
pub fn run(
    src: Arc<dyn Source>,
    mode: WalkMode,
    opts: &WalkOpts,
) -> (Result<WalkSummary, WalkError>, Vec<ScanBatch>) {
    let mut batches = Vec::new();
    let r = walk(
        src,
        mode,
        opts,
        &mut |b| {
            batches.push(b);
            Ok(())
        },
        &CancelToken::new(),
    );
    (r, batches)
}

pub fn full(generation: u64, known: Option<KnownDirs>) -> WalkMode {
    WalkMode::Full { generation, known }
}

pub fn dones(batches: &[ScanBatch]) -> Vec<DirDone> {
    batches.iter().flat_map(|b| b.dirs_done.clone()).collect()
}

pub fn entries(batches: &[ScanBatch]) -> Vec<ScanEntry> {
    batches.iter().flat_map(|b| b.entries.clone()).collect()
}

/// The display paths of the closed listings, in order, `!` marking incomplete ones with
/// their code (`"a/b!TIMEOUT"`).
pub fn done_paths(batches: &[ScanBatch]) -> Vec<String> {
    dones(batches)
        .iter()
        .map(|d| match (&d.code, d.complete) {
            (Some(c), false) => format!("{}!{c}", d.dir.display()),
            _ => d.dir.display(),
        })
        .collect()
}

/// The catalog's known directories after `batches` of generation `generation`, as the
/// indexer would write them.
pub fn known_from(batches: &[ScanBatch], generation: u64) -> KnownDirs {
    let all = entries(batches);
    KnownDirs::new(dones(batches).into_iter().filter(|d| d.complete).map(|d| {
        let mount_root = match (d.dir.parent(), d.dir.name()) {
            (Some(p), Some(n)) => all
                .iter()
                .any(|e| e.dir == p && e.name == n && e.mount_root),
            _ => false,
        };
        KnownDir {
            dir: d.dir,
            mtime_ns: d.mtime_ns,
            entries: d.entries,
            listed_gen: Some(generation),
            mount_root,
        }
    }))
}

/// A file node of the fake spec.
pub fn file(name: &str, mtime: i64) -> Value {
    json!({ "name": name, "size": 10, "mtimeNs": mtime })
}

/// A directory node of the fake spec.
pub fn dir(name: &str, mtime: i64, children: Vec<Value>) -> Value {
    json!({ "name": name, "type": "dir", "mtimeNs": mtime, "children": children })
}

pub fn root(children: Vec<Value>) -> Value {
    json!({ "volume": { "key": "k1", "rootFileId": "01" }, "mtimeNs": 1, "children": children })
}

/// Three levels: `a/{x,y}`, `b/z`, `c`, each with two files.
pub fn tree() -> Value {
    let files = |m| vec![file("1.mov", m), file("2.mov", m)];
    root(vec![
        dir(
            "a",
            10,
            [
                files(1),
                vec![dir("x", 11, files(2)), dir("y", 12, files(3))],
            ]
            .concat(),
        ),
        dir("b", 20, [files(4), vec![dir("z", 21, files(5))]].concat()),
        dir("c", 30, files(6)),
        file("top.mov", 7),
    ])
}

/// The root state of a walk that ended offline.
pub fn offline(r: &Result<WalkSummary, WalkError>) -> Option<RootState> {
    r.as_ref().err().and_then(|e| e.offline().cloned())
}
