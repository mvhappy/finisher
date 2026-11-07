//! `getattrlistbulk` against the real thing (plan 5D Task 2 on macOS): every record field the
//! parser reads equals `lstat` across several buffers, so the record layout (the
//! `ATTR_CMN_ERROR` slot after `returned`, the name's `attrreference_t`, the per-type tails)
//! holds at runtime; listed facts open through `open_for_read`; NFD and case behave as APFS
//! and HFS+ store them.
#![cfg(target_os = "macos")]

mod support;

use std::cell::Cell;
use std::ffi::OsStr;
use std::fs;
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::Path;

use fin_library::source::NativeSource;
use fin_library::{
    Collision, EntryType, Excludes, Expect, OpenError, RawEntry, RawRel, Source, collisions,
    identity, listing, open_for_read,
};
use support::image::image;

const NFD: &[u8] = b"cafe\xcc\x81.mov";
const NFC: &[u8] = "café.mov".as_bytes();

fn list_with(p: &Path, progress: &dyn Fn()) -> Vec<RawEntry> {
    let src = NativeSource::new(p);
    let d = src.open_dir(&RawRel::root()).unwrap();
    src.read_dir_with(&d, progress).unwrap()
}

fn list(p: &Path) -> Vec<RawEntry> {
    list_with(p, &|| {})
}

fn expect(e: &RawEntry) -> Expect {
    Expect {
        size: e.size,
        mtime_ns: e.mtime_ns,
        file_id: e.file_id.clone(),
    }
}

fn open(root: &Path, e: &RawEntry) -> Result<fs::File, OpenError> {
    open_for_read(root, &RawRel(vec![e.name.clone()]), &expect(e), false)
}

fn read(f: fs::File) -> Vec<u8> {
    use std::io::Read;
    let mut v = Vec::new();
    (&f).read_to_end(&mut v).unwrap();
    v
}

#[test]
fn bulk_records_match_lstat_across_buffers() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    // ~290-byte records: 3,000 of them span several 256 KiB buffers.
    let long: Vec<String> = (0..3000)
        .map(|i| format!("{i:04}-{}", "x".repeat(200)))
        .collect();
    for n in &long {
        fs::write(p.join(n), n.as_bytes()).unwrap();
    }
    fs::write(p.join("empty"), b"").unwrap();
    let sparse = fs::File::create(p.join("sparse.mov")).unwrap();
    sparse.set_len((5 << 30) + 3).unwrap();
    drop(sparse);
    fs::write(p.join("hard1"), b"linked").unwrap();
    fs::hard_link(p.join("hard1"), p.join("hard2")).unwrap();
    fs::write(p.join("hidden"), b"h").unwrap();
    let c = std::ffi::CString::new(p.join("hidden").as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid C string.
    assert_eq!(unsafe { libc::chflags(c.as_ptr(), libc::UF_HIDDEN) }, 0);
    fs::create_dir(p.join("dir")).unwrap();
    symlink("hard1", p.join("ln")).unwrap();
    let fifo = std::ffi::CString::new(p.join("fifo").as_os_str().as_bytes()).unwrap();
    // SAFETY: a valid C string.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    fs::write(p.join(OsStr::from_bytes(NFD)), b"nfd").unwrap();

    let buffers = Cell::new(0);
    let entries = list_with(p, &|| buffers.set(buffers.get() + 1));
    assert!(buffers.get() >= 3, "{} buffers", buffers.get());
    assert_eq!(entries.len(), long.len() + 9);
    let dev = fs::metadata(p).unwrap().dev();
    for e in &entries {
        let m = fs::symlink_metadata(p.join(OsStr::from_bytes(&e.name))).unwrap();
        let what = String::from_utf8_lossy(&e.name);
        let ft = m.file_type();
        let ty = if ft.is_symlink() {
            EntryType::Symlink
        } else if ft.is_dir() {
            EntryType::Dir
        } else if ft.is_file() {
            EntryType::File
        } else {
            EntryType::Other
        };
        assert_eq!(e.entry_type, ty, "{what}");
        if ty != EntryType::Dir {
            assert_eq!(e.size, m.len(), "{what}");
        }
        assert_eq!(
            e.mtime_ns,
            m.mtime() * 1_000_000_000 + m.mtime_nsec(),
            "{what}"
        );
        assert_eq!(
            e.file_id.as_deref(),
            Some(&m.ino().to_be_bytes()[..]),
            "{what}"
        );
        assert_eq!(e.dev.as_deref(), Some(dev.to_string().as_str()), "{what}");
        assert_eq!(
            e.attrs,
            fin_library::Attrs::Macos {
                flags: m.st_flags()
            },
            "{what}"
        );
        assert!(!e.mount_root && !e.automount, "{what}");
    }
    let by = |n: &[u8]| entries.iter().find(|e| e.name == n).unwrap();
    assert_eq!(by(b"sparse.mov").size, (5 << 30) + 3);
    assert_eq!(by(b"hard1").file_id, by(b"hard2").file_id);
    assert!(
        matches!(by(b"hidden").attrs, fin_library::Attrs::Macos { flags } if flags & libc::UF_HIDDEN != 0)
    );
    assert_eq!(by(b"fifo").entry_type, EntryType::Other);
    assert_eq!(by(NFD).name, NFD, "APFS lists a name as it was created");

    // What a listing records opens (the `Expect` check after open)…
    for n in [
        &b"hard1"[..],
        b"hard2",
        b"hidden",
        b"empty",
        b"sparse.mov",
        NFD,
    ] {
        open(p, by(n)).unwrap_or_else(|e| panic!("{}: {e:?}", String::from_utf8_lossy(n)));
    }
    assert_eq!(
        read(open(p, by(long[7].as_bytes())).unwrap()),
        long[7].as_bytes()
    );
    // …and anything else is refused before reading (the FIFO is never opened: no hang).
    assert!(matches!(open(p, by(b"fifo")), Err(OpenError::Changed)));
    assert!(matches!(open(p, by(b"ln")), Err(OpenError::NameSurrogate)));
    let mut stale = by(b"hard1").clone();
    stale.mtime_ns += 1;
    assert!(matches!(open(p, &stale), Err(OpenError::Changed)));
}

