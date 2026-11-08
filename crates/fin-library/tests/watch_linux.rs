//! The per-directory inotify watch of a local root (`fin_library::watch`; plan 5D Decision 30,
//! the Tasks 9–11 review): directories only, dot-directories and other mounts left out, new
//! directories watched, every failure reported, never a partial watch.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(target_os = "linux")]

mod support;

use std::fs;
use std::path::Path;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use fin_library::RawRel;
use fin_library::watch::{DirWatch, WatchMsg};
use support::{tmpfs, userns};

const WAIT: Duration = Duration::from_secs(10);

fn start(root: &Path, max: usize) -> (DirWatch, Receiver<WatchMsg>) {
    let (tx, rx) = channel();
    let w = DirWatch::start(
        root.to_path_buf(),
        max,
        Box::new(move |m| {
            let _ = tx.send(m);
        }),
    )
    .unwrap();
    (w, rx)
}

fn ready(rx: &Receiver<WatchMsg>) -> usize {
    match rx.recv_timeout(WAIT).unwrap() {
        WatchMsg::Ready { watches } => watches,
        other => panic!("expected ready, got {other:?}"),
    }
}

/// Every directory reported until `want` is among them (or the wait ends).
fn dirs_until(rx: &Receiver<WatchMsg>, want: &RawRel) -> Vec<RawRel> {
    let mut out = Vec::new();
    while !out.contains(want) {
        match rx.recv_timeout(WAIT) {
            Ok(WatchMsg::Dirs(d)) => out.extend(d),
            Ok(other) => panic!("expected dirs, got {other:?}"),
            Err(_) => panic!("{want} not reported; got {out:?}"),
        }
    }
    out
}

fn rel(parts: &[&str]) -> RawRel {
    RawRel(parts.iter().map(|p| p.as_bytes().to_vec()).collect())
}

/// Drains what arrives within `ms` (nothing expected).
fn quiet(rx: &Receiver<WatchMsg>, ms: u64) -> Vec<WatchMsg> {
    let mut out = Vec::new();
    while let Ok(m) = rx.recv_timeout(Duration::from_millis(ms)) {
        out.push(m);
    }
    out
}

#[test]
fn watches_directories_and_skips_dot_directories() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::create_dir_all(root.join(".cache/deep")).unwrap();
    for i in 0..20 {
        fs::write(root.join(format!("a/f{i}.mov")), b"x").unwrap();
    }
    let (w, rx) = start(root, 100);
    // The root, `a` and `a/b`: no file and no dot-directory has a watch of its own.
    assert_eq!(ready(&rx), 3);
    fs::write(root.join("a/b/new.mov"), b"x").unwrap();
    dirs_until(&rx, &rel(&["a", "b"]));
    // Dot-entries and anything in a dot-directory say nothing.
    fs::write(root.join(".hidden"), b"x").unwrap();
    fs::write(root.join(".cache/deep/x"), b"x").unwrap();
    assert_eq!(quiet(&rx, 300), vec![]);
    // A new directory is watched at once: a file written in it is reported by its directory.
    fs::create_dir(root.join("a/c")).unwrap();
    dirs_until(&rx, &rel(&["a"]));
    std::thread::sleep(Duration::from_millis(100));
    fs::write(root.join("a/c/x.mov"), b"x").unwrap();
    dirs_until(&rx, &rel(&["a", "c"]));
    w.close();
    fs::write(root.join("a/after.mov"), b"x").unwrap();
    assert_eq!(quiet(&rx, 300), vec![], "nothing after close");
}

#[test]
fn moved_directories_follow_their_new_path() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir_all(root.join("a/x")).unwrap();
    fs::create_dir_all(root.join("b")).unwrap();
    let (_w, rx) = start(root, 100);
    assert_eq!(ready(&rx), 4);
    fs::rename(root.join("a/x"), root.join("b/x")).unwrap();
    dirs_until(&rx, &rel(&["b"]));
    std::thread::sleep(Duration::from_millis(100));
    fs::write(root.join("b/x/f.mov"), b"x").unwrap();
    let got = dirs_until(&rx, &rel(&["b", "x"]));
    assert!(!got.contains(&rel(&["a", "x"])), "{got:?}");
}

