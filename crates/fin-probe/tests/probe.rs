use fin_core::Rational;
use fin_probe::fixtures::{FixtureOpts, write_fixture_mov, write_fixture_mp4};
use fin_probe::{Probe, fingerprint, probe};

fn write_fixture(path: &std::path::Path) {
    write_fixture_mp4(path, FixtureOpts::default());
}

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
    write_fixture(&path);
    let p = probe(&path).unwrap();
    assert_eq!(p.kind, fin_core::AssetKind::Media);
    assert_eq!(p.duration, Some(r(2, 1)));
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
    // `Path::ends_with` compares components: `/` and `\` both separate them on Windows.
    assert!(
        std::path::Path::new(p).is_absolute()
            && std::path::Path::new(p).ends_with("nope/missing.mp4"),
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
    write_fixture(&path);
    assert_eq!(probe(&path).unwrap().duration, Some(r(2, 1)));
}

#[test]
fn missing_relative_path_is_cwd_joined() {
    let err = probe(std::path::Path::new("nope/missing.mp4")).unwrap_err();
    // Absolutising normalises separators (backslashes on Windows).
    let expected = std::env::current_dir()
        .unwrap()
        .join("nope")
        .join("missing.mp4");
    assert_eq!(
        err.details["path"].as_str().unwrap(),
        expected.to_str().unwrap()
    );
}

#[test]
fn path_through_a_file_is_unreadable() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("plain.txt");
    std::fs::write(&file, b"x").unwrap();
    let err = probe(&file.join("child.mp4")).unwrap_err();
    assert_eq!(err.code, "ASSET_UNREADABLE");
    assert!(err.details["reason"].is_string());
    assert!(std::path::Path::new(err.details["path"].as_str().unwrap()).is_absolute());
    assert!(err.fix.is_some());
}

#[test]
fn audio_rate_is_track_timescale_above_65535() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hi.mp4");
    write_fixture_mp4(
        &path,
        FixtureOpts {
            sample_rate: 96_000,
            ..FixtureOpts::default()
        },
    );
    let a = probe(&path).unwrap().audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (96_000, 2));
}

#[test]
fn video_only_file_has_no_audio() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.mp4");
    write_fixture_mp4(
        &path,
        FixtureOpts {
            audio: false,
            fps: 30,
            ..FixtureOpts::default()
        },
    );
    let p = probe(&path).unwrap();
    assert!(p.audio.is_none());
    assert_eq!(p.video.unwrap().rate, r(30, 1));
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

#[test]
fn fragmented_mp4_is_rejected() {
    // ftyp + moov{mvhd(v0, zero duration), mvex{trex}}: what a fragmented file starts with.
    let mut mvhd = vec![0u8; 100];
    mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes()); // timescale
    mvhd[16 + 4..16 + 6].copy_from_slice(&[1, 0]); // rate 1.0 (16.16)
    mvhd[20..22].copy_from_slice(&[1, 0]); // volume
    let matrix = [0x10000u32, 0, 0, 0, 0x10000, 0, 0, 0, 0x40000000];
    for (i, m) in matrix.iter().enumerate() {
        mvhd[36 + i * 4..40 + i * 4].copy_from_slice(&m.to_be_bytes());
    }
    mvhd[96..100].copy_from_slice(&2u32.to_be_bytes()); // next track id
    let trex = boxed(b"trex", &[0u8; 24]);
    let mut moov = boxed(b"mvhd", &mvhd);
    moov.extend(boxed(b"mvex", &trex));
    let mut bytes = boxed(b"ftyp", b"isom\0\0\x02\0isom");
    bytes.extend(boxed(b"moov", &moov));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frag.mp4");
    std::fs::write(&path, bytes).unwrap();
    let err = probe(&path).unwrap_err();
    assert_eq!(err.code, "UNSUPPORTED_CONTAINER");
    assert_eq!(err.details["reason"], "fragmented mp4", "{err:?}");
}

#[test]
fn probe_mov_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mov");
    write_fixture_mov(&path, FixtureOpts::default());
    let bytes = std::fs::read(&path).unwrap();
    // A QuickTime file: `ftyp` with major brand `qt  `.
    assert_eq!(&bytes[4..12], b"ftypqt  ");
    let p = probe(&path).unwrap();
    assert_eq!(p.duration, Some(r(2, 1)));
    let v = p.video.unwrap();
    assert_eq!((v.width, v.height, v.rate), (320, 180, r(25, 1)));
    let a = p.audio.unwrap();
    assert_eq!((a.sample_rate, a.channels), (48_000, 2));
    assert_eq!(p.size, bytes.len() as u64);
}

/// The JSON fin-node sends for a probe is `Probe`'s serde form, which the generated TS type
/// (`@finisher/core-types`) describes.
#[test]
fn probe_json_is_the_serde_form() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("clip.mp4");
    write_fixture(&path);
    let p = probe(&path).unwrap();
    let v = serde_json::to_value(&p).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["audio", "duration", "fingerprint", "kind", "size", "video"]
    );
    assert_eq!(v["kind"], "media");
    assert_eq!(v["duration"], "2");
    assert_eq!(v["video"]["rate"], "25");
    assert_eq!(v["audio"]["sampleRate"], 48_000);
    assert_eq!(serde_json::from_value::<Probe>(v).unwrap(), p);
}
