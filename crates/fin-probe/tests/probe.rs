mod support;

use fin_core::Rational;
use fin_probe::{fingerprint, probe};

fn r(n: i64, d: i64) -> Rational {
    Rational::new(n, d).unwrap()
}

/// Flips one byte but restores the mtime, so only content can change the fingerprint.
fn rewrite_keeping_mtime(path: &std::path::Path, bytes: &mut [u8], at: usize) {
    let mtime = std::fs::metadata(path).unwrap().modified().unwrap();
    bytes[at] ^= 1;
    std::fs::write(path, &*bytes).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

#[test]
fn probes_generated_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    support::write_fixture(&path);
    let p = probe(&path).unwrap();
    assert_eq!(p.duration, r(2, 1));
    let v = p.video.unwrap();
    assert_eq!((v.width, v.height, v.rate), (320, 180, r(25, 1)));
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    assert_eq!(p.size, std::fs::metadata(&path).unwrap().len());
    assert!(p.fingerprint.starts_with("fp1:"));
}

#[test]
fn fingerprint_is_stable_and_sees_middle_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.bin");
    let mut bytes = vec![7u8; 1024 * 1024];
    std::fs::write(&path, &bytes).unwrap();
    let a = fingerprint(&path).unwrap();
    assert_eq!(a, fingerprint(&path).unwrap());
    let mid = bytes.len() / 2;
    rewrite_keeping_mtime(&path, &mut bytes, mid);
    assert_ne!(a, fingerprint(&path).unwrap());
}

#[test]
fn small_files_are_hashed_whole() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.bin");
    let mut bytes = vec![1u8; 100 * 1024];
    std::fs::write(&path, &bytes).unwrap();
    let a = fingerprint(&path).unwrap();
    // Outside the head/middle/tail samples a larger file would use.
    rewrite_keeping_mtime(&path, &mut bytes, 70 * 1024);
    assert_ne!(a, fingerprint(&path).unwrap());
}

#[test]
fn missing_file_reports_absolute_path() {
    let err = probe(std::path::Path::new("nope/missing.mp4")).unwrap_err();
    assert_eq!(err.code, "ASSET_NOT_FOUND");
    let p = err.details["path"].as_str().unwrap();
    assert!(
        std::path::Path::new(p).is_absolute() && p.ends_with("nope/missing.mp4"),
        "{p}"
    );
}

#[test]
fn non_mp4_is_unsupported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.mp4");
    std::fs::write(&path, b"definitely not an mp4 container, just text").unwrap();
    assert_eq!(probe(&path).unwrap_err().code, "UNSUPPORTED_CONTAINER");
}

#[test]
fn spaces_and_cyrillic_paths_work() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("мої відео");
    std::fs::create_dir(&sub).unwrap();
    let path = sub.join("кліп 1.mp4");
    support::write_fixture(&path);
    assert_eq!(probe(&path).unwrap().duration, r(2, 1));
}
