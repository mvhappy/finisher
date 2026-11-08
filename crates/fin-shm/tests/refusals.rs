//! Every refusal of spec §3.4: links at any segment, hard links, FIFOs, devices, directories,
//! sizes against the header and the cap, a file truncated or grown while it is read, an
//! overflowing header; Windows junctions and reparse points (`#[cfg(windows)]`, need a
//! Windows run).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_shm::{JobDir, RingReader, RingSpec, RingWriter, SlotState, checked_open, encode_header};

const CAP: u64 = 1 << 20;

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    (t, root)
}

fn reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "SHM_REFUSED", "{e:?}");
    e.details["reason"].as_str().unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    File::create_new(path).unwrap().write_all(bytes).unwrap();
}

fn spec() -> RingSpec {
    RingSpec {
        slot_bytes: 256,
        slots: 2,
    }
}

fn ring(path: &Path) -> RingWriter {
    RingWriter::create(path, spec(), CAP).unwrap()
}

#[test]
fn a_regular_file_opens() {
    let (_t, root) = tmp();
    write(&root.join("f"), b"abc");
    let f = checked_open(&root.join("f"), 3).unwrap();
    assert_eq!(f.metadata().unwrap().len(), 3);
}

#[test]
fn paths_that_are_not_absolute_or_have_dot_segments_are_refused() {
    let (_t, root) = tmp();
    assert_eq!(
        reason(&checked_open(Path::new("rel/f"), CAP).unwrap_err()),
        "notAbsolute"
    );
    let dotted = root.join("a").join("..").join("f");
    assert_eq!(
        reason(&checked_open(&dotted, CAP).unwrap_err()),
        "notAbsolute"
    );
}

#[test]
fn a_missing_file_is_refused() {
    let (_t, root) = tmp();
    assert_eq!(
        reason(&checked_open(&root.join("none"), CAP).unwrap_err()),
        "missing"
    );
}

#[test]
fn a_directory_is_not_a_file() {
    let (_t, root) = tmp();
    std::fs::create_dir(root.join("d")).unwrap();
    assert_eq!(
        reason(&checked_open(&root.join("d"), CAP).unwrap_err()),
        "notRegular"
    );
    // A file where a directory segment is expected.
    write(&root.join("f"), b"x");
    assert_eq!(
        reason(&checked_open(&root.join("f").join("g"), CAP).unwrap_err()),
        "notRegular"
    );
}

#[test]
fn a_file_over_the_cap_is_refused() {
    let (_t, root) = tmp();
    write(&root.join("f"), &[0u8; 11]);
    assert_eq!(
        reason(&checked_open(&root.join("f"), 10).unwrap_err()),
        "overCap"
    );
    drop(ring(&root.join("r")));
    let small = spec().file_bytes().unwrap() - 1;
    assert_eq!(
        reason(&RingReader::open_untrusted(&root.join("r"), small).unwrap_err()),
        "overCap"
    );
    assert_eq!(
        reason(
            &RingWriter::create(&root.join("r2"), spec(), small)
                .err()
                .unwrap()
        ),
        "overCap"
    );
    assert!(!root.join("r2").exists(), "nothing is created over the cap");
}

#[test]
fn a_ring_larger_or_smaller_than_its_header_is_refused() {
    let (_t, root) = tmp();
    let path = root.join("r");
    drop(ring(&path));
    let size = spec().file_bytes().unwrap();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"x")
        .unwrap();
    assert_eq!(
        reason(&RingReader::open_untrusted(&path, CAP).err().unwrap()),
        "sizeMismatch"
    );
    assert_eq!(
        reason(&RingReader::open_sealed(&path, CAP).err().unwrap()),
        "sizeMismatch"
    );
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(size - 1)
        .unwrap();
    assert_eq!(
        reason(&RingReader::open_untrusted(&path, CAP).err().unwrap()),
        "sizeMismatch"
    );
    let short = root.join("short");
    write(&short, &encode_header(&spec())[..20]);
    assert_eq!(
        reason(&RingReader::open_untrusted(&short, CAP).err().unwrap()),
        "header"
    );
}

#[test]
fn a_header_whose_size_overflows_is_refused() {
    let (_t, root) = tmp();
    let path = root.join("r");
    let spec = RingSpec {
        slot_bytes: u64::MAX / 2,
        slots: 3,
    };
    let mut bytes = encode_header(&spec).to_vec();
    bytes.resize(4096, 0);
    write(&path, &bytes);
    assert_eq!(
        reason(&RingReader::open_untrusted(&path, u64::MAX).err().unwrap()),
        "header"
    );
    assert_eq!(
        reason(
            &RingWriter::create(&root.join("w"), spec, u64::MAX)
                .err()
                .unwrap()
        ),
        "header"
    );
}

