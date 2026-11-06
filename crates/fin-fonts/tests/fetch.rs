//! The fetcher (4C Decisions 32 and 33), against the local test server with fault
//! injection. No test touches the internet.

mod support;

use std::fs;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fin_fonts::{FetchOptions, Fetcher, FileState, FontCache, FontCatalog};
use serde_json::{Value, json};
use support::{
    COMMIT, File as CatFile, Route, Server, catalog, catalog_json, font, names, opts, wait_for,
    zip_of,
};

const LONG: Duration = Duration::from_secs(30);

fn setup(files: &[CatFile]) -> (tempfile::TempDir, Arc<FontCache>, Arc<FontCatalog>) {
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    (dir, cache, catalog(files))
}

fn failure(err: &fin_core::CoreError) -> &str {
    assert_eq!(err.code, "FONT_FETCH_FAILED", "{}", err.message);
    err.details["reason"].as_str().unwrap()
}

#[test]
fn fetch_verifies_hash() {
    let srv = Server::start();
    let bytes = font(1, 8192);
    let mut wrong = bytes.clone();
    wrong[10] ^= 1;
    let (bad, good) = (format!("/{COMMIT}/bad.ttf"), format!("/{COMMIT}/good.ttf"));
    srv.route(&bad, Route::new(wrong));
    srv.route(&good, Route::new(bytes.clone()));

    // hash_mismatch_never_lands: a source serving other bytes fails at once, leaving nothing.
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&bad)]);
    let sha = file.sha();
    let (dir, cache, cat) = setup(&[file]);
    let f = Fetcher::new(cache.clone(), cat, opts());
    let err = f.fetch_now(&sha).unwrap_err();
    assert_eq!(failure(&err), "hashMismatch");
    assert_eq!(
        srv.hits(&bad).len(),
        1,
        "a wrong body is not retried at that source"
    );
    assert_eq!(names(dir.path()), [format!("{sha}.lock")]);
    assert!(cache.get(&sha).is_none());

    // The next source serves the right bytes.
    let file = CatFile::plain(
        "a",
        "regular",
        bytes.clone(),
        vec![srv.url(&bad), srv.url(&good)],
    );
    let (_dir, cache, cat) = setup(&[file]);
    Fetcher::new(cache.clone(), cat, opts())
        .fetch_now(&sha)
        .unwrap();
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
}

#[test]
fn resume_after_partial() {
    let srv = Server::start();
    let bytes = font(2, 200 * 1024);
    let half = 70_000;
    let cases = [("ranged", true), ("unranged", false)];
    for (name, range) in cases {
        let path = format!("/{COMMIT}/{name}.ttf");
        srv.route(
            &path,
            Route {
                range,
                truncate_first: 1,
                truncate_at: half,
                ..Route::new(bytes.clone())
            },
        );
        let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
        let sha = file.sha();
        let (_dir, cache, cat) = setup(&[file]);
        Fetcher::new(cache.clone(), cat, opts())
            .fetch_now(&sha)
            .unwrap();
        let hits = srv.hits(&path);
        assert_eq!(hits.len(), 2, "{name}");
        assert_eq!(hits[0].range, None);
        assert_eq!(hits[1].range.as_deref(), Some("bytes=70000-"), "{name}");
        assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..], "{name}");
    }

    // A download another process left behind continues from its part.json…
    let path = format!("/{COMMIT}/left.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (dir, cache, cat) = setup(&[file]);
    fs::write(dir.path().join(format!("{sha}.part")), &bytes[..50_000]).unwrap();
    let info = json!({ "url": srv.url(&path), "received": 40_000 });
    fs::write(
        dir.path().join(format!("{sha}.part.json")),
        info.to_string(),
    )
    .unwrap();
    let f = Fetcher::new(cache.clone(), cat, opts());
    f.fetch_now(&sha).unwrap();
    assert_eq!(srv.hits(&path)[0].range.as_deref(), Some("bytes=40000-"));
    assert_eq!(names(dir.path()), [sha.clone(), format!("{sha}.lock")]);

    // …unless it was from another URL; and a range over another representation restarts.
    let path = format!("/{COMMIT}/cdn.ttf");
    srv.route(
        &path,
        Route {
            range_total: Some(1234),
            ..Route::new(bytes.clone())
        },
    );
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let (dir, cache, cat) = setup(&[file]);
    fs::write(dir.path().join(format!("{sha}.part")), &bytes[..50_000]).unwrap();
    let info = json!({ "url": srv.url(&path), "received": 50_000 });
    fs::write(
        dir.path().join(format!("{sha}.part.json")),
        info.to_string(),
    )
    .unwrap();
    Fetcher::new(cache.clone(), cat, opts())
        .fetch_now(&sha)
        .unwrap();
    let hits = srv.hits(&path);
    assert_eq!(hits.len(), 2);
    assert_eq!(
        (hits[0].range.as_deref(), hits[1].range.as_deref()),
        (Some("bytes=50000-"), None)
    );
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
}

