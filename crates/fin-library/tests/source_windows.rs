//! The Windows source (plan 5D Task 3, Decisions 7, 8, 10, 15, 17): listings from directory
//! handles, the fallback chain, reparse points, names Win32 would rewrite, long paths, and
//! volume identity.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(windows)]

mod win_support;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fin_library::reparse::IO_REPARSE_TAG_MOUNT_POINT;
use fin_library::source::NativeSource;
use fin_library::{EntryType, RawEntry, RootState, Source, open_for_read, root_state};
use win_support::{expect_of, junction, read, rel, run, scan, skip, tempdir, verbatim};

fn list(root: &Path, dir: &str) -> Vec<RawEntry> {
    let src = NativeSource::new(root);
    let mut v = src.read_dir(&src.open_dir(&rel(dir)).unwrap()).unwrap();
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn find<'a>(v: &'a [RawEntry], name: &str) -> &'a RawEntry {
    v.iter()
        .find(|e| e.name == name.as_bytes())
        .unwrap_or_else(|| panic!("{name} not listed"))
}

#[test]
fn extd_dir_info_fields() {
    let t = tempdir();
    fs::write(t.path().join("a.mov"), vec![7u8; 1234]).unwrap();
    // 2021-05-06T07:08:09.123456700Z: 100 ns precision survives.
    let when = SystemTime::UNIX_EPOCH + Duration::new(1_620_284_889, 123_456_700);
    fs::File::options()
        .write(true)
        .open(t.path().join("a.mov"))
        .unwrap()
        .set_modified(when)
        .unwrap();
    fs::hard_link(t.path().join("a.mov"), t.path().join("link.mov")).unwrap();
    fs::create_dir(t.path().join("d")).unwrap();
    let v = list(t.path(), "");
    let a = find(&v, "a.mov");
    assert_eq!(a.entry_type, EntryType::File);
    assert_eq!(a.size, 1234);
    assert_eq!(a.mtime_ns, 1_620_284_889_123_456_700);
    let id = a.file_id.clone().expect("a 128-bit id");
    assert_eq!(id.len(), 16);
    assert_eq!(
        find(&v, "link.mov").file_id,
        a.file_id,
        "a hard link shares the id"
    );
    assert_ne!(find(&v, "d").file_id, a.file_id);
    assert_eq!(find(&v, "d").entry_type, EntryType::Dir);
    let dev = a.dev.clone().unwrap();
    assert_eq!(dev.len(), 16, "the volume serial as 16 hex digits: {dev}");
    assert_eq!(list(t.path(), ""), v, "a listing is stable");
}

#[test]
fn enumeration_fallback_chain() {
    let t = tempdir();
    for d in ["x", "y"] {
        fs::create_dir(t.path().join(d)).unwrap();
        fs::write(t.path().join(d).join("f.mov"), d).unwrap();
    }
    let walk = |refused: u8| {
        let src = Arc::new(NativeSource::new(t.path()).refuse_classes(refused));
        let entries: Vec<_> = scan(Arc::clone(&src) as Arc<dyn Source>)
            .into_iter()
            .flat_map(|b| b.entries)
            .collect();
        let stat = src.stat_root().unwrap();
        (entries, src.enumeration(), stat.enumeration)
    };
    let (extd, name, recorded) = walk(0);
    assert_eq!(
        (name, recorded.as_str()),
        ("FileIdExtdDirectoryInfo", "FileIdExtdDirectoryInfo")
    );
    let (both, name, recorded) = walk(1);
    assert_eq!(
        (name, recorded.as_str()),
        ("FileIdBothDirectoryInfo", "FileIdBothDirectoryInfo")
    );
    // On NTFS the 128-bit id is the 64-bit one, zero-extended: both classes agree.
    assert_eq!(both, extd);
    let (full, name, recorded) = walk(2);
    assert_eq!(
        (name, recorded.as_str()),
        ("FileFullDirectoryInfo", "FileFullDirectoryInfo")
    );
    assert_eq!(full.len(), extd.len());
    for (f, e) in full.iter().zip(&extd) {
        assert_eq!(f.file_id, None, "no ids in the last class");
        assert_eq!((&f.name, f.size, f.mtime_ns), (&e.name, e.size, e.mtime_ns));
    }
}