#[test]
fn a_ring_truncated_while_it_is_read_is_a_typed_error() {
    let (_t, root) = tmp();
    let path = root.join("r");
    let mut w = ring(&path);
    w.write_slot(0, &[7u8; 200]).unwrap();
    w.write_slot(1, &[8u8; 200]).unwrap();
    drop(w);
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    let mut buf = Vec::new();
    assert!(matches!(
        r.read_slot(0, &mut buf).unwrap(),
        SlotState::Ready { len: 200, .. }
    ));
    // Cut into slot 0's payload: the reader's next copy comes back short.
    let l = spec().layout().unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(l.slot_offset(0) + 10)
        .unwrap();
    assert_eq!(reason(&r.read_slot(0, &mut buf).unwrap_err()), "truncated");
    assert!(buf.is_empty());
    // Below the slot table: even the sequence read is short.
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(20)
        .unwrap();
    assert_eq!(reason(&r.read_slot(1, &mut buf).unwrap_err()), "truncated");
}

#[test]
fn a_ring_that_grows_or_gains_a_link_after_the_open_is_refused_at_the_next_read() {
    let (_t, root) = tmp();
    let path = root.join("r");
    ring(&path).write_slot(0, b"x").unwrap();
    let mut r = RingReader::open_untrusted(&path, CAP).unwrap();
    let mut buf = Vec::new();
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"more")
        .unwrap();
    assert_eq!(reason(&r.read_slot(0, &mut buf).unwrap_err()), "changed");
    let path2 = root.join("r2");
    ring(&path2).write_slot(0, b"x").unwrap();
    let mut r2 = RingReader::open_untrusted(&path2, CAP).unwrap();
    std::fs::hard_link(&path2, root.join("r2-link")).unwrap();
    assert_eq!(
        reason(&r2.read_slot(0, &mut buf).unwrap_err()),
        "hardLinked"
    );
}

#[test]
fn a_hard_linked_file_is_refused_by_both_names() {
    let (_t, root) = tmp();
    drop(ring(&root.join("r")));
    std::fs::hard_link(root.join("r"), root.join("other")).unwrap();
    for name in ["r", "other"] {
        assert_eq!(
            reason(&checked_open(&root.join(name), CAP).unwrap_err()),
            "hardLinked"
        );
        assert_eq!(
            reason(
                &RingReader::open_untrusted(&root.join(name), CAP)
                    .err()
                    .unwrap()
            ),
            "hardLinked"
        );
        assert_eq!(
            reason(
                &RingWriter::open_sized(&root.join(name), spec(), CAP)
                    .err()
                    .unwrap()
            ),
            "hardLinked"
        );
    }
}

#[test]
fn a_helper_id_must_be_a_plain_name() {
    let (_t, root) = tmp();
    let job = JobDir::resolve(&root.join("job1")).unwrap();
    assert_eq!(job.input(), root.join("job1").join("in"));
    assert_eq!(job.output(), root.join("job1").join("out"));
    assert_eq!(
        job.helper_output("codec-1").unwrap(),
        root.join("job1/helpers/codec-1/out")
    );
    for bad in ["", "..", "a/b", "a\\b", &"x".repeat(65)] {
        assert_eq!(
            reason(&job.helper_output(bad).unwrap_err()),
            "badName",
            "{bad:?}"
        );
    }
}

#[cfg(unix)]
mod unix {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::symlink;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_symlink_as_the_file_is_refused() {
        let (_t, root) = tmp();
        drop(ring(&root.join("real")));
        symlink(root.join("real"), root.join("link")).unwrap();
        assert_eq!(
            reason(&checked_open(&root.join("link"), CAP).unwrap_err()),
            "link"
        );
        assert_eq!(
            reason(
                &RingReader::open_untrusted(&root.join("link"), CAP)
                    .err()
                    .unwrap()
            ),
            "link"
        );
        assert_eq!(
            reason(
                &RingReader::open_sealed(&root.join("link"), CAP)
                    .err()
                    .unwrap()
            ),
            "link"
        );
        assert_eq!(
            reason(
                &RingWriter::open_sized(&root.join("link"), spec(), CAP)
                    .err()
                    .unwrap()
            ),
            "link"
        );
        // A dangling one too.
        symlink(root.join("nowhere"), root.join("dangling")).unwrap();
        assert_eq!(
            reason(&checked_open(&root.join("dangling"), CAP).unwrap_err()),
            "link"
        );
    }