#[test]
fn size_limit_aborts() {
    let srv = Server::start();
    let bytes = font(3, 8192);
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0; 100]);
    for close_delimited in [false, true] {
        let path = format!("/{COMMIT}/long-{close_delimited}.ttf");
        srv.route(
            &path,
            Route {
                close_delimited,
                ..Route::new(longer.clone())
            },
        );
        let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
        let sha = file.sha();
        let (dir, cache, cat) = setup(&[file]);
        let err = Fetcher::new(cache, cat, opts())
            .fetch_now(&sha)
            .unwrap_err();
        assert_eq!(failure(&err), "tooLarge", "{}", err.message);
        assert_eq!(srv.hits(&path).len(), 1);
        assert_eq!(names(dir.path()), [format!("{sha}.lock")]);
    }
}

#[test]
fn retries_with_backoff_then_next_source() {
    let srv = Server::start();
    let bytes = font(4, 4096);
    let (flaky, gone, good) = (
        format!("/{COMMIT}/flaky.ttf"),
        format!("/{COMMIT}/gone.ttf"),
        format!("/{COMMIT}/good.ttf"),
    );
    srv.route(
        &flaky,
        Route {
            fail_first: 100,
            status: 503,
            ..Route::new(bytes.clone())
        },
    );
    srv.route(&good, Route::new(bytes.clone()));
    let urls = vec![srv.url(&flaky), srv.url(&gone), srv.url(&good)];
    let file = CatFile::plain("a", "regular", bytes.clone(), urls);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let backoff = [20, 40, 80].map(Duration::from_millis).to_vec();
    let o = FetchOptions { backoff, ..opts() };
    let t0 = Instant::now();
    Fetcher::new(cache.clone(), cat, o).fetch_now(&sha).unwrap();
    assert!(
        t0.elapsed() >= Duration::from_millis(140),
        "{:?}",
        t0.elapsed()
    );
    assert_eq!(srv.hits(&flaky).len(), 3, "3 attempts at a failing source");
    assert_eq!(srv.hits(&gone).len(), 1, "a 404 is not retried");
    assert_eq!(srv.hits(&good).len(), 1);
    assert!(cache.has(&sha));
}

#[test]
fn fetch_failed_state_and_retry_at() {
    let srv = Server::start();
    let bytes = font(5, 4096);
    let path = format!("/{COMMIT}/later.ttf");
    srv.route(
        &path,
        Route {
            fail_first: 3,
            ..Route::new(bytes.clone())
        },
    );
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let o = FetchOptions {
        retry_after: Duration::from_millis(500),
        ..opts()
    };
    let f = Fetcher::new(cache.clone(), cat.clone(), o);
    let rx = f.subscribe();
    let before = fin_now_ms();
    f.request(&sha);
    let failed = wait_for(&rx, &sha, LONG, |s| matches!(s, FileState::Failed { .. }));
    let FileState::Failed {
        reason,
        retry_at_ms,
    } = failed
    else {
        unreachable!()
    };
    assert_eq!(reason, "http500");
    assert!(retry_at_ms >= before + 500 && retry_at_ms <= fin_now_ms() + 500);
    assert_eq!(srv.hits(&path).len(), 3);
    // Backing off: a request does nothing.
    f.request(&sha);
    assert!(matches!(f.state(&sha), FileState::Failed { .. }));
    // Then it retries by itself and succeeds.
    wait_for(&rx, &sha, LONG, |s| *s == FileState::Cached);
    assert_eq!(srv.hits(&path).len(), 4);

    // fetch_now reports the failure as FONT_FETCH_FAILED with retryAt.
    let dead = format!("/{COMMIT}/dead.ttf");
    srv.route(
        &dead,
        Route {
            fail_first: usize::MAX,
            ..Route::new(bytes.clone())
        },
    );
    let other = font(6, 4096);
    let file = CatFile::plain("b", "regular", other, vec![srv.url(&dead)]);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let err = Fetcher::new(cache, cat, opts())
        .fetch_now(&sha)
        .unwrap_err();
    assert_eq!(failure(&err), "http500");
    assert_eq!(err.details["key"], "Test b|400|normal");
    assert_eq!(err.details["sha256"], sha.as_str());
    assert!(err.details["retryAt"].as_i64().unwrap() > fin_now_ms());
    assert!(err.fix.as_deref().unwrap().ends_with("fin fonts fetch b"));
    assert!(err.fix.as_deref().unwrap().contains("HTTPS_PROXY"));
}

