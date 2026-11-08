//! The read-only cache (sub-project 6 plan 6A Decision 3): what media workers open. Builds and
//! runs with and without the `fetch` feature.

use std::fs;

use fin_fonts::{FontCache, sha256_hex};

#[test]
fn a_read_only_cache_reads_verified_files_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = fin_testfonts::test_sans();
    let sha = sha256_hex(&bytes);
    fs::write(dir.path().join(&sha), &bytes).unwrap();
    // A stray temp a writable open would sweep once it is old enough; the read-only one never
    // looks at it.
    let temp = dir.path().join(".~fin-1.part");
    fs::write(&temp, b"partial").unwrap();
    let wrong = sha256_hex(b"other");
    fs::write(dir.path().join(&wrong), b"not the bytes").unwrap();

    let cache = FontCache::open_read_only(dir.path());
    assert_eq!(cache.get(&sha).as_deref(), Some(&bytes[..]));
    assert!(cache.has(&sha));
    assert!(cache.get(&wrong).is_none());
    assert!(cache.mismatch(&wrong).is_some());
    // The mismatched file stays: a read-only cache never removes anything.
    assert!(dir.path().join(&wrong).exists());
    assert!(temp.exists());
}

#[test]
fn a_missing_directory_is_an_empty_cache() {
    let dir = tempfile::tempdir().unwrap();
    let gone = dir.path().join("fonts");
    let cache = FontCache::open_read_only(&gone);
    assert!(cache.get(&sha256_hex(b"x")).is_none());
    assert!(cache.list().is_empty());
    assert!(!gone.exists(), "nothing was created");
}