#[test]
fn more_directories_than_allowed_fail_the_watch() {
    let t = tempfile::tempdir().unwrap();
    for d in ["a", "b", "c", "d"] {
        fs::create_dir(t.path().join(d)).unwrap();
    }
    let (_w, rx) = start(t.path(), 3);
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchMsg::Failed("WATCH_LIMIT".into())
    );
    // Growth past the limit later fails it too.
    let (_w, rx) = start(t.path(), 6);
    assert_eq!(ready(&rx), 5);
    fs::create_dir(t.path().join("e")).unwrap();
    fs::create_dir(t.path().join("f")).unwrap();
    let mut got = Vec::new();
    loop {
        match rx.recv_timeout(WAIT).unwrap() {
            WatchMsg::Failed(code) => break assert_eq!(code, "WATCH_LIMIT"),
            m => got.push(m),
        }
    }
}

#[test]
fn an_unreadable_directory_fails_the_watch() {
    if support::euid() == 0 {
        return support::skip(
            "an_unreadable_directory_fails_the_watch",
            "root reads every directory",
        );
    }
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let locked = t.path().join("locked");
    fs::create_dir_all(locked.join("inner")).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let (_w, rx) = start(t.path(), 100);
    let got = rx.recv_timeout(WAIT).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(got, WatchMsg::Failed("EACCES".into()));
}

#[test]
fn a_removed_root_fails_the_watch() {
    let t = tempfile::tempdir().unwrap();
    let root = t.path().join("root");
    fs::create_dir(&root).unwrap();
    let (_w, rx) = start(&root, 100);
    assert_eq!(ready(&rx), 1);
    fs::remove_dir(&root).unwrap();
    loop {
        match rx.recv_timeout(WAIT).unwrap() {
            WatchMsg::Failed(code) => break assert_eq!(code, "WATCH_ROOT_GONE"),
            WatchMsg::Dirs(_) => {}
            other => panic!("{other:?}"),
        }
    }
    let (_w, rx) = start(&root, 100);
    assert_eq!(
        rx.recv_timeout(WAIT).unwrap(),
        WatchMsg::Failed("WATCH_ROOT_GONE".into())
    );
}

#[test]
fn messages_as_json() {
    assert_eq!(WatchMsg::Ready { watches: 3 }.to_json(), r#"{"ready":3}"#);
    assert_eq!(
        WatchMsg::Dirs(vec![rel(&["a"])]).to_json(),
        r#"{"dirs":[{"components":["YQ=="]}]}"#
    );
    assert_eq!(WatchMsg::All.to_json(), r#"{"all":true}"#);
    assert_eq!(
        WatchMsg::Failed("ENOSPC".into()).to_json(),
        r#"{"failed":"ENOSPC"}"#
    );
}

#[test]
fn nested_mounts_are_not_watched() {
    if !userns("nested_mounts_are_not_watched") {
        return;
    }
    let t = tempfile::tempdir().unwrap();
    let root = t.path();
    fs::create_dir_all(root.join("m")).unwrap();
    fs::create_dir_all(root.join("plain")).unwrap();
    tmpfs(&root.join("m"));
    fs::create_dir_all(root.join("m/inside/deeper")).unwrap();
    let (w, rx) = start(root, 100);
    // The root and `plain`: nothing on the tmpfs.
    assert_eq!(ready(&rx), 2);
    fs::write(root.join("m/inside/x.mov"), b"x").unwrap();
    assert_eq!(quiet(&rx, 300), vec![], "nothing below the nested mount");
    w.close();
    support::umount(&root.join("m"));
}