/// APFS keeps names as created and looks them up normalisation- and (by default)
/// case-insensitively: an NFC twin or a case twin is the same file, so one local listing never
/// holds both; keys and display names are NFC.
#[test]
fn apfs_names_nfd_and_case() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path();
    if identity(p).unwrap().case_sensitive {
        support::skip("apfs_names_nfd_and_case", "the temp dir is case-sensitive");
        return;
    }
    fs::write(p.join(OsStr::from_bytes(NFD)), b"1").unwrap();
    fs::write(p.join(OsStr::from_bytes(NFC)), b"2").unwrap();
    fs::write(p.join("Clip.MOV"), b"3").unwrap();
    fs::write(p.join("clip.mov"), b"4").unwrap();
    let raw = list(p);
    let names: Vec<&[u8]> = raw.iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    assert!(names.contains(&NFD) && names.contains(&&b"Clip.MOV"[..]));
    assert!(collisions(&names, false).is_empty());
    let entries = listing::entries(&RawRel::root(), raw.clone(), &Excludes::defaults(false));
    let nfd = entries.iter().find(|e| e.name == NFD).unwrap();
    assert_eq!((nfd.name_nfc.as_str(), nfd.name_lossy), ("café.mov", false));
    assert_eq!(nfd.rel_key, "café.mov");
    let clip = entries.iter().find(|e| e.name == b"Clip.MOV").unwrap();
    assert_eq!(clip.rel_key, "clip.mov");
    // The listed chain opens the file the second write replaced.
    let e = raw.iter().find(|e| e.name == NFD).unwrap();
    assert_eq!(read(open(p, e).unwrap()), b"2");
}

/// Case twins on case-sensitive APFS are two files with two keys; were the root judged
/// case-insensitive (an SMB share of a case-sensitive server), both keep their rows and get
/// the CASE bit.
#[test]
fn case_sensitive_apfs_twins() {
    let t = tempfile::tempdir().unwrap();
    let Some(img) = image("case_sensitive_apfs_twins", t.path(), "Case-sensitive APFS") else {
        return;
    };
    fs::write(img.mnt.join("Clip.mov"), b"upper").unwrap();
    fs::write(img.mnt.join("clip.mov"), b"lower").unwrap();
    assert!(identity(&img.mnt).unwrap().case_sensitive);
    let raw: Vec<RawEntry> = list(&img.mnt)
        .into_iter()
        .filter(|e| e.name.eq_ignore_ascii_case(b"clip.mov"))
        .collect();
    assert_eq!(raw.len(), 2);
    let names: Vec<&[u8]> = raw.iter().map(|e| e.name.as_slice()).collect();
    assert!(collisions(&names, true).is_empty());
    assert_eq!(
        collisions(&names, false),
        vec![
            (0, Collision(Collision::CASE)),
            (1, Collision(Collision::CASE))
        ]
    );
    for e in &raw {
        let want: &[u8] = if e.name == b"Clip.mov" {
            b"upper"
        } else {
            b"lower"
        };
        assert_eq!(read(open(&img.mnt, e).unwrap()), want);
    }
}

/// HFS+ stores names decomposed: an NFC name is listed as NFD bytes, displayed and keyed as
/// NFC, and opened by the listed (NFD) chain.
#[test]
fn hfs_plus_lists_nfd() {
    let t = tempfile::tempdir().unwrap();
    let Some(img) = image("hfs_plus_lists_nfd", t.path(), "HFS+") else {
        return;
    };
    fs::write(img.mnt.join(OsStr::from_bytes(NFC)), b"hfs").unwrap();
    let raw = list(&img.mnt);
    let e = raw
        .iter()
        .find(|e| e.name.starts_with(b"caf"))
        .expect("listed");
    assert_eq!(e.name, NFD, "HFS+ decomposes");
    let entries = listing::entries(&RawRel::root(), raw.clone(), &Excludes::defaults(false));
    let s = entries.iter().find(|s| s.name == NFD).unwrap();
    assert_eq!(
        (s.name_nfc.as_str(), s.rel_key.as_str()),
        ("café.mov", "café.mov")
    );
    assert_eq!(read(open(&img.mnt, e).unwrap()), b"hfs");
}
