//! The dataset builder's inputs (sp7 §12.1, §12.7; plan 7A Task 10). The builder is not built
//! in sub-project 7; this stub fixes what it may read: the entries of the diagnostics journal
//! (`<server-data>/events/`) and of the agent-trace store (`<server-data>/agent-traces/`),
//! and their blobs. **Nothing here reaches `control.db`** (conversations, sessions, runs,
//! nodes): an import-graph rule (`scripts/check-crate-deps.mjs`) holds this crate to
//! `fin-core`, and a test runs the readers against a server-data directory through a spy
//! [`Files`] that records every path opened.
//!
//! Reading goes through [`Files`], so every path the readers touch is observable. [`dedup`]
//! applies the duplicate rule of at-least-once delivery; [`replay`] rebuilds a run's exact
//! request bodies from the trace store's deltas, checked against each turn's `bodyBlake3`.

mod dedup;
mod replay;

pub use dedup::dedup;
pub use replay::{ReplayTurn, hash_of, replay};

use std::io;
use std::path::{Path, PathBuf};

use fin_core::events::Envelope;

/// One of the two stores the builder reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// `<server-data>/events/`: metadata (and, under training, content of other kinds).
    Diagnostics,
    /// `<server-data>/agent-traces/`: agent turns, calls and results with their content.
    AgentTraces,
}

impl Store {
    pub fn dir(self, server_data: &Path) -> PathBuf {
        server_data.join(match self {
            Store::Diagnostics => "events",
            Store::AgentTraces => "agent-traces",
        })
    }
}

/// The file access the readers use: the real file system ([`Disk`]) or a test's spy.
pub trait Files {
    /// The entries of `dir` (an absent directory is empty).
    fn list(&self, dir: &Path) -> io::Result<Vec<PathBuf>>;
    fn read(&self, file: &Path) -> io::Result<Vec<u8>>;
}

/// [`Files`] on the real file system.
#[derive(Debug, Default, Clone, Copy)]
pub struct Disk;

impl Files for Disk {
    fn list(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        match std::fs::read_dir(dir) {
            Ok(rd) => rd.map(|e| e.map(|e| e.path())).collect(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    fn read(&self, file: &Path) -> io::Result<Vec<u8>> {
        std::fs::read(file)
    }
}

/// Every entry of `store`, oldest first (its `j-*.jsonl` files in write order). A line that
/// does not parse (a torn tail) is skipped.
/// Journal files (`j-<yyyymmdd>-<ULID>.jsonl`) in the order they were written: by their ULID,
/// which the journal keeps monotonic, not by name, whose date goes back with the clock (review
/// #22, as in fin-journal's `files::list`).
fn write_order(files: &mut [PathBuf]) {
    let id = |p: &PathBuf| {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        name.strip_suffix(".jsonl")
            .and_then(|r| r.rsplit_once('-'))
            .map(|(_, id)| id.to_ascii_uppercase())
            .unwrap_or_default()
    };
    files.sort_by(|a, b| id(a).cmp(&id(b)).then_with(|| a.cmp(b)));
}

pub fn entries(fs: &dyn Files, server_data: &Path, store: Store) -> io::Result<Vec<Envelope>> {
    let mut files: Vec<PathBuf> = fs
        .list(&store.dir(server_data))?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("j-") && n.ends_with(".jsonl"))
        })
        .collect();
    write_order(&mut files);
    let mut out = Vec::new();
    for f in files {
        let text = fs.read(&f)?;
        out.extend(
            text.split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .filter_map(|l| serde_json::from_slice::<Envelope>(l).ok()),
        );
    }
    Ok(out)
}

/// The bytes of the blob `hex` of `store` (`blobs/<hex>`), if it is there.
pub fn blob(
    fs: &dyn Files,
    server_data: &Path,
    store: Store,
    hex: &str,
) -> io::Result<Option<Vec<u8>>> {
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    match fs.read(&store.dir(server_data).join("blobs").join(hex)) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod order_tests {
    use super::*;

    #[test]
    fn files_are_read_in_write_order_not_name_order() {
        // Written second, after the clock went back a day: an earlier date, a later ULID.
        let mut files = vec![
            PathBuf::from("j-20261109-01JC0000000000000000000002.jsonl"),
            PathBuf::from("j-20261110-01JC0000000000000000000001.jsonl"),
        ];
        write_order(&mut files);
        assert_eq!(
            files[0],
            PathBuf::from("j-20261110-01JC0000000000000000000001.jsonl")
        );
    }
}
