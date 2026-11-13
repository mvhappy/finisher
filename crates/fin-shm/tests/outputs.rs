//! Outputs and inputs under the rules of a worker that may be hostile or still alive (plan 6A
//! Task 11-12 review B1, B3 and L2): a copy commit never gives the destination the writer's
//! inode, outputs are created new below no link, and inputs are written below no link.
//! Most cases make symlinks or read inodes and are Unix-only; on Windows the helpers they share
//! go unused.
#![cfg_attr(not(unix), allow(unused_imports, dead_code))]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fin_core::CoreError;
use fin_shm::{
    commit_output, commit_output_copy, create_new_output, create_output, create_output_dir,
    write_input,
};

const CAP: u64 = 1 << 20;

fn tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(t.path()).unwrap();
    for d in ["out", "dest", "elsewhere"] {
        std::fs::create_dir(root.join(d)).unwrap();
    }
    (t, root)
}

fn reason(e: &CoreError) -> &str {
    assert_eq!(e.code, "SHM_REFUSED", "{e:?}");
    e.details["reason"].as_str().unwrap()
}

fn write(path: &Path, bytes: &[u8]) {
    File::create_new(path).unwrap().write_all(bytes).unwrap();
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[cfg(unix)]
fn ino(path: &Path) -> u64 {
    std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(path).unwrap())
}

/// B1: a warm worker is alive and may hold the file it wrote. A rename gives the destination
/// its inode (it can change the file later); a copy never does.
#[cfg(unix)]
#[test]
fn commit_while_a_writer_holds_a_handle() {
    let (_t, root) = tmp();
    let (from, to) = (root.join("out/a.png"), root.join("dest/a.png"));
    write(&from, b"pixels");
    let src_ino = ino(&from);
    // The worker's handle, still open after its job.
    let mut writer = OpenOptions::new().append(true).open(&from).unwrap();
    commit_output_copy(&from, &to, CAP).unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"pixels");
    assert_ne!(ino(&to), src_ino, "the destination is a new inode");
    assert!(!from.exists(), "the source is removed");
    assert_eq!(names(&root.join("dest")), ["a.png"], "no temporary left");
    // It writes on: the destination does not change.
    writer.write_all(b" and more").unwrap();
    writer.sync_all().unwrap();
    assert_eq!(std::fs::read(&to).unwrap(), b"pixels");

    // Contrast: a rename shares the inode, so the same late write reaches the destination.
    let (from2, to2) = (root.join("out/b.png"), root.join("dest/b.png"));
    write(&from2, b"pixels");
    let mut writer2 = OpenOptions::new().append(true).open(&from2).unwrap();
    commit_output(&from2, &to2, CAP).unwrap();
    writer2.write_all(b" and more").unwrap();
    assert_eq!(std::fs::read(&to2).unwrap(), b"pixels and more");
}

#[test]
fn a_copy_commit_checks_the_file_like_a_move() {
    let (_t, root) = tmp();
    let (from, to) = (root.join("out/big.png"), root.join("dest/big.png"));
    write(&from, &[0u8; 64]);
    write(&to, b"old");
    let e = commit_output_copy(&from, &to, 16).unwrap_err();
    assert_eq!(reason(&e), "overCap");
    assert_eq!(
        std::fs::read(&to).unwrap(),
        b"old",
        "a refusal leaves the destination alone"
    );
    #[cfg(unix)]
    {
        let link = root.join("out/link.png");
        std::os::unix::fs::symlink(root.join("elsewhere"), &link).unwrap();
        let e = commit_output_copy(&link, &root.join("dest/link.png"), CAP).unwrap_err();
        assert_eq!(reason(&e), "link");
    }
}

/// B3: an `out/` that is a link is refused, and nothing is written through it.
#[cfg(unix)]
#[test]
fn create_output_refuses_a_linked_out_dir() {
    let (_t, root) = tmp();
    let job = root.join("job");
    std::fs::create_dir(&job).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), job.join("out")).unwrap();
    for f in [create_output, create_new_output] {
        let e = f(&job.join("out/look-0.png")).unwrap_err();
        assert_eq!(reason(&e), "link");
    }
    assert!(names(&root.join("elsewhere")).is_empty());
}

