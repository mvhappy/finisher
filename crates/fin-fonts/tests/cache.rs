//! The content-addressed font cache (4C Decision 32), against the local test server.

mod support;

use std::fs::{self, File, FileTimes};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fin_fonts::{Fetcher, FileState, FontCache, STRAY_AFTER};
use support::{COMMIT, File as CatFile, Route, Server, catalog, font, names, opts, wait_for};

const LONG: Duration = Duration::from_secs(30);

fn set_mtime(path: &std::path::Path, t: SystemTime) {
    let f = File::options().write(true).open(path).unwrap();
    f.set_times(FileTimes::new().set_modified(t)).unwrap();
}

fn slow(bytes: &[u8]) -> Route {
    Route {
        chunk_delay: Duration::from_millis(30),
        ..Route::new(bytes.to_vec())
    }
}

#[test]
fn write_is_temp_then_rename() {
    let srv = Server::start();
    let bytes = font(1, 160 * 1024);
    let path = format!("/{COMMIT}/a.ttf");
    srv.route(&path, slow(&bytes));
    let file = CatFile::plain("a", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let f = Fetcher::new(cache.clone(), catalog(&[file]), opts());
    let rx = f.subscribe();
    f.request(&sha);
    wait_for(
        &rx,
        &sha,
        LONG,
        |s| matches!(s, FileState::Fetching { bytes, total } if *bytes > 0 && bytes < total),
    );
    assert!(
        !dir.path().join(&sha).exists(),
        "never visible before it verifies"
    );
    assert!(dir.path().join(format!("{sha}.part")).exists());
    assert!(dir.path().join(format!("{sha}.part.json")).exists());
    wait_for(&rx, &sha, LONG, |s| *s == FileState::Cached);
    assert_eq!(names(dir.path()), [sha.clone(), format!("{sha}.lock")]);
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
}

#[test]
fn existing_file_is_verified_and_kept() {
    let srv = Server::start();
    let bytes = font(2, 160 * 1024);
    let path = format!("/{COMMIT}/b.ttf");
    srv.route(&path, slow(&bytes));
    let file = CatFile::plain("b", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let f = Fetcher::new(cache.clone(), catalog(&[file]), opts());
    let rx = f.subscribe();
    let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000_000);

    // A good file lands (from elsewhere) while the download runs: it is kept, not replaced.
    f.request(&sha);
    wait_for(
        &rx,
        &sha,
        LONG,
        |s| matches!(s, FileState::Fetching { bytes, .. } if *bytes > 0),
    );
    fs::write(dir.path().join(&sha), &bytes).unwrap();
    set_mtime(&dir.path().join(&sha), old);
    wait_for(&rx, &sha, LONG, |s| *s == FileState::Cached);
    let meta = fs::metadata(dir.path().join(&sha)).unwrap();
    assert_eq!(meta.modified().unwrap(), old, "the existing file was kept");
    assert!(!dir.path().join(format!("{sha}.part")).exists());

    // Cached and verified: no network.
    let hits = srv.hits(&path).len();
    f.fetch_now(&sha).unwrap();
    assert_eq!(srv.hits(&path).len(), hits);

    // A bad file in its place is replaced by a fresh fetch.
    fs::write(dir.path().join(&sha), vec![0u8; bytes.len()]).unwrap();
    set_mtime(&dir.path().join(&sha), old + Duration::from_secs(5));
    f.fetch_now(&sha).unwrap();
    assert_eq!(srv.hits(&path).len(), hits + 1);
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
}

#[test]
fn tampered_cache_file_is_rejected() {
    let srv = Server::start();
    let bytes = font(3, 4096);
    let path = format!("/{COMMIT}/c.ttf");
    srv.route(&path, Route::new(bytes.clone()));
    let file = CatFile::plain("c", "regular", bytes.clone(), vec![srv.url(&path)]);
    let sha = file.sha();
    let dir = tempfile::tempdir().unwrap();
    let cache = Arc::new(FontCache::open(dir.path()).unwrap());
    let f = Fetcher::new(cache.clone(), catalog(&[file]), opts());
    f.fetch_now(&sha).unwrap();
    assert!(cache.get(&sha).is_some());
    assert_eq!(f.state(&sha), FileState::Cached);

    let target = dir.path().join(&sha);
    let mut tampered = bytes.clone();
    tampered[100] ^= 0xff;
    fs::write(&target, &tampered).unwrap();
    set_mtime(&target, SystemTime::now() + Duration::from_secs(10));
    assert!(cache.get(&sha).is_none());
    let provider: &dyn fin_text::FontBytes = &*cache;
    assert!(provider.get(&sha).is_none());
    assert!(!cache.has(&sha));
    assert!(
        matches!(f.state(&sha), FileState::Failed { ref reason, .. } if reason == "hashMismatch")
    );

    // A request fetches it again, replacing the bad file.
    let rx = f.subscribe();
    f.request(&sha);
    wait_for(&rx, &sha, LONG, |s| *s == FileState::Cached);
    assert_eq!(&*cache.get(&sha).unwrap(), &bytes[..]);
    assert_eq!(f.state(&sha), FileState::Cached);
}

#[test]
fn stray_temp_swept_after_24h() {
    let dir = tempfile::tempdir().unwrap();
    let cache = FontCache::open(dir.path()).unwrap();
    let (a, b) = ("a".repeat(64), "b".repeat(64));
    let d = dir.path();
    for name in [
        ".~fin-1.part".to_string(),
        format!("{a}.part"),
        format!("{a}.part.json"),
        format!("{b}.part"),
        format!("{b}.part.json"),
        a.clone(),
        "notes.txt".into(),
    ] {
        fs::write(d.join(name), b"x").unwrap();
    }
    // Another fetcher holds b's lock: its resumable download is not stray.
    let held = File::create(d.join(format!("{b}.lock"))).unwrap();
    held.lock().unwrap();

    let now = SystemTime::now();
    assert_eq!(cache.sweep(now + Duration::from_secs(3600)), 0);
    assert_eq!(cache.sweep(now + STRAY_AFTER + Duration::from_secs(60)), 3);
    assert_eq!(
        names(d),
        [
            a.clone(),
            format!("{b}.lock"),
            format!("{b}.part"),
            format!("{b}.part.json"),
            "notes.txt".into()
        ]
    );
    drop(held);
    assert_eq!(cache.sweep(now + STRAY_AFTER + Duration::from_secs(60)), 2);
    assert_eq!(cache.list(), [a]);
}

#[test]
fn only_sha256_names_are_looked_up() {
    let dir = tempfile::tempdir().unwrap();
    let cache = FontCache::open(&dir.path().join("fonts")).unwrap();
    fs::write(dir.path().join("secret"), b"x").unwrap();
    assert!(cache.get("../secret").is_none());
    assert!(!cache.has("../secret"));
    assert!(cache.get(&"A".repeat(64)).is_none());
    assert!(cache.list().is_empty());
}
