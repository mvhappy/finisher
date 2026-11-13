//! The reader entry points (plan 5D Task 5, Decisions 20 and 35): `probe_reader` over any
//! `Read + Seek` gives what `probe` gives, `probe_with` opens through the caller's opener
//! once, the fingerprint survives a failed probe, and `PROBE_VERSION` exists.

use std::cell::Cell;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use fin_probe::fixtures::{
    FixtureOpts, sample_lottie, write_cube, write_dotlottie, write_fixture_mov, write_fixture_mp4,
    write_jpeg, write_m4a, write_png, write_svg, write_wav, write_webp,
};
use fin_probe::{FileMeta, PROBE_VERSION, fingerprint, probe, probe_reader, probe_with};

fn fixtures(dir: &Path) -> Vec<PathBuf> {
    let at = |n: &str| dir.join(n);
    write_fixture_mp4(&at("a.mp4"), FixtureOpts::default());
    write_fixture_mov(&at("a.mov"), FixtureOpts::default());
    write_m4a(&at("a.m4a"), FixtureOpts::default());
    write_png(&at("a.png"), 64, 36);
    write_jpeg(&at("a.jpg"), 48, 27);
    write_webp(&at("a.webp"), 32, 18);
    write_svg(&at("a.svg"), 100, 50);
    write_dotlottie(&at("a.lottie"), &sample_lottie(), true);
    write_dotlottie(&at("b.lottie"), &sample_lottie(), false);
    write_cube(&at("a.cube"), 4);
    write_wav(&at("a.wav"), 48_000, 2, 4800);
    std::fs::write(at("a.ttf"), fin_testfonts::test_sans()).unwrap();
    std::fs::write(
        at("a.ttc"),
        fin_testfonts::collection(&[fin_testfonts::test_sans(), fin_testfonts::test_han()]),
    )
    .unwrap();
    // Over 192 KiB: the fingerprint samples the head, middle and tail.
    std::fs::write(at("big.ttf"), fin_testfonts::test_big(1024 * 1024)).unwrap();
    // Failures: not a kind, and a truncated PNG.
    std::fs::write(at("junk.bin"), [0u8, 159, 146, 150, 1, 2, 3, 4, 5, 6]).unwrap();
    let png = std::fs::read(at("a.png")).unwrap();
    std::fs::write(at("cut.png"), &png[..png.len().min(12)]).unwrap();
    std::fs::write(at("empty.dat"), b"").unwrap();
    let mut all: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    all.sort();
    all
}

#[test]
fn probe_reader_equals_probe() {
    let dir = tempfile::tempdir().unwrap();
    let all = fixtures(dir.path());
    assert!(all.len() >= 17);
    for path in all {
        let want = probe(&path);
        let meta = FileMeta::of(&std::fs::metadata(&path).unwrap());
        let bytes = std::fs::read(&path).unwrap();
        let out = probe_reader(Cursor::new(bytes), meta, &path);
        match (&want, &out.probe) {
            (Ok(a), Ok(b)) => {
                assert_eq!(a, b, "{}", path.display());
                assert!(a.fingerprint.starts_with("fp1:"));
                assert_eq!(out.fingerprint.as_deref(), Some(a.fingerprint.as_str()));
                assert_eq!(out.fingerprint, fingerprint(&path).ok());
            }
            (Err(a), Err(b)) => {
                assert_eq!((&a.code, &a.details), (&b.code, &b.details), "{path:?}");
                assert_eq!(out.fingerprint, fingerprint(&path).ok(), "{path:?}");
            }
            _ => panic!("{}: {want:?} vs {:?}", path.display(), out.probe),
        }
    }
}

#[test]
fn probe_with_uses_the_given_opener() {
    let dir = tempfile::tempdir().unwrap();
    write_png(&dir.path().join("real.png"), 64, 36);
    // The path named in errors and the file the opener returns are different things.
    let named = Path::new("library/virtual.png");
    let asked = Cell::new(None);
    let p = probe_with(named, |path| {
        asked.set(Some(path.to_path_buf()));
        File::open(dir.path().join("real.png"))
    })
    .unwrap();
    assert_eq!(asked.take().as_deref(), Some(named));
    let i = p.image.unwrap();
    assert_eq!((i.width, i.height), (64, 36));
    // An opener error is the probe's error, naming the path.
    let e = probe_with(named, |_| {
        Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
    })
    .unwrap_err();
    assert_eq!(e.code, "ASSET_UNREADABLE");
    // A probe failure carries the path it was given.
    std::fs::write(dir.path().join("junk.bin"), [0u8, 1, 2, 3]).unwrap();
    let e = probe_with(named, |_| File::open(dir.path().join("junk.bin"))).unwrap_err();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
    assert!(
        e.details["path"].as_str().unwrap().ends_with("virtual.png"),
        "{e:?}"
    );
}

#[test]
fn probe_entry_uses_one_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.mp4");
    write_fixture_mp4(&path, FixtureOpts::default());
    let opens = Cell::new(0);
    let p = probe_with(&path, |path| {
        opens.set(opens.get() + 1);
        File::open(path)
    })
    .unwrap();
    // The metadata, the fingerprint and the probe all come from the one handle.
    assert_eq!(opens.get(), 1);
    assert!(p.media.is_some());
    // A failing kind opens once too.
    std::fs::write(dir.path().join("j.bin"), [0u8, 1, 2]).unwrap();
    opens.set(0);
    let _ = probe_with(&dir.path().join("j.bin"), |path| {
        opens.set(opens.get() + 1);
        File::open(path)
    });
    assert_eq!(opens.get(), 1);
}

#[test]
fn fp_survives_probe_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("junk.bin");
    std::fs::write(&path, [0u8, 159, 146, 150, 1, 2, 3, 4]).unwrap();
    let meta = FileMeta::of(&std::fs::metadata(&path).unwrap());
    let out = probe_reader(Cursor::new(std::fs::read(&path).unwrap()), meta, &path);
    assert_eq!(out.probe.unwrap_err().code, "UNSUPPORTED_CONTAINER");
    assert_eq!(out.fingerprint, Some(fingerprint(&path).unwrap()));
    // A reader that fails under the fingerprint has none, and the probe fails with it.
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("boom"))
        }
    }
    impl Seek for Broken {
        fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
            Ok(0)
        }
    }
    let out = probe_reader(
        Broken,
        FileMeta {
            size: 8,
            mtime_ns: 0,
        },
        &path,
    );
    assert_eq!(out.fingerprint, None);
    assert_eq!(out.probe.unwrap_err().code, "ASSET_UNREADABLE");
}

#[test]
fn probe_version_is_exported() {
    assert_eq!(PROBE_VERSION, 2);
}
