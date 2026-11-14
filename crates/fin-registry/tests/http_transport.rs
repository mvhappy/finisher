//! The test-only HTTP transport (8A Task 5): the same repository served over loopback HTTP
//! refreshes to the same trusted set as from the folder, and caps hold.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use fin_registry::transport::{HttpTransport, Transport, TransportError};
use fin_registry::tuf::Policy;
use support::tuf::Reg;

/// Serves files under `root` over HTTP/1.1 on loopback until the test ends; returns the URL.
fn serve(root: PathBuf) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut line = String::new();
            let mut reader = BufReader::new(conn.try_clone().unwrap());
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                    break;
                }
            }
            let path = line
                .split(' ')
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/');
            let file = root.join(path);
            let (status, body) = match std::fs::read(&file) {
                Ok(b) if !path.contains("..") => ("200 OK", b),
                _ => ("404 Not Found", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = conn.write_all(head.as_bytes());
            let _ = conn.write_all(&body);
        }
    });
    url
}

#[test]
fn http_transport_matches_fs_transport() {
    let mut r = Reg::new(None);
    let acme = r.publisher("acme");
    let info = r.b.publish(&r.pkg("@acme/pack", "1.0.0"), &acme).unwrap();
    r.b.rotate(
        fin_registry::editor::Role::Root,
        &[fin_registry::editor::SigningKey::generate()],
        1,
    )
    .unwrap();
    let url = serve(r.repo.clone());
    let http = Arc::new(HttpTransport::at(&url));
    let mut over_fs = r.client();
    r.refresh(&mut over_fs).unwrap();
    let mut over_http = r.client_in(&r.tmp.path().join("http-trust"));
    r.refresh_with(
        &mut over_http,
        http.clone(),
        Policy::default(),
        r.clock.as_ref(),
    )
    .unwrap();
    assert_eq!(
        over_http.trusted_files().unwrap(),
        over_fs.trusted_files().unwrap()
    );
    assert_eq!(over_http.versions(), over_fs.versions());
    let a = over_fs
        .fetch_target_bytes(
            &fin_registry::transport::FsTransport::new(&r.repo),
            &info.name,
        )
        .unwrap();
    let b = over_http
        .fetch_target_bytes(http.as_ref(), &info.name)
        .unwrap();
    assert_eq!(a, b);
}

#[test]
fn http_respects_length_caps() {
    let r = Reg::new(None);
    let url = serve(r.repo.clone());
    let http = HttpTransport::at(&url);
    let ts = std::fs::read(r.meta_path("timestamp.json")).unwrap();
    let n = ts.len() as u64;
    assert_eq!(http.fetch("metadata/timestamp.json", n).unwrap(), ts);
    assert!(matches!(
        http.fetch("metadata/timestamp.json", n - 1),
        Err(TransportError::TooLarge { .. })
    ));
    assert!(matches!(
        http.fetch("metadata/2.root.json", 1 << 20),
        Err(TransportError::NotFound { .. })
    ));
    assert!(matches!(
        http.fetch("../etc/passwd", 10),
        Err(TransportError::BadPath { .. })
    ));
    // A refresh over HTTP refuses an oversized timestamp like the folder transport does.
    let mut b = ts.clone();
    b.extend(std::iter::repeat_n(b' ', 16 * 1024));
    std::fs::write(r.meta_path("timestamp.json"), b).unwrap();
    let mut c = r.client();
    let e = r
        .refresh_with(&mut c, Arc::new(http), Policy::default(), r.clock.as_ref())
        .unwrap_err();
    assert_eq!((e.code, e.reason()), ("REGISTRY_INVALID", Some("tooLarge")));
}

/// Mirrors `scripts/check-test-hooks.mjs`: the default members' normal build never enables
/// `fin-registry/test-http`.
#[test]
fn feature_off_in_default_members() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let out = std::process::Command::new(env!("CARGO"))
        .args([
            "tree",
            "-e",
            "features,normal",
            "--prefix",
            "none",
            // Not `--offline`: a fresh host has only the crates it built so far, and the tree
            // of the default members needs every one's manifest.
            "--locked",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8_lossy(&out.stdout);
    assert!(
        tree.contains("fin-registry"),
        "fin-registry is not a default member"
    );
    assert!(
        !tree.contains("feature \"test-http\""),
        "a default member enables test-http"
    );
}

/// Answers like Python's `http.server` (the `tuf-conformance` simulator): an `HTTP/1.0`
/// response with no `Connection` header, after which the server closes the socket. The close
/// lands `late` after the response (a slow runner), and a second request on the same socket
/// is never read. Returns the URL and the number of connections accepted.
fn serve_http10(late: std::time::Duration) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = accepted.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(conn.try_clone().unwrap());
                loop {
                    let mut h = String::new();
                    if reader.read_line(&mut h).unwrap_or(0) == 0 || h == "\r\n" {
                        break;
                    }
                }
                let body = b"{}";
                let head = format!("HTTP/1.0 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                let _ = conn.write_all(head.as_bytes());
                let _ = conn.write_all(body);
                std::thread::sleep(late);
            });
        }
    });
    (url, accepted)
}

#[test]
fn an_http10_response_is_never_reused_even_when_the_close_is_late() {
    // An HTTP/1.0 response without `Connection: keep-alive` ends its connection (RFC 9112
    // §9.3), but `ureq` 3.5 pools it unless the close has already arrived; the next fetch then
    // fails `REGISTRY_UNREACHABLE: … Peer disconnected` (tuf-conformance on CI, 2026-11-14).
    let (url, accepted) = serve_http10(std::time::Duration::from_millis(500));
    let t = HttpTransport::at(&url);
    for path in [
        "metadata/timestamp.json",
        "metadata/1.snapshot.json",
        "metadata/2.root.json",
    ] {
        let got = t.fetch(path, 1024);
        assert!(got.is_ok(), "{path}: {got:?}");
    }
    assert_eq!(
        accepted.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "one connection per fetch"
    );
}
