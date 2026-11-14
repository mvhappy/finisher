#![cfg(feature = "fetch")]
//! Proxies come from the environment (4C Decision 33). Its own test binary: it sets
//! `HTTPS_PROXY` for the whole process before any fetcher exists.

mod support;

use std::sync::Arc;

use fin_fonts::{FetchOptions, Fetcher, FontCache};
use support::{COMMIT, File as CatFile, Route, Server, catalog, font, opts};

#[test]
fn proxy_env_is_used() {
    // The target is a port nothing listens on: only the proxy can answer.
    let unused = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let target_port = unused.local_addr().unwrap().port();
    drop(unused);
    let proxy = Server::start();
    let bytes = font(1, 4096);
    let path = format!("/{COMMIT}/proxied.ttf");
    proxy.route(&path, Route::new(bytes.clone()));
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
    unsafe { std::env::set_var("HTTPS_PROXY", format!("http://127.0.0.1:{}", proxy.port())) };

    let url = format!("http://127.0.0.1:{target_port}{path}");
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![url]);
    let sha = file.sha();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let o = FetchOptions {
        proxy_from_env: true,
        ..opts()
    };
    Fetcher::new(cache.clone(), catalog(&[file]), o)
        .fetch_now(&sha)
        .unwrap();
    assert_eq!(proxy.connects(), [format!("127.0.0.1:{target_port}")]);
    assert_eq!(proxy.hits(&path).len(), 1);
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
}