fn fin_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The child process of `concurrent_fetch_one_download` (a no-op unless it is that child).
#[test]
fn concurrent_child() {
    let Ok(spec) = std::env::var("FIN_FONTS_CHILD") else {
        return;
    };
    let v: Value = serde_json::from_str(&spec).unwrap();
    let cache = Arc::new(FontCache::open(v["dir"].as_str().unwrap().as_ref()).unwrap());
    let cat = Arc::new(FontCatalog::from_json(&v["catalog"].to_string()).unwrap());
    let f = Fetcher::new(cache, cat, opts());
    f.fetch_now(v["sha"].as_str().unwrap()).unwrap();
    assert_eq!(f.state(v["sha"].as_str().unwrap()), FileState::Cached);
}

#[test]
fn concurrent_fetch_one_download() {
    let srv = Server::start();
    let bytes = font(7, 300 * 1024);
    let path = format!("/{COMMIT}/shared.ttf");
    srv.route(
        &path,
        Route {
            chunk_delay: Duration::from_millis(20),
            ..Route::new(bytes.clone())
        },
    );
    let files = [CatFile::plain(
        "a",
        "regular",
        bytes.clone(),
        vec![srv.url(&path)],
    )];
    let sha = files[0].sha();
    let dir = tempfile::tempdir().unwrap();
    let spec = json!({ "dir": dir.path(), "catalog": catalog_json(&files), "sha": sha });
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["concurrent_child", "--exact", "--test-threads", "1"])
        .env("FIN_FONTS_CHILD", spec.to_string())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (dir, cat, sha) = (dir.path().to_path_buf(), catalog(&files), sha.clone());
            std::thread::spawn(move || {
                let cache = Arc::new(FontCache::open(&dir).unwrap());
                let f = Fetcher::new(cache, cat, opts());
                f.fetch_now(&sha).unwrap();
                f.state(&sha)
            })
        })
        .collect();
    for t in threads {
        assert_eq!(t.join().unwrap(), FileState::Cached);
    }
    assert!(child.wait().unwrap().success());
    assert_eq!(srv.hits(&path).len(), 1, "one download for all three");
    assert_eq!(names(dir.path()), [sha.clone(), format!("{sha}.lock")]);
}

