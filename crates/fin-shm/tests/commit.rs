//! `commit_output`: the move out of `out/` by rename or by a bounded copy, its refusals, and
//! what happens when the file is swapped, grown or truncated between the checks and the move
//! (the `test-hooks` `before_rename` callback).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_shm::{commit_output, hooks};

const CAP: u64 = 1 << 20;

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    std::fs::create_dir(root.join("out")).unwrap();
    std::fs::create_dir(root.join("dest")).unwrap();
    (t, root)
}

fn reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "SHM_REFUSED", "{e:?}");
    e.details["reason"].as_str().unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    File::create_new(path).unwrap().write_all(bytes).unwrap();
}

fn leftovers(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

/// Runs `f` with the copy path forced on this thread.
fn copying<R>(f: impl FnOnce() -> R) -> R {
    hooks::force_copy(true);
    let r = f();
    hooks::force_copy(false);
    r
}

#[test]
fn an_output_moves_by_rename_and_by_copy() {
    for copy in [false, true] {
        let (_t, root) = tmp();
        let (from, to) = (
            root.join("out").join("a.png"),
            root.join("dest").join("a.png"),
        );
        write(&from, b"pixels");
        let run = || commit_output(&from, &to, CAP);
        if copy { copying(run) } else { run() }.unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"pixels", "copy: {copy}");
        assert!(!from.exists(), "copy: {copy}");
        assert_eq!(leftovers(&root.join("dest")), ["a.png"], "copy: {copy}");
    }
}

#[test]
fn an_output_replaces_an_existing_destination() {
    for copy in [false, true] {
        let (_t, root) = tmp();
        let (from, to) = (root.join("out").join("a"), root.join("dest").join("a"));
        write(&from, b"new");
        write(&to, b"old");
        let run = || commit_output(&from, &to, CAP);
        if copy { copying(run) } else { run() }.unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"new");
    }
}

#[test]
fn a_refused_output_leaves_the_destination_alone() {
    let (_t, root) = tmp();
    let to = root.join("dest").join("a");
    let big = root.join("out").join("big");
    write(&big, &[1u8; 11]);
    assert_eq!(
        reason(&commit_output(&big, &to, 10).unwrap_err()),
        "overCap"
    );
    let linked = root.join("out").join("linked");
    write(&linked, b"x");
    std::fs::hard_link(&linked, root.join("elsewhere")).unwrap();
    assert_eq!(
        reason(&commit_output(&linked, &to, CAP).unwrap_err()),
        "hardLinked"
    );
    assert_eq!(
        reason(&commit_output(&root.join("out").join("none"), &to, CAP).unwrap_err()),
        "missing"
    );
    assert_eq!(
        reason(&commit_output(&big, Path::new("rel/a"), CAP).unwrap_err()),
        "notAbsolute"
    );
    assert!(!to.exists());
    assert!(big.exists() && linked.exists());
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::symlink;

    use super::*;

    /// Unix only: on Windows the checked handle shares no writes, so the hook could not
    /// change the file at all.
    #[test]
    fn a_file_that_grows_or_shrinks_during_the_copy_is_refused_without_leftovers() {
        let (_t, root) = tmp();
        let to = root.join("dest").join("a");
        let from = root.join("out").join("a");
        write(&from, &[5u8; 4096]);
        let p = from.clone();
        hooks::before_rename(move || {
            OpenOptions::new()
                .append(true)
                .open(&p)
                .unwrap()
                .write_all(b"more")
                .unwrap()
        });
        assert_eq!(
            reason(&copying(|| commit_output(&from, &to, CAP)).unwrap_err()),
            "changed"
        );
        let p = from.clone();
        hooks::before_rename(move || {
            OpenOptions::new()
                .write(true)
                .open(&p)
                .unwrap()
                .set_len(100)
                .unwrap()
        });
        assert_eq!(
            reason(&copying(|| commit_output(&from, &to, CAP)).unwrap_err()),
            "truncated"
        );
        assert!(
            leftovers(&root.join("dest")).is_empty(),
            "{:?}",
            leftovers(&root.join("dest"))
        );
    }

    #[test]
    fn a_symlinked_output_or_out_directory_is_refused() {
        let (_t, root) = tmp();
        let to = root.join("dest").join("a");
        write(&root.join("secret"), b"secret");
        symlink(root.join("secret"), root.join("out").join("a")).unwrap();
        assert_eq!(
            reason(&commit_output(&root.join("out").join("a"), &to, CAP).unwrap_err()),
            "link"
        );
        symlink(root.join("out"), root.join("out2")).unwrap();
        write(&root.join("out").join("b"), b"b");
        assert_eq!(
            reason(&commit_output(&root.join("out2").join("b"), &to, CAP).unwrap_err()),
            "link"
        );
        assert!(!to.exists());
        assert_eq!(std::fs::read(root.join("secret")).unwrap(), b"secret");
    }

    #[test]
    fn a_file_swapped_for_a_link_after_its_checks_never_lands() {
        let (_t, root) = tmp();
        let (from, to) = (root.join("out").join("a"), root.join("dest").join("a"));
        write(&from, b"checked");
        write(&root.join("secret"), b"secret");
        let (p, secret) = (from.clone(), root.join("secret"));
        hooks::before_rename(move || {
            std::fs::remove_file(&p).unwrap();
            symlink(&secret, &p).unwrap();
        });
        assert_eq!(
            reason(&commit_output(&from, &to, CAP).unwrap_err()),
            "changed"
        );
        assert!(
            std::fs::symlink_metadata(&to).is_err(),
            "the swapped-in link was removed"
        );
        assert_eq!(std::fs::read(root.join("secret")).unwrap(), b"secret");
    }

    #[test]
    fn a_file_hard_linked_after_its_checks_is_removed_from_the_destination() {
        let (_t, root) = tmp();
        let (from, to) = (root.join("out").join("a"), root.join("dest").join("a"));
        write(&from, b"checked");
        let (p, other) = (from.clone(), root.join("other"));
        hooks::before_rename(move || std::fs::hard_link(&p, &other).unwrap());
        assert_eq!(
            reason(&commit_output(&from, &to, CAP).unwrap_err()),
            "hardLinked"
        );
        assert!(!to.exists());
    }

    #[test]
    fn a_copy_reads_the_checked_file_even_if_it_is_swapped() {
        let (_t, root) = tmp();
        let (from, to) = (root.join("out").join("a"), root.join("dest").join("a"));
        write(&from, b"checked");
        let p = from.clone();
        hooks::before_rename(move || {
            std::fs::remove_file(&p).unwrap();
            write(&p, b"swapped");
        });
        copying(|| commit_output(&from, &to, CAP)).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"checked");
        assert_eq!(
            std::fs::read(&from).unwrap(),
            b"swapped",
            "only the checked file is removed"
        );
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    #[test]
    fn an_output_another_handle_can_still_write_is_busy() {
        let (_t, root) = tmp();
        let (from, to) = (root.join("out").join("a"), root.join("dest").join("a"));
        write(&from, b"x");
        let writer = OpenOptions::new().write(true).open(&from).unwrap();
        assert_eq!(reason(&commit_output(&from, &to, CAP).unwrap_err()), "busy");
        drop(writer);
        commit_output(&from, &to, CAP).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"x");
    }
}
