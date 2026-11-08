//! The safe opener on Windows (plan 5D Task 3, Decision 19).
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(windows)]

mod win_support;

use std::fs;
use std::io::Write;
use std::os::windows::fs::OpenOptionsExt;

use fin_library::{Expect, OpenError, open_for_read};
use win_support::{err, expect_of, junction, read, rel, skip, tempdir};

const SHARE_ALL: u32 = 7; // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
const DELETE: u32 = 0x0001_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;

/// Another application holds the file open for write and delete: it is still read, and
/// while our handle is open that application can still write, rename and delete it.
#[test]
fn opens_with_all_share_flags() {
    let t = tempdir();
    let p = t.path().join("busy.mov");
    fs::write(&p, b"busy bytes").unwrap();
    let mut other = fs::OpenOptions::new()
        .access_mode(GENERIC_WRITE | DELETE)
        .share_mode(SHARE_ALL)
        .open(&p)
        .unwrap();
    let rr = rel("busy.mov");
    let f = open_for_read(t.path(), &rr, &expect_of(t.path(), &rr), false).unwrap();
    other.write_all(b"!").unwrap();
    drop(other);
    fs::rename(&p, t.path().join("moved.mov")).expect("our handle does not block a rename");
    fs::remove_file(t.path().join("moved.mov")).expect("nor a delete");
    assert_eq!(read(f).len(), 10);
}

#[test]
fn open_refuses_junction_component() {
    let outside = tempdir();
    fs::write(outside.path().join("o.mov"), b"outside").unwrap();
    let t = tempdir();
    fs::create_dir(t.path().join("real")).unwrap();
    fs::write(t.path().join("real/f.mov"), b"inside").unwrap();
    junction(&t.path().join("jin"), &t.path().join("real"));
    junction(&t.path().join("jout"), outside.path());
    let inside = expect_of(t.path(), &rel("real/f.mov"));
    let got = err(open_for_read(t.path(), &rel("jin/f.mov"), &inside, false));
    assert!(matches!(got, OpenError::NameSurrogate), "{got:?}");
    let any = Expect {
        size: 7,
        mtime_ns: 0,
        file_id: None,
        dev: None,
    };
    let got = err(open_for_read(t.path(), &rel("jout/o.mov"), &any, false));
    assert!(matches!(got, OpenError::NameSurrogate), "{got:?}");
    // The junction itself as the file.
    let got = err(open_for_read(t.path(), &rel("jout"), &any, false));
    assert!(matches!(got, OpenError::NameSurrogate), "{got:?}");
    // The real path is fine.
    let f = open_for_read(t.path(), &rel("real/f.mov"), &inside, false).unwrap();
    assert_eq!(read(f), b"inside");
}

/// A root that is itself a junction is resolved as given (it is the owner's canonical root);
/// a file symlink below a root is never followed, wherever it points.
#[test]
fn final_path_must_be_under_root() {
    let t = tempdir();
    fs::create_dir(t.path().join("real")).unwrap();
    fs::write(t.path().join("real/f.mov"), b"inside").unwrap();
    junction(&t.path().join("rootlink"), &t.path().join("real"));
    let via = t.path().join("rootlink");
    let f = open_for_read(&via, &rel("f.mov"), &expect_of(&via, &rel("f.mov")), false).unwrap();
    assert_eq!(read(f), b"inside");

    let outside = tempdir();
    fs::write(outside.path().join("o.mov"), b"outside").unwrap();
    match std::os::windows::fs::symlink_file(
        outside.path().join("o.mov"),
        t.path().join("real/l.mov"),
    ) {
        Ok(()) => {
            let any = Expect {
                size: 7,
                mtime_ns: 0,
                file_id: None,
                dev: None,
            };
            let got = err(open_for_read(t.path(), &rel("real/l.mov"), &any, false));
            assert!(matches!(got, OpenError::NameSurrogate), "{got:?}");
        }
        Err(_) => skip(
            "final_path_must_be_under_root",
            "file symlinks need Developer Mode or SeCreateSymbolicLinkPrivilege",
        ),
    }
}

#[test]
fn identity_mismatch_is_changed() {
    let t = tempdir();
    fs::write(t.path().join("f.mov"), b"one").unwrap();
    fs::write(t.path().join("g.mov"), b"two").unwrap();
    fs::create_dir(t.path().join("d")).unwrap();
    let rr = rel("f.mov");
    let good = expect_of(t.path(), &rr);
    let changed = |e: &Expect| {
        matches!(
            err(open_for_read(t.path(), &rr, e, false)),
            OpenError::Changed
        )
    };
    assert!(
        changed(&Expect {
            size: 4,
            ..good.clone()
        }),
        "size"
    );
    assert!(
        changed(&Expect {
            mtime_ns: good.mtime_ns + 100,
            ..good.clone()
        }),
        "mtime"
    );
    let other_id = expect_of(t.path(), &rel("g.mov")).file_id;
    assert!(
        changed(&Expect {
            file_id: other_id,
            ..good.clone()
        }),
        "file id"
    );
    assert!(
        changed(&Expect {
            dev: Some("0000000000000001".into()),
            ..good.clone()
        }),
        "device"
    );
    // Without a device the root's volume is required, and given.
    let f = open_for_read(
        t.path(),
        &rr,
        &Expect {
            dev: None,
            ..good.clone()
        },
        false,
    )
    .unwrap();
    assert_eq!(read(f), b"one");
    // A directory where the file was.
    let got = err(open_for_read(t.path(), &rel("d"), &good, false));
    assert!(matches!(got, OpenError::Changed), "{got:?}");
    // Rewritten in place: same id, new size and mtime.
    fs::write(t.path().join("f.mov"), b"longer").unwrap();
    assert!(changed(&good), "rewritten");
}

#[test]
fn missing_file_and_stubs() {
    let t = tempdir();
    fs::write(t.path().join("clip.mov.nextcloud"), b"").unwrap();
    let e = Expect {
        size: 10,
        mtime_ns: 1,
        file_id: None,
        dev: None,
    };
    let got = err(open_for_read(t.path(), &rel("clip.mov"), &e, false));
    assert!(
        matches!(got, OpenError::CloudOnly),
        "a stub stands for it: {got:?}"
    );
    let got = err(open_for_read(t.path(), &rel("gone.mov"), &e, false));
    assert!(matches!(got, OpenError::NotFound), "{got:?}");
    let got = err(open_for_read(t.path(), &rel("nodir/x.mov"), &e, false));
    assert!(matches!(got, OpenError::NotFound), "{got:?}");
    let bad = fin_library::RawRel(vec![b"..".to_vec(), b"x".to_vec()]);
    assert!(matches!(
        err(open_for_read(t.path(), &bad, &e, false)),
        OpenError::Escapes
    ));
    // An alternate data stream of a real file is never opened.
    fs::write(t.path().join("f.mov"), b"main").unwrap();
    let ads = fin_library::RawRel(vec![b"f.mov:hidden".to_vec()]);
    assert!(matches!(
        err(open_for_read(t.path(), &ads, &e, false)),
        OpenError::Io(_)
    ));
    let bs = fin_library::RawRel(vec![b"a\\b".to_vec()]);
    assert!(matches!(
        err(open_for_read(t.path(), &bs, &e, false)),
        OpenError::Escapes
    ));
}