#[test]
fn junctions_and_symlinks_skipped() {
    let outside = tempdir();
    fs::write(outside.path().join("secret.mov"), b"outside").unwrap();
    let t = tempdir();
    fs::create_dir(t.path().join("real")).unwrap();
    fs::write(t.path().join("real/f.mov"), b"inside").unwrap();
    junction(&t.path().join("jin"), &t.path().join("real"));
    junction(&t.path().join("jout"), outside.path());
    let file_link = std::os::windows::fs::symlink_file(
        outside.path().join("secret.mov"),
        t.path().join("flink.mov"),
    );
    if file_link.is_err() {
        skip(
            "junctions_and_symlinks_skipped",
            "file symlinks need Developer Mode or SeCreateSymbolicLinkPrivilege; junctions only",
        );
    }
    let v = list(t.path(), "");
    for j in ["jin", "jout"] {
        let e = find(&v, j);
        assert_eq!(e.entry_type, EntryType::Symlink, "{j}");
        assert_eq!(e.reparse_tag, Some(IO_REPARSE_TAG_MOUNT_POINT), "{j}");
    }
    if file_link.is_ok() {
        assert_eq!(find(&v, "flink.mov").entry_type, EntryType::Symlink);
    }
    let batches = scan(Arc::new(NativeSource::new(t.path())));
    let listed: Vec<_> = batches
        .iter()
        .flat_map(|b| b.dirs_done.iter().map(|d| d.dir.display()))
        .collect();
    assert_eq!(listed, ["", "real"], "only the real directory is descended");
    let names: Vec<_> = batches
        .iter()
        .flat_map(|b| b.entries.iter().map(|e| e.name_nfc.clone()))
        .collect();
    assert!(!names.contains(&"secret.mov".to_owned()), "{names:?}");
    // Targeted through the walker's open, a junction is refused, never followed.
    let src = NativeSource::new(t.path());
    assert!(src.open_dir(&rel("jout")).is_err());
}

#[test]
fn long_paths_with_verbatim_prefix() {
    let t = tempdir();
    let comp = "d".repeat(120);
    let mut p = verbatim(t.path());
    let mut r = Vec::new();
    for _ in 0..4 {
        p.push(&comp);
        r.push(comp.clone());
    }
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("deep.mov"), b"deep").unwrap();
    r.push("deep.mov".into());
    assert!(p.as_os_str().len() > 480, "past MAX_PATH");
    let batches = scan(Arc::new(NativeSource::new(t.path())));
    let found = batches
        .iter()
        .flat_map(|b| &b.entries)
        .any(|e| e.name == b"deep.mov");
    assert!(found, "the deep file is listed");
    let rr = rel(&r.join("/"));
    let f = open_for_read(t.path(), &rr, &expect_of(t.path(), &rr), false).unwrap();
    assert_eq!(read(f), b"deep");
}

/// Names Win32 path parsing would rewrite or route to a device are listed and opened as
/// themselves (the components never pass through it).
#[test]
fn reserved_names_and_trailing_dots() {
    let t = tempdir();
    let names = ["CON", "nul.txt", "trail.", "space ", "AUX.mov"];
    for n in names {
        fs::write(verbatim(t.path()).join(n), n).unwrap();
    }
    let v = list(t.path(), "");
    for n in names {
        let e = find(&v, n);
        assert_eq!(e.entry_type, EntryType::File, "{n}");
        let rr = rel(n);
        let f = open_for_read(t.path(), &rr, &expect_of(t.path(), &rr), false).unwrap();
        assert_eq!(read(f), n.as_bytes(), "{n}");
    }
}

