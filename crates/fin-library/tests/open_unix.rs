//! The safe opener on Linux and macOS (plan 5D Task 2, Decision 19).
#![cfg(unix)]

mod support;

use std::fs;
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;

use fin_library::{Expect, OpenError, RawRel, open_for_read};
use support::{expect_of, rel};

fn read(f: fs::File) -> Vec<u8> {
    let mut v = Vec::new();
    let mut f = f;
    f.read_to_end(&mut v).unwrap();
    v
}

fn err(r: Result<fs::File, OpenError>) -> OpenError {
    r.expect_err("the open should have been refused")
}

#[test]
fn opens_non_utf8_name_by_raw_chain() {
    let t = tempfile::tempdir().unwrap();
    // NFD `café.mov` (e + U+0301); every OS stores it as given.
    let nfd: &[u8] = b"cafe\xcc\x81.mov";
    let path_of = |parts: &[&[u8]]| {
        let mut p = t.path().to_path_buf();
        for c in parts {
            p.push(std::ffi::OsStr::from_bytes(c));
        }
        p
    };
    fs::write(path_of(&[nfd]), b"nfd").unwrap();
    let r = rel(&[nfd]);
    let f = open_for_read(t.path(), &r, &expect_of(&path_of(&[nfd])), false).unwrap();
    assert_eq!(read(f), b"nfd");

    #[cfg(target_os = "linux")]
    {
        // Linux names are bytes: a non-UTF-8 directory and file, and an NFC twin of the NFD
        // name with different content, each opened by its own raw chain.
        let dir: &[u8] = b"d\xff";
        let file: &[u8] = b"f\xfe.mov";
        let nfc: &[u8] = "café.mov".as_bytes();
        fs::create_dir(path_of(&[dir])).unwrap();
        fs::write(path_of(&[dir, file]), b"raw").unwrap();
        fs::write(path_of(&[nfc]), b"nfc").unwrap();
        let f = open_for_read(
            t.path(),
            &rel(&[dir, file]),
            &expect_of(&path_of(&[dir, file])),
            false,
        )
        .unwrap();
        assert_eq!(read(f), b"raw");
        let f = open_for_read(t.path(), &rel(&[nfc]), &expect_of(&path_of(&[nfc])), false).unwrap();
        assert_eq!(read(f), b"nfc");
        let f = open_for_read(t.path(), &r, &expect_of(&path_of(&[nfd])), false).unwrap();
        assert_eq!(
            read(f),
            b"nfd",
            "the twin did not take the NFD name's place"
        );
    }
}

fn link_tree() -> (tempfile::TempDir, tempfile::TempDir) {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("o.mov"), b"outside").unwrap();
    let t = tempfile::tempdir().unwrap();
    fs::create_dir(t.path().join("real")).unwrap();
    fs::write(t.path().join("real/f.mov"), b"inside").unwrap();
    symlink("real", t.path().join("dirlink")).unwrap();
    symlink("real/f.mov", t.path().join("filelink.mov")).unwrap();
    symlink(outside.path(), t.path().join("out")).unwrap();
    (t, outside)
}

fn assert_link_refusals(t: &tempfile::TempDir, outside: &tempfile::TempDir) {
    let e = expect_of(&t.path().join("real/f.mov"));
    let ok = open_for_read(t.path(), &rel(&[b"real", b"f.mov"]), &e, false).unwrap();
    assert_eq!(read(ok), b"inside");
    assert!(matches!(
        err(open_for_read(
            t.path(),
            &rel(&[b"dirlink", b"f.mov"]),
            &e,
            false
        )),
        OpenError::NameSurrogate
    ));
    assert!(matches!(
        err(open_for_read(t.path(), &rel(&[b"filelink.mov"]), &e, false)),
        OpenError::NameSurrogate
    ));
    let eo = expect_of(&outside.path().join("o.mov"));
    assert!(matches!(
        err(open_for_read(
            t.path(),
            &rel(&[b"out", b"o.mov"]),
            &eo,
            false
        )),
        OpenError::NameSurrogate
    ));
}

#[test]
fn open_refuses_symlink_component() {
    let (t, outside) = link_tree();
    assert_link_refusals(&t, &outside);
}

#[test]
fn open_stays_beneath_root() {
    let (t, outside) = link_tree();
    let e = expect_of(&outside.path().join("o.mov"));
    // A wire path that names anything outside its chain is refused before any open.
    let dots = RawRel(vec![b"..".to_vec(), b"o.mov".to_vec()]);
    assert!(matches!(
        err(open_for_read(t.path(), &dots, &e, false)),
        OpenError::Escapes
    ));
    let slash = RawRel(vec![b"real/../../x".to_vec()]);
    assert!(matches!(
        err(open_for_read(t.path(), &slash, &e, false)),
        OpenError::Escapes
    ));
    assert!(matches!(
        err(open_for_read(t.path(), &RawRel::root(), &e, false)),
        OpenError::Io(_)
    ));
    assert!(matches!(
        err(open_for_read(t.path(), &rel(&[b"nope.mov"]), &e, false)),
        OpenError::NotFound
    ));
    assert!(matches!(
        err(open_for_read(t.path(), &rel(&[b"real"]), &e, false)),
        OpenError::Changed
    ));
}

