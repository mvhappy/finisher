//! A loopback HTTP server for font files and test catalogs over it (no internet), the Rust
//! twin of `packages/fin-node/test/fontserver.ts`, and the finisherd environment that makes a
//! test-hooks server fetch from it (plan 4C Task 24).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// A commit-shaped path segment, so test URLs pass the catalog's immutable-URL rule.
pub const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

pub fn sha256(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    d.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Clone)]
enum Route {
    Bytes(Arc<Vec<u8>>),
    Status(u16),
}

#[derive(Default)]
struct State {
    routes: HashMap<String, Route>,
    hits: HashMap<String, usize>,
}

/// Serves registered paths over plain HTTP on 127.0.0.1 until dropped.
pub struct FontServer {
    pub port: u16,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
}

fn respond(mut stream: TcpStream, state: &Mutex<State>) {
    let mut head = Vec::new();
    let mut buf = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&head);
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    let route = {
        let mut s = state.lock().unwrap();
        *s.hits.entry(path.clone()).or_default() += 1;
        s.routes.get(&path).cloned()
    };
    let _ = match route {
        Some(Route::Bytes(bytes)) => stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ncontent-type: font/ttf\r\nconnection: close\r\n\r\n",
                    bytes.len()
                )
                .as_bytes(),
            )
            .and_then(|()| stream.write_all(&bytes)),
        Some(Route::Status(code)) => stream.write_all(
            format!("HTTP/1.1 {code} Error\r\ncontent-length: 0\r\nconnection: close\r\n\r\n")
                .as_bytes(),
        ),
        None => stream.write_all(
            b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
        ),
    };
    let _ = stream.flush();
}

impl FontServer {
    pub fn start() -> FontServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, st) = (state.clone(), stop.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                if st.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(stream) = conn {
                    let s = s.clone();
                    std::thread::spawn(move || respond(stream, &s));
                }
            }
        });
        FontServer { port, state, stop }
    }

    pub fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/{COMMIT}/{name}", self.port)
    }

    /// Serves `bytes` at `name`; returns its URL.
    pub fn serve(&self, name: &str, bytes: &[u8]) -> String {
        let path = format!("/{COMMIT}/{name}");
        self.state
            .lock()
            .unwrap()
            .routes
            .insert(path, Route::Bytes(Arc::new(bytes.to_vec())));
        self.url(name)
    }

    /// Answers `status` at `name`; returns its URL.
    pub fn fail(&self, name: &str, status: u16) -> String {
        let path = format!("/{COMMIT}/{name}");
        self.state
            .lock()
            .unwrap()
            .routes
            .insert(path, Route::Status(status));
        self.url(name)
    }

    pub fn hits(&self, name: &str) -> usize {
        let path = format!("/{COMMIT}/{name}");
        self.state
            .lock()
            .unwrap()
            .hits
            .get(&path)
            .copied()
            .unwrap_or(0)
    }
}

impl Drop for FontServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// One catalog file: entry `id`, style `regular` (weight `weight`), family `family`, served
/// at `url`.
pub struct CatalogFile<'a> {
    pub id: &'a str,
    pub family: &'a str,
    pub weight: u16,
    pub bytes: &'a [u8],
    pub url: String,
}

/// A test catalog (OFL-1.1, Latin) of one entry per file; `fallback` lists entry ids.
pub fn catalog_json(version: &str, files: &[CatalogFile<'_>], fallback: &[&str]) -> Value {
    let entries: Vec<Value> = files
        .iter()
        .map(|f| {
            json!({
                "id": f.id, "family": f.family,
                "licence": { "spdx": "OFL-1.1", "url": format!("https://example.com/{COMMIT}/OFL.txt") },
                "homepage": "https://example.com/",
                "scripts": ["Latn"],
                "styles": [{
                    "id": "regular", "weight": f.weight, "italic": false,
                    "file": { "sha256": sha256(f.bytes), "size": f.bytes.len(), "format": "ttf",
                              "faces": [{ "index": 0, "family": f.family, "weight": f.weight, "italic": false, "scripts": ["Latn"] }] },
                    "sources": [{ "url": f.url }],
                }],
            })
        })
        .collect();
    json!({ "catalogVersion": version, "fallback": fallback, "entries": entries })
}

/// The test-hooks fin-node build `pnpm --filter @finisher/fin-node build:test` makes.
pub fn test_native_dir() -> PathBuf {
    super::repo_root().join("packages/fin-node/test/native")
}

/// Why the font tests cannot run (no test-hooks fin-node binary), if they cannot.
pub fn why_no_test_hooks() -> Option<String> {
    let has = std::fs::read_dir(test_native_dir())
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().ends_with(".node"))
        })
        .unwrap_or(false);
    (!has).then(|| {
        "no test-hooks fin-node binary (run `pnpm --filter @finisher/fin-node build:test`)".into()
    })
}

/// finisherd's environment for a test catalog at `catalog`: the test-hooks binary, no fetch
/// backoff, no proxies.
pub fn env(catalog: &Path) -> Vec<(String, OsString)> {
    vec![
        ("FIN_NODE_BINARY_DIR".into(), test_native_dir().into()),
        ("FIN_FONT_CATALOG".into(), catalog.into()),
        ("FIN_FONTS_TEST_BACKOFF_MS".into(), "0".into()),
        ("FIN_FONTS_TEST_RETRY_MS".into(), "60000".into()),
        ("FIN_FONTS_TEST_NO_PROXY".into(), "1".into()),
    ]
}

/// A server on a test catalog, and the catalog file (rewrite it, then restart the server, to
/// change catalogs); `None` when it cannot run (see `Server::start_with`).
pub struct Fonts {
    pub server: super::Server,
    pub catalog: PathBuf,
    _dir: tempfile::TempDir,
}

pub fn start(test: &str, catalog: &Value) -> Option<Fonts> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(&path, catalog.to_string()).unwrap();
    let server = super::Server::start_with(test, env(&path), why_no_test_hooks())?;
    Some(Fonts {
        server,
        catalog: path,
        _dir: dir,
    })
}
