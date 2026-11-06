//! Shared by the fin-fonts tests: the local HTTP server, catalogs over it, fetch options
//! without delays, generated fonts and zips.
#![allow(dead_code)]

pub mod http_server;

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use fin_fonts::{FetchOptions, FileState, FontCatalog, sha256_hex};
use serde_json::{Value, json};

pub use http_server::{Route, Server};

/// A commit-shaped path segment, so test URLs pass the immutable-URL rule.
pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

/// One catalog file for a test catalog.
pub struct File {
    pub entry: &'static str,
    pub style: &'static str,
    pub weight: u16,
    pub bytes: Vec<u8>,
    /// Plain sources, in order.
    pub urls: Vec<String>,
    /// Or an archive source: (url, archive bytes, member).
    pub archive: Option<(String, Vec<u8>, String)>,
}

impl File {
    pub fn plain(
        entry: &'static str,
        style: &'static str,
        bytes: Vec<u8>,
        urls: Vec<String>,
    ) -> File {
        File {
            entry,
            style,
            weight: if style == "bold" { 700 } else { 400 },
            bytes,
            urls,
            archive: None,
        }
    }

    pub fn sha(&self) -> String {
        sha256_hex(&self.bytes)
    }
}

/// The JSON of a catalog of `files` (one entry per distinct entry id).
pub fn catalog_json(files: &[File]) -> Value {
    let mut entries: Vec<Value> = Vec::new();
    for f in files {
        let sources: Vec<Value> = match &f.archive {
            Some((url, zip, member)) => vec![json!({
                "url": url, "sha256": sha256_hex(zip), "size": zip.len(),
                "archive": { "format": "zip", "member": member },
            })],
            None => f.urls.iter().map(|u| json!({ "url": u })).collect(),
        };
        let family = format!("Test {}", f.entry);
        let style = json!({
            "id": f.style, "weight": f.weight, "italic": false,
            "file": { "sha256": f.sha(), "size": f.bytes.len(), "format": "ttf",
                "faces": [{ "index": 0, "family": family, "weight": f.weight, "italic": false, "scripts": ["Latn"] }] },
            "sources": sources,
        });
        match entries.iter_mut().find(|e| e["id"] == f.entry) {
            Some(e) => e["styles"].as_array_mut().unwrap().push(style),
            None => entries.push(json!({
                "id": f.entry, "family": family,
                "licence": { "spdx": "OFL-1.1", "url": format!("https://example.com/{COMMIT}/OFL.txt") },
                "homepage": "https://example.com/", "scripts": ["Latn"], "styles": [style],
            })),
        }
    }
    json!({ "catalogVersion": "2026.11.1", "fallback": [], "entries": entries })
}

pub fn catalog(files: &[File]) -> Arc<FontCatalog> {
    Arc::new(FontCatalog::from_json(&catalog_json(files).to_string()).unwrap())
}

/// Loopback http, no proxy, no backoff waits, a long retry and lock wait.
pub fn opts() -> FetchOptions {
    FetchOptions {
        offline: false,
        allow_loopback_http: true,
        proxy_from_env: false,
        backoff: vec![Duration::ZERO; 3],
        retry_after: Duration::from_secs(600),
        lock_wait: Duration::from_secs(60),
    }
}

/// A generated font of `size` bytes, distinct per `seed`.
pub fn font(seed: u8, size: usize) -> Vec<u8> {
    let mut b = fin_testfonts::test_big(size);
    let n = b.len();
    b[n - 1] ^= seed;
    b
}

pub fn zip_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opt = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    for (name, bytes) in members {
        w.start_file(*name, opt).unwrap();
        w.write_all(bytes).unwrap();
    }
    w.finish().unwrap().into_inner()
}

/// Waits for `want` on `rx` for `sha`; panics after `timeout`.
pub fn wait_for(
    rx: &Receiver<(String, FileState)>,
    sha: &str,
    timeout: Duration,
    want: impl Fn(&FileState) -> bool,
) -> FileState {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok((s, st)) if s == sha && want(&st) => return st,
            Ok(_) => {}
            Err(e) => panic!("no wanted state for {sha}: {e}"),
        }
    }
}

/// The directory's file names, sorted.
pub fn names(dir: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    v.sort();
    v
}
