//! `asset.probe` of a library file (plan 5D Task 13): `probe_with` over `open_for_read` with
//! the catalogued row, its refusals as their own codes, and never a probe by name.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod support;

use std::fs;
use std::path::Path;

use fin_library::{FileRef, RawRel, probe_asset};
use support::{expect_of, rel};

fn with<T>(dir: &Path, name: &str, hydrate: bool, f: impl FnOnce(&FileRef<'_>) -> T) -> T {
    let path = dir.join(name);
    let r: RawRel = rel(&[name.as_bytes()]);
    let expect = expect_of(&path);
    f(&FileRef {
        file: 1,
        root: dir,
        rel: &r,
        expect: &expect,
        hydrate,
        display: &path,
    })
}

#[test]
fn asset_probe_of_library_file_uses_catalogued_row() {
    let t = tempfile::tempdir().unwrap();
    fin_probe::fixtures::write_png(&t.path().join("a.png"), 64, 36);
    let want = fin_probe::probe(&t.path().join("a.png")).unwrap();
    assert_eq!(with(t.path(), "a.png", false, probe_asset).unwrap(), want);
}

#[test]
fn changed_file_is_file_changed_and_nothing_is_probed() {
    let t = tempfile::tempdir().unwrap();
    fin_probe::fixtures::write_png(&t.path().join("a.png"), 64, 36);
    let r = rel(&[b"a.png"]);
    let mut expect = expect_of(&t.path().join("a.png"));
    expect.size += 1;
    let e = probe_asset(&FileRef {
        file: 1,
        root: t.path(),
        rel: &r,
        expect: &expect,
        hydrate: false,
        display: &t.path().join("a.png"),
    })
    .unwrap_err();
    assert_eq!(e.code, "FILE_CHANGED");
}

#[test]
fn gone_file_is_file_not_found() {
    let t = tempfile::tempdir().unwrap();
    fin_probe::fixtures::write_png(&t.path().join("a.png"), 64, 36);
    let r = rel(&[b"a.png"]);
    let expect = expect_of(&t.path().join("a.png"));
    fs::remove_file(t.path().join("a.png")).unwrap();
    let e = probe_asset(&FileRef {
        file: 1,
        root: t.path(),
        rel: &r,
        expect: &expect,
        hydrate: false,
        display: &t.path().join("a.png"),
    })
    .unwrap_err();
    assert_eq!(e.code, "FILE_NOT_FOUND");
}

#[test]
fn a_link_swapped_in_is_not_followed() {
    let t = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fin_probe::fixtures::write_png(&outside.path().join("secret.png"), 8, 8);
    fin_probe::fixtures::write_png(&t.path().join("a.png"), 64, 36);
    let r = rel(&[b"a.png"]);
    let expect = expect_of(&t.path().join("a.png"));
    fs::remove_file(t.path().join("a.png")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.png"), t.path().join("a.png")).unwrap();
    let e = probe_asset(&FileRef {
        file: 1,
        root: t.path(),
        rel: &r,
        expect: &expect,
        hydrate: false,
        display: &t.path().join("a.png"),
    })
    .unwrap_err();
    assert_eq!(e.code, "PATH_NOT_ALLOWED");
}

#[test]
fn an_unreadable_probe_keeps_probe_withs_own_error() {
    let t = tempfile::tempdir().unwrap();
    fs::write(
        t.path().join("x.png"),
        b"\0\x9f\x92\x96 not an image at all",
    )
    .unwrap();
    let e = with(t.path(), "x.png", false, probe_asset).unwrap_err();
    assert_eq!(e.code, "UNSUPPORTED_CONTAINER");
}