/// The 8.3 short name of `path`, if the volume made one.
fn short_name(path: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut buf = vec![0u16; 1024];
    // SAFETY: valid NUL-terminated input and an output buffer of its length.
    let n = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetShortPathNameW(
            w.as_ptr(),
            buf.as_mut_ptr(),
            buf.len() as u32,
        )
    } as usize;
    let s = String::from_utf16_lossy(&buf[..n.min(buf.len())]);
    let last = s.rsplit('\\').next()?.to_owned();
    (n > 0 && last != path.file_name()?.to_str()?).then_some(last)
}

/// An 8.3 short name never stands for its long name: not in a listing, not in a lookup, not
/// in an open; nor does another case of a name. The system temp directory is used: 8.3 names
/// are usually on for the system volume only.
#[test]
fn short_names_and_case_never_match() {
    let t = tempfile::tempdir().unwrap();
    let long = "longfilename_abcdef.mov";
    fs::write(t.path().join(long), b"x").unwrap();
    let short = short_name(&t.path().join(long));
    match &short {
        Some(s) => eprintln!("STEP0 the test file has the short name {s}"),
        None => skip(
            "short_names_and_case_never_match",
            "8.3 names are off on the temp volume; only the case part is decisive",
        ),
    }
    let src = NativeSource::new(t.path());
    let root = src.open_dir(&rel("")).unwrap();
    let listed: Vec<_> = src
        .read_dir(&root)
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(listed, [long.as_bytes().to_vec()]);
    let exp = expect_of(t.path(), &rel(long));
    let mut others = vec![long.to_uppercase()];
    others.extend(short.clone());
    for other in &others {
        assert_eq!(
            src.stat_child(&root, other.as_bytes()).unwrap(),
            None,
            "{other}"
        );
        let got = open_for_read(t.path(), &rel(other), &exp, false);
        assert!(
            matches!(got, Err(fin_library::OpenError::NotFound)),
            "{other}: {got:?}"
        );
    }
    assert!(src.stat_child(&root, long.as_bytes()).unwrap().is_some());
    assert!(
        !src.stat_root().unwrap().case_sensitive,
        "NTFS directories are case-insensitive by default"
    );
}

#[test]
fn stat_child_and_dir_mtime() {
    let t = tempdir();
    fs::create_dir(t.path().join("d")).unwrap();
    fs::write(t.path().join("f.mov"), b"x").unwrap();
    let src = NativeSource::new(t.path());
    let root = src.open_dir(&rel("")).unwrap();
    let listed = list(t.path(), "");
    for e in &listed {
        assert_eq!(src.stat_child(&root, &e.name).unwrap().as_ref(), Some(e));
    }
    assert_eq!(src.stat_child(&root, b"nope").unwrap(), None);
    // Wildcard characters are listed, not matched as a pattern.
    assert_eq!(src.stat_child(&root, b"*").unwrap(), None);
    let before = src.dir_mtime(&root).unwrap().unwrap();
    std::thread::sleep(Duration::from_millis(30));
    fs::write(t.path().join("new.mov"), b"y").unwrap();
    let after = src
        .dir_mtime(&src.open_dir(&rel("")).unwrap())
        .unwrap()
        .unwrap();
    assert!(
        after > before,
        "adding an entry moves the directory's mtime"
    );
}

#[test]
fn volume_serial_unc_and_case_flag() {
    let t = tempdir();
    fs::write(t.path().join("f.mov"), b"x").unwrap();
    let id = fin_library::identity(t.path()).unwrap();
    assert_eq!(id.kind, "NTFS");
    assert_eq!(id.dev_id.len(), 16);
    assert!(!id.remote && !id.case_sensitive && !id.mount_root);
    assert_eq!(id.enumeration, "FileIdExtdDirectoryInfo");
    assert_eq!(id.sample.len(), 1);
    assert!(id.root_file_id.is_some());
    assert_eq!(
        fin_library::identity(t.path()).unwrap().key,
        id.key,
        "the key is stable"
    );
    let drive = Path::new(&t.path().to_str().unwrap()[..3]).to_path_buf();
    let vol = fin_library::identity(&drive).unwrap();
    assert!(vol.mount_root, "a drive root is the root of its volume");
    assert_eq!(vol.key, id.key, "one volume, one key");
    match std::env::var("FIN_TEST_SHARE") {
        Ok(share) => {
            let s = fin_library::identity(Path::new(&share)).unwrap();
            assert!(s.remote, "a share is remote");
            assert_ne!(s.key, id.key);
            assert_eq!(fin_library::identity(Path::new(&share)).unwrap().key, s.key);
        }
        Err(_) => skip(
            "volume_serial_unc_and_case_flag",
            "FIN_TEST_SHARE is not set (UNC part)",
        ),
    }
}