#[test]
fn identity_mismatch_is_changed() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("a.mov");
    fs::write(&p, b"content").unwrap();
    let good = expect_of(&p);
    let r = rel(&[b"a.mov"]);
    assert_eq!(
        read(open_for_read(t.path(), &r, &good, false).unwrap()),
        b"content"
    );
    let no_id = Expect {
        file_id: None,
        ..good.clone()
    };
    assert!(
        open_for_read(t.path(), &r, &no_id, false).is_ok(),
        "no catalogued id: size and mtime decide"
    );
    for bad in [
        Expect {
            size: good.size + 1,
            ..good.clone()
        },
        Expect {
            mtime_ns: good.mtime_ns + 1,
            ..good.clone()
        },
        Expect {
            file_id: Some(vec![0, 0, 0, 0, 0, 0, 0, 1]),
            ..good.clone()
        },
    ] {
        assert!(matches!(
            err(open_for_read(t.path(), &r, &bad, false)),
            OpenError::Changed
        ));
    }
    // Replaced (renamed over) by another file: same name, a new inode. With the new size and
    // mtime but the catalogued id, the open is still refused.
    fs::write(t.path().join("new"), b"CONTENT").unwrap();
    fs::rename(t.path().join("new"), &p).unwrap();
    let replaced = Expect {
        file_id: good.file_id.clone(),
        ..expect_of(&p)
    };
    assert!(matches!(
        err(open_for_read(t.path(), &r, &replaced, false)),
        OpenError::Changed
    ));
}

#[test]
fn placeholder_refused_before_open() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("x.mov.nextcloud"), b"").unwrap();
    fs::write(t.path().join("o.mov.owncloud"), b"").unwrap();
    fs::write(t.path().join(".y.mov.icloud"), b"bplist").unwrap();
    let e = Expect {
        size: 10,
        mtime_ns: 1,
        file_id: None,
    };
    for name in [&b"x.mov"[..], b"o.mov", b"y.mov"] {
        for hydrate in [false, true] {
            assert!(
                matches!(
                    err(open_for_read(t.path(), &rel(&[name]), &e, hydrate)),
                    OpenError::CloudOnly
                ),
                "{}",
                String::from_utf8_lossy(name)
            );
        }
    }
    // A FIFO where a file was catalogued: refused before an open could block on it.
    let fifo = t.path().join("pipe.mov");
    let c = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
    // SAFETY: a valid C string.
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    assert!(matches!(
        err(open_for_read(t.path(), &rel(&[b"pipe.mov"]), &e, false)),
        OpenError::Changed
    ));
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use fin_library::open::hooks::{self, Forced};
    use fin_library::{Source, source::NativeSource};

    #[test]
    fn openat2_enosys_and_eperm_fall_back_per_component() {
        let (t, outside) = link_tree();
        fs::create_dir_all(t.path().join("a/b/c")).unwrap();
        fs::write(t.path().join("a/b/c/deep.mov"), b"deep").unwrap();
        let e = expect_of(&t.path().join("a/b/c/deep.mov"));
        for errno in [libc::ENOSYS, libc::EPERM] {
            let forced = Forced {
                openat2_errno: Some(errno),
                noatime_eperm: false,
            };
            let ((), counts) = hooks::with(forced, || {
                let f = open_for_read(t.path(), &rel(&[b"a", b"b", b"c", b"deep.mov"]), &e, false)
                    .unwrap();
                assert_eq!(read(f), b"deep");
                assert_link_refusals(&t, &outside);
            });
            assert!(counts.fallbacks >= 2, "errno {errno}: {counts:?}");
        }
        // Without hooks the same opens take openat2 (kernel ≥ 5.6) and agree.
        let ((), counts) = hooks::with(Forced::default(), || assert_link_refusals(&t, &outside));
        assert_eq!(counts.fallbacks, 0, "this kernel has openat2");
    }

    #[test]
    fn noatime_dropped_after_eperm() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join("d")).unwrap();
        fs::write(t.path().join("d/f.mov"), b"f").unwrap();
        let e = expect_of(&t.path().join("d/f.mov"));
        let r = rel(&[b"d", b"f.mov"]);
        let forced = Forced {
            openat2_errno: None,
            noatime_eperm: true,
        };
        let ((), counts) = hooks::with(forced, || {
            for _ in 0..3 {
                assert_eq!(read(open_for_read(t.path(), &r, &e, false).unwrap()), b"f");
            }
        });
        assert_eq!(
            counts.noatime_drops, 1,
            "dropped once, then not asked again"
        );

        let src = NativeSource::new(t.path());
        assert!(src.uses_noatime());
        let ((), counts) = hooks::with(forced, || {
            for _ in 0..3 {
                let d = src.open_dir(&rel(&[b"d"])).unwrap();
                assert_eq!(src.read_dir(&d).unwrap().len(), 1);
            }
        });
        assert!(!src.uses_noatime());
        assert_eq!(counts.noatime_drops, 1);
    }
}