    #[test]
    fn a_symlink_at_any_directory_segment_is_refused() {
        let (_t, root) = tmp();
        let real = root.join("real");
        std::fs::create_dir_all(real.join("out")).unwrap();
        drop(ring(&real.join("out").join("r")));
        // The job directory itself, and a directory inside it.
        symlink(&real, root.join("job")).unwrap();
        let through_job = root.join("job").join("out").join("r");
        assert_eq!(
            reason(&checked_open(&through_job, CAP).unwrap_err()),
            "link"
        );
        std::fs::create_dir(root.join("job2")).unwrap();
        symlink(real.join("out"), root.join("job2").join("out")).unwrap();
        let through_out = root.join("job2").join("out").join("r");
        assert_eq!(
            reason(&checked_open(&through_out, CAP).unwrap_err()),
            "link"
        );
        assert_eq!(
            reason(&RingReader::open_untrusted(&through_out, CAP).err().unwrap()),
            "link"
        );
        // The writer never creates through a link either.
        let e = RingWriter::create(&root.join("job2").join("out").join("new"), spec(), CAP)
            .err()
            .unwrap();
        assert_eq!(reason(&e), "link");
        assert!(!real.join("out").join("new").exists());
    }

    #[test]
    fn the_writer_never_creates_over_a_planted_link() {
        let (_t, root) = tmp();
        write(&root.join("victim"), b"keep");
        symlink(root.join("victim"), root.join("r")).unwrap();
        assert_eq!(
            reason(
                &RingWriter::create(&root.join("r"), spec(), CAP)
                    .err()
                    .unwrap()
            ),
            "exists"
        );
        assert_eq!(std::fs::read(root.join("victim")).unwrap(), b"keep");
    }

    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let (_t, root) = tmp();
        let path = root.join("fifo");
        let c = CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let a = checked_open(&p, CAP).err().map(|e| reason(&e).to_string());
            let b = RingReader::open_untrusted(&p, CAP)
                .err()
                .map(|e| reason(&e).to_string());
            tx.send((a, b)).unwrap();
        });
        let (a, b) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("opening a FIFO blocked");
        assert_eq!(a.as_deref(), Some("notRegular"));
        assert_eq!(b.as_deref(), Some("notRegular"));
    }

    #[test]
    fn a_device_is_refused() {
        assert_eq!(
            reason(&checked_open(Path::new("/dev/null"), CAP).unwrap_err()),
            "notRegular"
        );
    }
}

#[cfg(windows)]
mod windows {
    use std::process::Command;

    use super::*;

    fn mklink(args: &[&str]) -> bool {
        Command::new("cmd")
            .arg("/C")
            .arg("mklink")
            .args(args)
            .status()
            .is_ok_and(|s| s.success())
    }

    #[test]
    fn a_junction_at_a_directory_segment_is_refused() {
        let (_t, root) = tmp();
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        drop(ring(&real.join("r")));
        let junction = root.join("out");
        assert!(mklink(&[
            "/J",
            junction.to_str().unwrap(),
            real.to_str().unwrap()
        ]));
        assert_eq!(
            reason(&checked_open(&junction.join("r"), CAP).unwrap_err()),
            "link"
        );
        assert_eq!(
            reason(
                &RingReader::open_untrusted(&junction.join("r"), CAP)
                    .err()
                    .unwrap()
            ),
            "link"
        );
        let e = RingWriter::create(&junction.join("new"), spec(), CAP)
            .err()
            .unwrap();
        assert_eq!(reason(&e), "link");
        assert!(!real.join("new").exists());
    }

    #[test]
    fn a_junction_as_the_file_is_refused() {
        let (_t, root) = tmp();
        std::fs::create_dir(root.join("real")).unwrap();
        let j = root.join("j");
        assert!(mklink(&[
            "/J",
            j.to_str().unwrap(),
            root.join("real").to_str().unwrap()
        ]));
        assert_eq!(reason(&checked_open(&j, CAP).unwrap_err()), "link");
    }

    #[test]
    fn a_file_symlink_is_refused() {
        let (_t, root) = tmp();
        drop(ring(&root.join("real")));
        let link = root.join("link");
        // Needs Developer Mode or the symlink privilege; without either there is nothing to test.
        if std::os::windows::fs::symlink_file(root.join("real"), &link).is_err() {
            eprintln!("skipped: this account cannot create symlinks");
            return;
        }
        assert_eq!(reason(&checked_open(&link, CAP).unwrap_err()), "link");
        assert_eq!(
            reason(&RingReader::open_untrusted(&link, CAP).err().unwrap()),
            "link"
        );
    }

    #[test]
    fn a_device_path_is_refused() {
        let e = checked_open(Path::new(r"\\.\NUL"), CAP).unwrap_err();
        assert!(["notAbsolute", "notRegular"].contains(&reason(&e)), "{e:?}");
    }
}