#[test]
fn cache_reuse_no_network() {
    let srv = Server::start();
    let bytes = font(8, 4096);
    let path = format!("/{COMMIT}/reuse.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (dir, cache, cat) = setup(&[file]);
    Fetcher::new(cache, cat.clone(), opts())
        .fetch_now(&sha)
        .unwrap();
    let conns = srv.connections();

    // A new process on the same server data: hits only.
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let f = Fetcher::new(cache.clone(), cat, opts());
    f.request(&sha);
    f.fetch_now(&sha).unwrap();
    assert_eq!(f.state(&sha), FileState::Cached);
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(srv.connections(), conns);
    assert_eq!(srv.hits(&path).len(), 1);
}

#[test]
fn offline_never_connects() {
    let srv = Server::start();
    let bytes = font(9, 4096);
    let path = format!("/{COMMIT}/offline.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let o = FetchOptions {
        offline: true,
        ..opts()
    };
    let f = Fetcher::new(cache.clone(), cat, o);
    f.request(&sha);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(srv.connections(), 0);
    assert_eq!(f.state(&sha), FileState::Missing);
    assert!(cache.get(&sha).is_none());

    // The explicit command still fetches (owner answer 1); cache hits then work offline.
    f.fetch_now(&sha).unwrap();
    assert_eq!(srv.hits(&path).len(), 1);
    f.request(&sha);
    assert_eq!(f.state(&sha), FileState::Cached);
    assert_eq!(srv.hits(&path).len(), 1);
}

#[test]
fn archive_member_extracted_and_verified() {
    let srv = Server::start();
    let (regular, bold) = (font(10, 6000), font(11, 7000));
    let zip = zip_of(&[
        ("LICENSE.txt", b"OFL"),
        ("ttf/A-Regular.ttf", &regular),
        ("ttf/A-Bold.ttf", &bold),
    ]);
    let path = "/releases/download/v1.0/a.zip".to_string();
    srv.route(&path, Route::new(zip.clone()));
    let member = |style, bytes: &[u8], name: &str| CatFile {
        archive: Some((srv.url(&path), zip.clone(), name.to_string())),
        ..CatFile::plain("a", style, bytes.to_vec(), vec![])
    };
    let files = [
        member("regular", &regular, "ttf/A-Regular.ttf"),
        member("bold", &bold, "ttf/A-Bold.ttf"),
    ];
    let (rs, bs) = (files[0].sha(), files[1].sha());
    let (dir, cache, cat) = setup(&files);
    let f = Fetcher::new(cache.clone(), cat, opts());
    f.fetch_now(&rs).unwrap();
    assert_eq!(&*cache.get(&rs).unwrap(), &regular[..]);
    // Its sibling came out of the same download; the archive is gone.
    assert_eq!(&*cache.get(&bs).unwrap(), &bold[..]);
    f.fetch_now(&bs).unwrap();
    assert_eq!(srv.hits(&path).len(), 1);
    let left: Vec<String> = names(dir.path())
        .into_iter()
        .filter(|n| !n.ends_with(".lock"))
        .collect();
    let mut want = vec![rs.clone(), bs.clone()];
    want.sort();
    assert_eq!(left, want);

    // A member whose bytes are not the catalog's never lands; nor does one of another size.
    let wrong = |bytes: Vec<u8>, name: &str| {
        let file = CatFile {
            archive: Some((srv.url(&path), zip.clone(), name.to_string())),
            ..CatFile::plain("b", "regular", bytes, vec![])
        };
        let sha = file.sha();
        let (dir, cache, cat) = setup(&[file]);
        let err = Fetcher::new(cache, cat, opts())
            .fetch_now(&sha)
            .unwrap_err();
        assert!(!dir.path().join(&sha).exists());
        assert!(names(dir.path()).iter().all(|n| n.ends_with(".lock")));
        failure(&err).to_string()
    };
    let mut same_size = regular.clone();
    same_size[0] ^= 1;
    assert_eq!(wrong(same_size, "ttf/A-Regular.ttf"), "hashMismatch");
    assert_eq!(wrong(font(12, 9000), "ttf/A-Regular.ttf"), "sizeMismatch");
    assert_eq!(wrong(regular.clone(), "ttf/Missing.ttf"), "archive");

    // An archive that is not the catalog's fails its own hash.
    let mut bad_zip = zip.clone();
    let n = bad_zip.len();
    bad_zip[n / 2] ^= 1;
    let bad_path = "/releases/download/v1.0/bad.zip".to_string();
    srv.route(&bad_path, Route::new(bad_zip));
    let file = CatFile {
        archive: Some((srv.url(&bad_path), zip.clone(), "ttf/A-Regular.ttf".into())),
        ..CatFile::plain("c", "regular", regular.clone(), vec![])
    };
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let err = Fetcher::new(cache, cat, opts())
        .fetch_now(&sha)
        .unwrap_err();
    assert_eq!(failure(&err), "hashMismatch");
}

#[test]
fn concurrent_siblings_download_the_archive_once() {
    let srv = Server::start();
    let (regular, bold) = (font(20, 6000), font(21, 7000));
    let zip = zip_of(&[("A-Regular.ttf", &regular), ("A-Bold.ttf", &bold)]);
    let path = "/releases/download/v2.0/siblings.zip".to_string();
    srv.route(
        &path,
        Route {
            chunk_delay: Duration::from_millis(40),
            ..Route::new(zip.clone())
        },
    );
    let member = |style, bytes: &[u8], name: &str| CatFile {
        archive: Some((srv.url(&path), zip.clone(), name.to_string())),
        ..CatFile::plain("a", style, bytes.to_vec(), vec![])
    };
    let files = [
        member("regular", &regular, "A-Regular.ttf"),
        member("bold", &bold, "A-Bold.ttf"),
    ];
    let (rs, bs) = (files[0].sha(), files[1].sha());
    let dir = tempfile::tempdir().unwrap();
    let spec = json!({ "dir": dir.path(), "catalog": catalog_json(&files), "sha": bs });
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["concurrent_child", "--exact", "--test-threads", "1"])
        .env("FIN_FONTS_CHILD", spec.to_string())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let threads: Vec<_> = [rs.clone(), bs.clone()]
        .into_iter()
        .map(|sha| {
            let (dir, cat) = (dir.path().to_path_buf(), catalog(&files));
            std::thread::spawn(move || {
                let cache = Arc::new(FontCache::open(&dir).unwrap());
                Fetcher::new(cache, cat, opts()).fetch_now(&sha).unwrap();
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert!(child.wait().unwrap().success());
    assert_eq!(
        srv.hits(&path).len(),
        1,
        "one archive download for both members"
    );
    let cache = FontCache::open(dir.path()).unwrap();
    assert_eq!(&*cache.get(&rs).unwrap(), &regular[..]);
    assert_eq!(&*cache.get(&bs).unwrap(), &bold[..]);
}

#[test]
fn waiting_for_a_lock_holds_no_download_slot() {
    let srv = Server::start();
    let (a, b) = (font(22, 4096), font(23, 4096));
    let (pa, pb) = (format!("/{COMMIT}/held.ttf"), format!("/{COMMIT}/free.ttf"));
    srv.route(&pa, Route::new(a.clone()));
    srv.route(&pb, Route::new(b.clone()));
    let files = [
        CatFile::plain("a", "regular", a.clone(), vec![srv.url(&pa)]),
        CatFile::plain("b", "regular", b.clone(), vec![srv.url(&pb)]),
    ];
    let (sa, sb) = (files[0].sha(), files[1].sha());
    let (dir, cache, cat) = setup(&files);
    // Another process is fetching `a` and keeps its lock.
    let held = fs::File::create(dir.path().join(format!("{sa}.lock"))).unwrap();
    held.lock().unwrap();
    // More waiters on `a` than the process has download slots.
    let waiters: Vec<_> = (0..fin_fonts::MAX_DOWNLOADS + 1)
        .map(|_| {
            let (cache, cat, sa) = (cache.clone(), cat.clone(), sa.clone());
            std::thread::spawn(move || Fetcher::new(cache, cat, opts()).fetch_now(&sa))
        })
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    let t0 = Instant::now();
    let f = Fetcher::new(cache.clone(), cat.clone(), opts());
    f.fetch_now(&sb).unwrap();
    assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
    assert!(matches!(
        f.state(&sa),
        FileState::Missing | FileState::Queued
    ));
    drop(held);
    for w in waiters {
        w.join().unwrap().unwrap();
    }
    assert_eq!(srv.hits(&pa).len(), 1);
    assert!(cache.has(&sa) && cache.has(&sb));
}

#[test]
fn offline_can_change_at_run_time() {
    let srv = Server::start();
    let bytes = font(24, 4096);
    let path = format!("/{COMMIT}/toggle.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let f = Fetcher::new(cache, cat, opts());
    assert!(!f.offline());
    f.set_offline(true);
    f.request(&sha);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(srv.connections(), 0);
    f.set_offline(false);
    let rx = f.subscribe();
    f.request(&sha);
    wait_for(&rx, &sha, LONG, |s| *s == FileState::Cached);
    assert_eq!(srv.hits(&path).len(), 1);
}

#[test]
fn https_required_outside_test_hooks() {
    let srv = Server::start();
    let bytes = font(13, 4096);
    let path = format!("/{COMMIT}/plain.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let (_dir, cache, cat) = setup(&[file]);
    let o = FetchOptions {
        allow_loopback_http: false,
        ..opts()
    };
    let err = Fetcher::new(cache, cat, o).fetch_now(&sha).unwrap_err();
    assert_eq!(failure(&err), "insecureUrl");
    assert_eq!(srv.connections(), 0);

    // Plain http elsewhere is refused by the catalog even in test-hooks builds.
    let mut doc = catalog_json(&[CatFile::plain(
        "a",
        "regular",
        bytes,
        vec![format!("http://example.com/{COMMIT}/a.ttf")],
    )]);
    let err = FontCatalog::from_json(&doc.to_string()).unwrap_err();
    assert_eq!(err.details["rule"], "https");
    doc["entries"][0]["styles"][0]["sources"][0]["url"] = json!(srv.url("/main/a.ttf"));
    let err = FontCatalog::from_json(&doc.to_string()).unwrap_err();
    assert_eq!(err.details["rule"], "immutableUrl");
}

#[test]
fn unknown_file_is_not_in_catalog() {
    let (_dir, cache, cat) = setup(&[]);
    let f = Fetcher::new(cache, cat, opts());
    let sha = "0".repeat(64);
    assert_eq!(f.fetch_now(&sha).unwrap_err().code, "FONT_NOT_IN_CATALOG");
    f.request(&sha);
    assert_eq!(f.state(&sha), FileState::Missing);
}