/// A free drive letter for `subst`.
fn free_letter() -> Option<char> {
    ('M'..='Y')
        .rev()
        .find(|c| !Path::new(&format!("{c}:\\")).exists())
}

/// `subst` drives: removed is unreachable; re-added, the identity is the same (the letter is
/// not part of it), and the root is the same directory as through its own path.
#[test]
fn subst_drive_removed_is_unreachable_and_identity_stable_across_remount() {
    let Some(letter) = free_letter() else {
        skip("subst", "no free drive letter");
        return;
    };
    let t = tempdir();
    fs::write(t.path().join("f.mov"), b"x").unwrap();
    fs::create_dir(t.path().join("d")).unwrap();
    let drive = format!("{letter}:");
    let root = format!("{letter}:\\");
    let target = t.path().to_str().unwrap().to_owned();
    if run("subst", &[&drive, &target]).is_none() {
        skip("subst", "subst is not available");
        return;
    }
    let stored = fin_library::identity(Path::new(&root)).unwrap();
    assert_eq!(stored.key, fin_library::identity(t.path()).unwrap().key);
    let timeout = Duration::from_secs(10);
    assert!(matches!(
        root_state(Path::new(&root), &stored, timeout),
        RootState::Online(_)
    ));
    run("subst", &[&drive, "/D"]).expect("subst /D");
    let gone = root_state(Path::new(&root), &stored, timeout);
    run("subst", &[&drive, &target]).expect("subst again");
    let back = root_state(Path::new(&root), &stored, timeout);
    run("subst", &[&drive, "/D"]);
    eprintln!("STEP0 subst removed: {gone:?}");
    assert!(matches!(gone, RootState::Unreachable { .. }), "{gone:?}");
    match back {
        RootState::Online(now) => {
            assert_eq!(now.key, stored.key);
            assert_eq!(now.root_file_id, stored.root_file_id);
            assert_eq!(now.sample, stored.sample);
        }
        other => panic!("re-added: {other:?}"),
    }
}

/// WOF-compressed files are read through the filter (Decision 19's open has no
/// `FILE_OPEN_REPARSE_POINT`). Measured: on NTFS the WOF filter hides its tag from
/// enumeration, so the file lists as a plain file with its logical size.
#[test]
fn wof_file_is_readable() {
    let t = tempdir();
    let data: Vec<u8> = (0..200_000u32)
        .flat_map(|i| (i % 251).to_le_bytes())
        .collect();
    let p = t.path().join("w.mov");
    fs::write(&p, &data).unwrap();
    let Some(out) = run("compact", &["/c", "/exe:xpress4k", p.to_str().unwrap()]) else {
        skip("wof_file_is_readable", "compact /exe is not available");
        return;
    };
    assert!(out.contains("[OK]"), "{out}");
    let e = find(&list(t.path(), ""), "w.mov").clone();
    eprintln!(
        "STEP0 WOF file: {:?} tag={:?}",
        e.attrs,
        e.reparse_tag.map(|t| format!("{t:#x}"))
    );
    assert_eq!(e.entry_type, EntryType::File);
    assert!(e.placeholder_free());
    assert_eq!(e.size, data.len() as u64, "the logical size");
    let rr = rel("w.mov");
    let f = open_for_read(t.path(), &rr, &expect_of(t.path(), &rr), false).unwrap();
    assert_eq!(read(f), data);
}

trait PlaceholderFree {
    fn placeholder_free(&self) -> bool;
}

impl PlaceholderFree for RawEntry {
    fn placeholder_free(&self) -> bool {
        fin_library::placeholder::from_attrs(self.attrs).is_none()
    }
}
