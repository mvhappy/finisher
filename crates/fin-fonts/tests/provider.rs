#![cfg(feature = "fetch")]
//! Two `rustls` providers in one process (sub-project 8 spec §6.6, 8A Task 1). `tough` brings
//! `rustls` with `aws_lc_rs`; fin-fonts uses `ring`. With both compiled in and no process
//! default, `rustls` panics wherever a `ClientConfig` is built without naming its provider.
//!
//! This binary compiles both in (the dev-dependency on `rustls` with `aws_lc_rs`), checks that
//! an unnamed builder really would panic here, then drives fin-fonts' one client config
//! (`http::Http::new`, built by every `Fetcher`) into a TLS handshake. No network: the
//! request goes through `HTTPS_PROXY` to a loopback listener that answers the `CONNECT` and
//! reads the ClientHello. Its own test binary: it sets the proxy for the whole process, and
//! nothing else in it may install a process default.

#[allow(unused_imports)]
mod support;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use fin_fonts::{FetchOptions, Fetcher, FontCache};
use rustls::crypto::CryptoProvider;
use support::{COMMIT, File as CatFile, catalog, font, opts};

#[test]
fn no_process_default_provider() {
    // Precondition: both providers are compiled in, so an unnamed builder panics.
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let unnamed = std::panic::catch_unwind(|| {
        let _ = rustls::ClientConfig::builder();
    });
    std::panic::set_hook(hook);
    assert!(
        unnamed.is_err(),
        "this binary must compile in both rustls providers, or the test proves nothing"
    );
    assert!(CryptoProvider::get_default().is_none());

    // A loopback proxy: answers the first `CONNECT`, then records the tunnel's first byte.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            let Ok(s) = conn else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let mut r = BufReader::new(s);
            let mut request = String::new();
            let _ = r.read_line(&mut request);
            let mut line = String::new();
            while r.read_line(&mut line).is_ok_and(|n| n > 2) {
                line.clear();
            }
            let mut s = r.into_inner();
            let _ = s.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n");
            let mut first = [0u8; 1];
            let byte = s.read_exact(&mut first).ok().map(|()| first[0]);
            if tx.send((request.trim_end().to_string(), byte)).is_err() {
                return;
            }
        }
    });
    for var in [
        "ALL_PROXY",
        "all_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "https_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        // SAFETY: the only test in this binary, before any other thread reads the environment.
        unsafe { std::env::remove_var(var) };
    }
    // SAFETY: as above.
    unsafe { std::env::set_var("HTTPS_PROXY", format!("http://127.0.0.1:{port}")) };

    let bytes = font(1, 1024);
    let url = format!("https://raw.githubusercontent.com/example/fonts/{COMMIT}/provider.ttf");
    let file = CatFile::plain("a", "regular", bytes, vec![url]);
    let sha = file.sha();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let o = FetchOptions {
        proxy_from_env: true,
        backoff: vec![Duration::ZERO],
        ..opts()
    };
    let err = Fetcher::new(cache, catalog(&[file]), o)
        .fetch_now(&sha)
        .expect_err("the proxy hangs up after the ClientHello, so the fetch fails");
    assert_eq!(err.code, "FONT_FETCH_FAILED", "{err:?}");

    // The client built its `ring` config and started a handshake: a TLS record (0x16).
    let (request, first) = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(request, "CONNECT raw.githubusercontent.com:443 HTTP/1.1");
    assert_eq!(first, Some(0x16), "expected a TLS ClientHello");
    assert!(CryptoProvider::get_default().is_none());
}
