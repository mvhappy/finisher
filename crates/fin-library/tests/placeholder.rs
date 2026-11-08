//! Placeholder and reparse-tag rules (plan 5D Task 1, Decisions 8, 12).

use std::collections::HashSet;

use fin_library::exclude::code;
use fin_library::placeholder::{
    SF_DATALESS, Stub, classify, dir_from_attrs, from_attrs, shadowed, stub, win::*,
};
use fin_library::reparse::*;
use fin_library::{Attrs, EntryType, Excludes, Placeholder, RawEntry, RawRel, ScanEntry, listing};

fn entry(name: &str, t: EntryType, attrs: Attrs, tag: Option<u32>) -> RawEntry {
    let mut e = RawEntry::new(name, t, 10, 1);
    e.attrs = attrs;
    e.reparse_tag = tag;
    e
}

fn none() -> HashSet<&'static [u8]> {
    HashSet::new()
}

#[test]
fn classify_matrix() {
    use EntryType::{Dir, File};
    let mac = |flags| Attrs::Macos { flags };
    let w = |attributes| Attrs::Windows { attributes };
    const RP: u32 = FILE_ATTRIBUTE_REPARSE_POINT;
    type Case = (
        &'static str,
        EntryType,
        Attrs,
        Option<u32>,
        Option<Placeholder>,
    );
    let cases: Vec<Case> = vec![
        ("linux file", File, Attrs::None, None, None),
        (
            "dataless file",
            File,
            mac(SF_DATALESS),
            None,
            Some(Placeholder::Dataless),
        ),
        (
            "dataless dir",
            Dir,
            mac(SF_DATALESS),
            None,
            Some(Placeholder::Dataless),
        ),
        ("other mac flags", File, mac(0x8000 | 0x20), None, None),
        (
            "dataless among others",
            File,
            mac(SF_DATALESS | 0x8000),
            None,
            Some(Placeholder::Dataless),
        ),
        ("plain windows", File, w(0x20), None, None),
        (
            "recall on open",
            File,
            w(FILE_ATTRIBUTE_RECALL_ON_OPEN),
            None,
            Some(Placeholder::RecallOnOpen),
        ),
        (
            "recall on data",
            File,
            w(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
            None,
            Some(Placeholder::RecallOnData),
        ),
        (
            "offline",
            File,
            w(FILE_ATTRIBUTE_OFFLINE),
            None,
            Some(Placeholder::Offline),
        ),
        (
            "unpinned alone",
            File,
            w(FILE_ATTRIBUTE_UNPINNED),
            None,
            Some(Placeholder::Unpinned),
        ),
        (
            "pinned alone is hydrated",
            File,
            w(FILE_ATTRIBUTE_PINNED),
            None,
            None,
        ),
        (
            "pinned still recalling",
            File,
            w(FILE_ATTRIBUTE_PINNED | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
            None,
            Some(Placeholder::RecallOnData),
        ),
        (
            "open wins over data",
            File,
            w(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | FILE_ATTRIBUTE_RECALL_ON_OPEN
                | FILE_ATTRIBUTE_OFFLINE),
            None,
            Some(Placeholder::RecallOnOpen),
        ),
        (
            "data wins over offline",
            File,
            w(FILE_ATTRIBUTE_OFFLINE
                | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
                | FILE_ATTRIBUTE_UNPINNED),
            None,
            Some(Placeholder::RecallOnData),
        ),
        (
            "cloud tag, recall bit",
            File,
            w(RP | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
            Some(IO_REPARSE_TAG_CLOUD),
            Some(Placeholder::RecallOnData),
        ),
        (
            "cloud_3 tag, unpinned",
            File,
            w(RP | FILE_ATTRIBUTE_UNPINNED),
            Some(IO_REPARSE_TAG_CLOUD | 0x3000),
            Some(Placeholder::Unpinned),
        ),
        (
            "hydrated cloud file",
            File,
            w(RP | FILE_ATTRIBUTE_PINNED),
            Some(IO_REPARSE_TAG_CLOUD),
            None,
        ),
        ("dedup file", File, w(RP), Some(IO_REPARSE_TAG_DEDUP), None),
        (
            "wof file",
            File,
            w(RP | 0x800),
            Some(IO_REPARSE_TAG_WOF),
            None,
        ),
        (
            "dedup offline",
            File,
            w(RP | FILE_ATTRIBUTE_OFFLINE),
            Some(IO_REPARSE_TAG_DEDUP),
            Some(Placeholder::Offline),
        ),
        (
            "recall-on-open directory",
            Dir,
            w(FILE_ATTRIBUTE_DIRECTORY | RP | FILE_ATTRIBUTE_RECALL_ON_OPEN),
            Some(IO_REPARSE_TAG_CLOUD),
            Some(Placeholder::RecallOnOpen),
        ),
    ];
    for (what, t, attrs, tag, want) in cases {
        let e = entry("x.mov", t, attrs, tag);
        assert_eq!(classify(&e, &none()), want, "{what}");
        assert_eq!(from_attrs(attrs), want, "{what}");
    }

    // A directory is a placeholder only by a bit that says its listing is not local:
    // `UNPINNED` alone ("free up space") leaves it populated (measured on Cloud Files).
    let unpinned_dir = w(FILE_ATTRIBUTE_DIRECTORY | RP | FILE_ATTRIBUTE_UNPINNED);
    let e = entry("d", Dir, unpinned_dir, Some(IO_REPARSE_TAG_CLOUD));
    assert_eq!(classify(&e, &none()), None, "unpinned directory");
    assert_eq!(dir_from_attrs(unpinned_dir), None);
    assert_eq!(
        from_attrs(unpinned_dir),
        Some(Placeholder::Unpinned),
        "the file rule"
    );
    // Cloud Files' not-yet-populated directory: OFFLINE | RECALL_ON_DATA_ACCESS.
    let unpopulated = w(FILE_ATTRIBUTE_DIRECTORY
        | RP
        | FILE_ATTRIBUTE_OFFLINE
        | 0x2000
        | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS);
    let e = entry("d", Dir, unpopulated, Some(IO_REPARSE_TAG_CLOUD));
    assert_eq!(classify(&e, &none()), Some(Placeholder::RecallOnData));
}

#[test]
fn name_surrogates_only_are_skipped() {
    for t in [
        IO_REPARSE_TAG_SYMLINK,
        IO_REPARSE_TAG_MOUNT_POINT,
        IO_REPARSE_TAG_LX_SYMLINK,
        0x2000_0099,
    ] {
        assert!(is_name_surrogate(t), "{t:#x}");
    }
    for t in [
        IO_REPARSE_TAG_CLOUD,
        IO_REPARSE_TAG_CLOUD | 0xF000,
        IO_REPARSE_TAG_DEDUP,
        IO_REPARSE_TAG_WOF,
        IO_REPARSE_TAG_NFS,
        IO_REPARSE_TAG_AF_UNIX,
        IO_REPARSE_TAG_APPEXECLINK,
    ] {
        assert!(!is_name_surrogate(t), "{t:#x}");
    }
    assert!(is_cloud(IO_REPARSE_TAG_CLOUD) && is_cloud(0x9000_A01A));
    assert!(!is_cloud(IO_REPARSE_TAG_DEDUP) && !is_cloud(0x9000_001B));

    // In a listing a junction or symlink becomes `symlink`; data-bearing tags stay files.
    let dir_attrs = Attrs::Windows {
        attributes: FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let file_attrs = Attrs::Windows {
        attributes: FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let raw = vec![
        entry(
            "junction",
            EntryType::Dir,
            dir_attrs,
            Some(IO_REPARSE_TAG_MOUNT_POINT),
        ),
        entry(
            "link.mov",
            EntryType::File,
            file_attrs,
            Some(IO_REPARSE_TAG_SYMLINK),
        ),
        entry(
            "wof.mov",
            EntryType::File,
            file_attrs,
            Some(IO_REPARSE_TAG_WOF),
        ),
        entry(
            "dedup.mov",
            EntryType::File,
            file_attrs,
            Some(IO_REPARSE_TAG_DEDUP),
        ),
        entry(
            "cloud.mov",
            EntryType::File,
            file_attrs,
            Some(IO_REPARSE_TAG_CLOUD),
        ),
        entry(
            "cloud-dir",
            EntryType::Dir,
            dir_attrs,
            Some(IO_REPARSE_TAG_CLOUD),
        ),
    ];
    let got: Vec<(String, EntryType, Option<u32>)> =
        listing::entries(&RawRel::root(), raw, &Excludes::defaults(false))
            .into_iter()
            .map(|e| (e.name_nfc, e.entry_type, e.reparse_tag))
            .collect();
    assert_eq!(
        got,
        vec![
            (
                "cloud-dir".into(),
                EntryType::Dir,
                Some(IO_REPARSE_TAG_CLOUD)
            ),
            (
                "cloud.mov".into(),
                EntryType::File,
                Some(IO_REPARSE_TAG_CLOUD)
            ),
            (
                "dedup.mov".into(),
                EntryType::File,
                Some(IO_REPARSE_TAG_DEDUP)
            ),
            (
                "junction".into(),
                EntryType::Symlink,
                Some(IO_REPARSE_TAG_MOUNT_POINT)
            ),
            (
                "link.mov".into(),
                EntryType::Symlink,
                Some(IO_REPARSE_TAG_SYMLINK)
            ),
            ("wof.mov".into(), EntryType::File, Some(IO_REPARSE_TAG_WOF)),
        ]
    );
}

#[test]
fn nextcloud_owncloud_icloud_stub_names() {
    let s = |n: &[u8]| stub(n);
    assert_eq!(
        s(b".clip.mov.icloud"),
        Some(Stub {
            target: b"clip.mov".to_vec(),
            kind: Placeholder::IcloudStub
        })
    );
    assert_eq!(s(b"clip.mov.nextcloud").unwrap().target, b"clip.mov");
    assert_eq!(
        s(b"clip.mov.nextcloud").unwrap().kind,
        Placeholder::Nextcloud
    );
    assert_eq!(s(b"clip.mov.owncloud").unwrap().kind, Placeholder::Owncloud);
    assert_eq!(s(b"\xffraw.owncloud").unwrap().target, b"\xffraw");
    // The stub of a dot-file stands for that dot-file.
    assert_eq!(s(b"..hidden.icloud").unwrap().target, b".hidden");
    for not in [
        &b"clip.mov"[..],
        b".icloud",
        b"..icloud",
        b"...icloud",
        b".nextcloud",
        b"..nextcloud",
        b"clip.icloud",
        b"clip.NEXTCLOUD",
        b"clip.nextcloud.mov",
    ] {
        assert_eq!(s(not), None, "{}", String::from_utf8_lossy(not));
    }
    // Directories are never stubs.
    let d = entry("x.nextcloud", EntryType::Dir, Attrs::None, None);
    assert_eq!(classify(&d, &none()), None);
    assert!(!shadowed(&d, &HashSet::from([&b"x"[..]])));
}

fn by_name(entries: &[ScanEntry], name: &[u8]) -> ScanEntry {
    entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{}", String::from_utf8_lossy(name)))
        .clone()
}

#[test]
fn stub_and_real_file_real_wins() {
    let f = |n: &str| entry(n, EntryType::File, Attrs::None, None);
    let siblings: HashSet<&[u8]> = HashSet::from([&b"a.mov"[..], b"a.mov.nextcloud"]);
    assert_eq!(classify(&f("a.mov.nextcloud"), &siblings), None);
    assert!(shadowed(&f("a.mov.nextcloud"), &siblings));
    assert_eq!(
        classify(&f("b.mov.nextcloud"), &siblings),
        Some(Placeholder::Nextcloud)
    );
    assert!(!shadowed(&f("b.mov.nextcloud"), &siblings));
    // An attribute placeholder wins over the name and is never shadowed.
    let mut dl = f("a.mov.nextcloud");
    dl.attrs = Attrs::Macos { flags: SF_DATALESS };
    assert_eq!(classify(&dl, &siblings), Some(Placeholder::Dataless));
    assert!(!shadowed(&dl, &siblings));

    let raw = vec![
        f("a.mov.nextcloud"),
        f("a.mov"),
        f(".b.mov.icloud"),
        f("c.mov.owncloud"),
        f("d.mov.owncloud"),
        f("d.mov.nextcloud"),
        dl.clone().tap_name("e.mov.nextcloud"),
    ];
    let out = listing::entries(
        &RawRel::root().join(b"Cloud"),
        raw,
        &Excludes::defaults(true),
    );
    assert_eq!(out.len(), 7);
    let a = by_name(&out, b"a.mov");
    assert_eq!((a.placeholder, a.excluded.as_deref()), (None, None));
    let a_stub = by_name(&out, b"a.mov.nextcloud");
    assert_eq!(
        (a_stub.placeholder, a_stub.excluded.as_deref()),
        (None, Some(code::STUB_SHADOWED))
    );
    // A stub is reported as the file it stands for (never a dot-file exclusion for iCloud).
    let b = by_name(&out, b"b.mov");
    assert_eq!(
        (b.placeholder, b.excluded.as_deref()),
        (Some(Placeholder::IcloudStub), None)
    );
    assert_eq!(b.rel_key, "Cloud/b.mov");
    assert_eq!(
        by_name(&out, b"c.mov").placeholder,
        Some(Placeholder::Owncloud)
    );
    // Two stubs for one name: the first in raw order claims it, the other is shadowed.
    let d = by_name(&out, b"d.mov");
    assert_eq!(d.placeholder, Some(Placeholder::Nextcloud));
    let d2 = by_name(&out, b"d.mov.owncloud");
    assert_eq!(d2.excluded.as_deref(), Some(code::STUB_SHADOWED));
    // A dataless stub-named file keeps its own name.
    assert_eq!(
        by_name(&out, b"e.mov.nextcloud").placeholder,
        Some(Placeholder::Dataless)
    );
    // Names stay unique per listing, so the catalog's (dirId, name) never conflicts.
    let names: HashSet<&[u8]> = out.iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(names.len(), out.len());
}

trait TapName {
    fn tap_name(self, n: &str) -> Self;
}

impl TapName for RawEntry {
    fn tap_name(mut self, n: &str) -> Self {
        self.name = n.as_bytes().to_vec();
        self
    }
}

#[test]
fn listing_is_sorted_by_reported_name() {
    // On disk `.x.icloud` sorts first; it is reported as `x`, between `a` and `y`.
    let raw = vec![
        entry("y", EntryType::File, Attrs::None, None),
        entry(".x.icloud", EntryType::File, Attrs::None, None),
        entry("a", EntryType::File, Attrs::None, None),
    ];
    let out = listing::entries(&RawRel::root(), raw, &Excludes::defaults(true));
    let names: Vec<&[u8]> = out.iter().map(|e| e.name.as_slice()).collect();
    assert_eq!(names, [&b"a"[..], b"x", b"y"]);
    assert!(out[1].placeholder.is_some());
}