/// B3: a file or link of the output's name (a retry's, or a worker's plant) is unlinked, never
/// followed; `create_new_output` (replay) refuses it.
#[cfg(unix)]
#[test]
fn create_output_replaces_what_is_there_without_following_it() {
    let (_t, root) = tmp();
    let victim = root.join("elsewhere/victim");
    write(&victim, b"keep me");
    let out = root.join("out/look-0.png");
    std::os::unix::fs::symlink(&victim, &out).unwrap();
    let e = create_new_output(&out).unwrap_err();
    assert_eq!(reason(&e), "exists");
    let mut f = create_output(&out).unwrap();
    f.write_all(b"png").unwrap();
    drop(f);
    assert_eq!(std::fs::read(&out).unwrap(), b"png");
    assert!(
        !std::fs::symlink_metadata(&out)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    // A dangling link, and a retry's earlier output.
    std::fs::remove_file(&out).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere/not-there"), &out).unwrap();
    drop(create_output(&out).unwrap());
    assert!(!root.join("elsewhere/not-there").exists());
    drop(create_output(&out).unwrap());
    assert_eq!(std::fs::metadata(&out).unwrap().len(), 0);
    // Hard link planted: replaced, the other name keeps its bytes.
    std::fs::remove_file(&out).unwrap();
    std::fs::hard_link(&victim, &out).unwrap();
    drop(create_output(&out).unwrap());
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
}

/// L2: inputs are written below no link, and a link planted at the final name is replaced.
#[cfg(unix)]
#[test]
fn write_input_refuses_a_linked_dir_and_replaces_a_linked_name() {
    let (_t, root) = tmp();
    let job = root.join("job");
    std::fs::create_dir(&job).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), job.join("in")).unwrap();
    let e = write_input(&job.join("in"), "snapshot.json", b"{}").unwrap_err();
    assert_eq!(reason(&e), "link");
    assert!(names(&root.join("elsewhere")).is_empty());

    let input = root.join("in");
    std::fs::create_dir(&input).unwrap();
    let victim = root.join("elsewhere/victim");
    write(&victim, b"keep me");
    std::os::unix::fs::symlink(&victim, input.join("snapshot.json")).unwrap();
    std::os::unix::fs::symlink(&victim, input.join("snapshot.json.tmp")).unwrap();
    write_input(&input, "snapshot.json", b"{\"a\":1}").unwrap();
    assert_eq!(
        std::fs::read(input.join("snapshot.json")).unwrap(),
        b"{\"a\":1}"
    );
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    assert_eq!(names(&input), ["snapshot.json"]);
}

/// 6B-1 Task 14: an output directory (a cache entry's `out/<key>/`) is made below no link, kept
/// when a retry made it already, and refused when a link or a file has its name.
#[cfg(unix)]
#[test]
fn create_output_dir_makes_or_keeps_a_real_directory_only() {
    use std::os::unix::fs::PermissionsExt;
    let (_t, root) = tmp();
    let dir = root.join("out/key");
    create_output_dir(&dir).unwrap();
    let meta = std::fs::symlink_metadata(&dir).unwrap();
    assert!(meta.is_dir());
    assert_eq!(meta.permissions().mode() & 0o777, 0o700);
    // A retried attempt's directory is kept, with what it holds.
    write(&dir.join("old"), b"x");
    create_output_dir(&dir).unwrap();
    assert!(dir.join("old").exists());
    // Files go in it through the checked create.
    create_output(&dir.join("meta.json")).unwrap();
    // A link of that name, to a directory or anywhere, is refused and never followed.
    let link = root.join("out/linked");
    std::os::unix::fs::symlink(root.join("elsewhere"), &link).unwrap();
    assert_eq!(reason(&create_output_dir(&link).unwrap_err()), "link");
    // A file of that name is not a directory.
    write(&root.join("out/file"), b"x");
    assert_eq!(
        reason(&create_output_dir(&root.join("out/file")).unwrap_err()),
        "notRegular"
    );
    // A linked parent is refused before anything is made.
    let job = root.join("job");
    std::fs::create_dir(&job).unwrap();
    std::os::unix::fs::symlink(root.join("elsewhere"), job.join("out")).unwrap();
    assert_eq!(
        reason(&create_output_dir(&job.join("out/key")).unwrap_err()),
        "link"
    );
    assert!(names(&root.join("elsewhere")).is_empty());
    // Relative paths and `..` are refused.
    assert_eq!(
        reason(&create_output_dir(Path::new("out/key")).unwrap_err()),
        "notAbsolute"
    );
}
