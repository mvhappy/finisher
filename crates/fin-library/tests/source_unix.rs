//! The native Unix source (plan 5D Task 2): Linux `getdents64` + `statx`, macOS
//! `getattrlistbulk`.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(unix)]

mod support;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

use fin_library::source::NativeSource;
use fin_library::{EntryType, Excludes, RawEntry, RawRel, Source, listing};
use support::{euid, rel, skip};

fn by_name<'a>(entries: &'a [RawEntry], name: &[u8]) -> &'a RawEntry {
    entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{} not listed", String::from_utf8_lossy(name)))
}

fn list(src: &NativeSource, rel: &RawRel) -> Vec<RawEntry> {
    let d = src.open_dir(rel).unwrap();
    src.read_dir(&d).unwrap()
}

fn mtime_ns(m: &fs::Metadata) -> i64 {
    m.mtime() * 1_000_000_000 + m.mtime_nsec()
}

#[test]
fn enumerates_without_opening_files() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("a.mov"), b"0123456789").unwrap();
    fs::write(t.path().join("b.txt"), b"abc").unwrap();
    fs::create_dir(t.path().join("d")).unwrap();
    fs::write(t.path().join("d/c.mov"), b"c").unwrap();
    for f in ["a.mov", "b.txt", "d/c.mov"] {
        fs::set_permissions(t.path().join(f), fs::Permissions::from_mode(0o000)).unwrap();
    }
    if euid() != 0 {
        // Mode 0000: any open of these files would fail, so a listing that reports them did
        // not open them.
        assert!(fs::File::open(t.path().join("a.mov")).is_err());
    }
    let src = NativeSource::new(t.path());
    let stat = src.stat_root().unwrap();
    assert_eq!(stat.entries, 3);
    let root = list(&src, &RawRel::root());
    assert_eq!(root.len(), 3);
    for (name, size) in [("a.mov", 10), ("b.txt", 3)] {
        let e = by_name(&root, name.as_bytes());
        let m = fs::symlink_metadata(t.path().join(name)).unwrap();
        assert_eq!(e.entry_type, EntryType::File);
        assert_eq!(e.size, size);
        assert_eq!(e.mtime_ns, mtime_ns(&m));
    }
    assert_eq!(by_name(&root, b"d").entry_type, EntryType::Dir);
    let sub = list(&src, &rel(&[b"d"]));
    assert_eq!(by_name(&sub, b"c.mov").size, 1);
    for f in ["a.mov", "b.txt", "d/c.mov"] {
        fs::set_permissions(t.path().join(f), fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn listing_matches_lstat() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    fs::write(p.join("f1.mov"), vec![7u8; 4097]).unwrap();
    fs::write(p.join("empty"), b"").unwrap();
    fs::create_dir(p.join("dir")).unwrap();
    symlink("f1.mov", p.join("ln")).unwrap();
    let names: Vec<String> = (0..300).map(|i| format!("many-{i:03}.wav")).collect();
    for n in &names {
        fs::write(p.join(n), n.as_bytes()).unwrap();
    }
    let src = NativeSource::new(p);
    let entries = list(&src, &RawRel::root());
    assert_eq!(entries.len(), 4 + names.len());
    for e in &entries {
        let name = std::str::from_utf8(&e.name).unwrap();
        let m = fs::symlink_metadata(p.join(name)).unwrap();
        let ty = if m.file_type().is_symlink() {
            EntryType::Symlink
        } else if m.is_dir() {
            EntryType::Dir
        } else {
            EntryType::File
        };
        assert_eq!(e.entry_type, ty, "{name}");
        if ty == EntryType::File {
            assert_eq!(e.size, m.len(), "{name}");
        }
        assert_eq!(e.mtime_ns, mtime_ns(&m), "{name}");
        assert_eq!(
            e.file_id.as_deref(),
            Some(&m.ino().to_be_bytes()[..]),
            "{name}"
        );
        assert!(!e.mount_root && !e.automount, "{name}");
    }
}

#[test]
fn symlinks_skipped_never_followed() {
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.mov"), b"s").unwrap();
    let t = tempfile::tempdir().unwrap();
    symlink(outside.path(), t.path().join("dirlink")).unwrap();
    symlink(
        outside.path().join("secret.mov"),
        t.path().join("filelink.mov"),
    )
    .unwrap();
    let src = NativeSource::new(t.path());
    let raw = list(&src, &RawRel::root());
    assert_eq!(by_name(&raw, b"dirlink").entry_type, EntryType::Symlink);
    assert_eq!(
        by_name(&raw, b"filelink.mov").entry_type,
        EntryType::Symlink
    );
    let entries = listing::entries(&RawRel::root(), raw, &Excludes::defaults(true));
    assert!(entries.iter().all(|e| e.entry_type == EntryType::Symlink));
    // A link is never opened as a directory, by chain or from its parent's handle.
    assert!(src.open_dir(&rel(&[b"dirlink"])).is_err());
    let root = src.open_dir(&RawRel::root()).unwrap();
    assert!(src.open_child(&root, b"dirlink").is_err());
}

#[test]
fn open_child_equals_open_dir() {
    let t = tempfile::tempdir().unwrap();
    fs::create_dir_all(t.path().join("a/b")).unwrap();
    fs::write(t.path().join("a/b/x.mov"), b"x").unwrap();
    let src = NativeSource::new(t.path());
    let root = src.open_dir(&RawRel::root()).unwrap();
    let a = src.open_child(&root, b"a").unwrap();
    let b = src.open_child(&a, b"b").unwrap();
    assert_eq!(b.rel(), &rel(&[b"a", b"b"]));
    assert_eq!(src.read_dir(&b).unwrap(), list(&src, &rel(&[b"a", b"b"])));
    assert!(src.open_child(&a, b"..").is_err(), "`..` is refused");
}

#[test]
fn automount_not_triggered() {
    // The rule itself (an automount entry is excluded and never opened) is the fake's
    // (`tests/fake.rs`); here: a real automount point on this host, if it has an idle one.
    #[cfg(target_os = "linux")]
    {
        use fin_library::volume::linux::mountinfo;
        let mounts = mountinfo().unwrap();
        let idle = mounts.iter().find(|m| {
            m.fstype == "autofs"
                && !mounts
                    .iter()
                    .any(|o| o.parent == m.id && o.mount_point == m.mount_point)
        });
        let Some(auto) = idle else {
            skip(
                "automount_not_triggered",
                "no idle autofs or systemd automount point on this host (the rule is tested on the fake source)",
            );
            return;
        };
        let point =
            std::path::Path::new(std::str::from_utf8(&auto.mount_point).unwrap()).to_path_buf();
        let parent = point.parent().unwrap();
        let name = point.file_name().unwrap().as_encoded_bytes();
        let src = NativeSource::new(parent);
        let raw = list(&src, &RawRel::root());
        let e = by_name(&raw, name);
        assert!(e.automount, "the automount point is reported as one");
        let entries = listing::entries(&RawRel::root(), raw.clone(), &Excludes::defaults(true));
        let se = entries.iter().find(|s| s.name == name).unwrap();
        assert_eq!(se.excluded.as_deref(), Some("AUTOMOUNT"));
        let after = mountinfo().unwrap();
        assert!(
            !after
                .iter()
                .any(|o| o.parent == auto.id && o.mount_point == auto.mount_point),
            "listing the parent mounted it"
        );
    }
    #[cfg(target_os = "macos")]
    skip(
        "automount_not_triggered",
        "no automount point is set up for tests on macOS (the rule is tested on the fake source)",
    );
}

#[test]
fn file_ids_and_mount_roots() {
    let t = tempfile::tempdir().unwrap();
    fs::write(t.path().join("f"), b"1").unwrap();
    let src = NativeSource::new(t.path());
    let e = list(&src, &RawRel::root()).remove(0);
    let m = fs::symlink_metadata(t.path().join("f")).unwrap();
    assert_eq!(e.file_id, Some(m.ino().to_be_bytes().to_vec()));
    assert!(e.dev.as_deref().is_some_and(|d| !d.is_empty()));
    assert!(!e.mount_root);
    let stat = src.stat_root().unwrap();
    assert!(!stat.mount_root(), "a temp directory is not a mount root");
    assert_eq!(
        stat.root_file_id,
        Some(fs::metadata(t.path()).unwrap().ino().to_be_bytes().to_vec())
    );

    // `/` holds mount roots on both OSes: Linux `/proc`, macOS `/dev` (devfs).
    let slash = NativeSource::new("/");
    assert!(
        slash.stat_root().unwrap().mount_root(),
        "`/` is a mount root"
    );
    let top = list(&slash, &RawRel::root());
    let nested: &[u8] = if cfg!(target_os = "linux") {
        b"proc"
    } else {
        b"dev"
    };
    assert!(by_name(&top, nested).mount_root);
}
