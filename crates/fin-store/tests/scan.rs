//! Folder scan and classification (sub-project 4 spec §3.4, §3.6, §3.7).

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::new_project;
use fin_store::conflict_names::{Provider, classify};
use fin_store::names::{self, SegmentName, SnapName};
use fin_store::scan::{self, Entry, NotArrivedWhy, scan};
use fin_store::segment::{self, Header};
use fin_store::snapfile::{self, SnapHeader};

fn header(branch: &str, epoch: u64, first: u64, last: u64) -> Header {
    Header {
        format_version: segment::FORMAT_VERSION,
        min_reader_version: segment::MIN_READER_VERSION,
        project_id: "p1".into(),
        branch: branch.into(),
        writer_id: "w1".into(),
        epoch,
        first_seq: first,
        last_seq: last,
        prev_segment_hash: None,
        op_schema_versions: [1, 1],
        created_at: 0,
        pack: false,
        replaces: vec![],
    }
}

/// Writes a valid one-record segment under its canonical name; returns the path.
fn put_segment(root: &Path, branch: &str, epoch: u64, seq: u64) -> PathBuf {
    let (bytes, hash) = segment::encode(&header(branch, epoch, seq, seq), &[b"{\"seq\":1}"]);
    let name = SegmentName::new(epoch, seq, seq, names::hash12(&hash).unwrap()).unwrap();
    let dir = root.join(".finisher/segments").join(branch);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name.to_string());
    fs::write(&path, bytes).unwrap();
    path
}

fn snap_bytes(branch: &str) -> (Vec<u8>, String) {
    let project = new_project("p1".into(), "x".into(), fin_core::Rational::from_int(25)).unwrap();
    let h = SnapHeader {
        format_version: snapfile::FORMAT_VERSION,
        min_reader_version: snapfile::MIN_READER_VERSION,
        project_id: "p1".into(),
        branch: branch.into(),
        seq: 5,
        tx_hash: format!("blake3:{}", "9f".repeat(32)),
        model_schema_version: fin_core::SCHEMA_VERSION,
        writer_id: "w1".into(),
        epoch: 1,
    };
    snapfile::encode(&h, &project)
}

fn root() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join(".finisher")).unwrap();
    d
}

fn foreign(entries: &[Entry], file: &str) -> Option<Option<Provider>> {
    entries.iter().find_map(|e| match e {
        Entry::Foreign {
            path,
            conflict_copy,
        } if path.file_name().unwrap() == file => Some(*conflict_copy),
        _ => None,
    })
}

#[test]
fn classifies_each_provider_pattern() {
    let host = vec!["DESKTOP-1".to_string(), "mac".to_string()];
    let seg = "00000001-000000000005-000000000005-0123456789ab";
    let table: &[(String, Option<Provider>)] = &[
        (format!("{seg}.fseg"), None),
        (
            format!("{seg} (Bob's conflicted copy 2026-11-04).fseg"),
            Some(Provider::Dropbox),
        ),
        (
            format!("{seg} (BOB'S CONFLICTED COPY).fseg"),
            Some(Provider::Dropbox),
        ),
        (
            format!("{seg}.sync-conflict-20261104-101010-ABCDEFG.fseg"),
            Some(Provider::Syncthing),
        ),
        (format!("{seg} 2.fseg"), Some(Provider::ICloud)),
        (format!("{seg} 12.FSEG"), Some(Provider::ICloud)),
        ("lease 3.json".into(), Some(Provider::ICloud)),
        ("notes 2".into(), Some(Provider::ICloud)),
        (format!("{seg}-desktop-1.fseg"), Some(Provider::OneDrive)),
        (format!("{seg}-MAC.fseg"), Some(Provider::OneDrive)),
        ("lease-mac.json".into(), Some(Provider::OneDrive)),
        (format!("{seg} (1).fseg"), Some(Provider::GoogleDrive)),
        (format!("{seg}(12).fseg"), Some(Provider::GoogleDrive)),
        (
            format!("{seg} [Conflict].fseg"),
            Some(Provider::GoogleDrive),
        ),
        // Near misses.
        (format!("{seg}.fseg.bak"), None),
        (format!("{seg} 2x.fseg"), None),
        (format!("{seg}-other.fseg"), None),
        (format!("{seg}().fseg"), None),
        (format!("{seg}(a).fseg"), None),
        ("lease.json.old".into(), None),
        (" 2".into(), Some(Provider::ICloud)),
    ];
    for (name, want) in table {
        assert_eq!(classify(name, &host), *want, "{name}");
    }
}

#[test]
fn own_names_are_never_foreign() {
    let hash = |i: u64| format!("{:012x}", i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 16);
    // Hostnames that look like pieces of our own names.
    let mut host = vec!["fseg".to_string(), "main".into(), "0001".into(), "2".into()];
    let mut own: Vec<String> = vec![
        "lease.json".into(),
        "lease.claim-7".into(),
        "lease.claim-00000012".into(),
        "lease.probe-01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
    ];
    for i in 0..300u64 {
        let h = hash(i);
        host.push(h.clone());
        let seg = SegmentName::new(i % 100, i, i + (i % 7), &h).unwrap();
        let snap = SnapName::new(i * 3, &h).unwrap();
        own.push(seg.to_string());
        own.push(snap.to_string());
        own.push(names::temp_name());
    }
    // Digit-heavy extremes.
    own.push(
        SegmentName::new(2, 2, 2, "000000000001")
            .unwrap()
            .to_string(),
    );
    own.push(
        SegmentName::new(99_999_999, 999_999_999_999, 999_999_999_999, "111111111111")
            .unwrap()
            .to_string(),
    );
    own.push(SnapName::new(1, "222222222222").unwrap().to_string());
    for n in &own {
        assert!(!n.contains(' ') && !n.contains('('), "{n}");
        assert_eq!(classify(n, &host), None, "{n}");
    }
}

#[test]
fn lease_json_conflict_copy_is_flagged() {
    let d = root();
    let f = d.path().join(".finisher");
    fs::write(f.join("lease.json"), [0u8; 512]).unwrap();
    fs::write(f.join("lease 2.json"), [0u8; 512]).unwrap();
    fs::write(f.join("lease (Bob's conflicted copy).json"), [0u8; 512]).unwrap();
    fs::write(f.join("lease.claim-4"), "w1").unwrap();
    fs::write(f.join("lease.probe-ABC"), "x").unwrap();
    fs::write(f.join("stray.txt"), "x").unwrap();
    let entries = scan(d.path()).unwrap();
    assert_eq!(
        foreign(&entries, "lease 2.json"),
        Some(Some(Provider::ICloud))
    );
    assert_eq!(
        foreign(&entries, "lease (Bob's conflicted copy).json"),
        Some(Some(Provider::Dropbox))
    );
    assert_eq!(foreign(&entries, "stray.txt"), Some(None));
    assert_eq!(entries.len(), 3, "{entries:?}");
}

#[test]
fn temp_files_are_not_arrived() {
    let d = root();
    put_segment(d.path(), "main", 1, 1);
    let dir = d.path().join(".finisher/segments/main");
    let t = dir.join(names::temp_name());
    fs::write(&t, "half").unwrap();
    let entries = scan(d.path()).unwrap();
    assert!(entries.contains(&Entry::NotArrived {
        path: t,
        why: NotArrivedWhy::Temp
    }));
    assert_eq!(entries.len(), 2);
}

#[test]
fn placeholder_is_not_arrived() {
    let d = root();
    let seg = put_segment(d.path(), "main", 1, 1);
    let snap_dir = d.path().join(".finisher/snapshots/main");
    fs::create_dir_all(&snap_dir).unwrap();
    // A legacy iCloud stub is recognised by name.
    let stub = snap_dir.join(".000000000005-0123456789ab.fsnap.icloud");
    fs::write(&stub, "stub").unwrap();
    // A hooked placeholder is never read: make its bytes unreadable on purpose.
    let hooked = snap_dir.join("000000000005-0123456789ab.fsnap");
    fs::write(
        &hooked,
        "garbage that would be Damaged if read\n{\"crc32c\":\"00000000\",\"hash\":\"blake3:00\"}\n",
    )
    .unwrap();
    scan::mark_placeholder(&hooked);
    scan::mark_placeholder(&seg);
    let entries = scan(d.path()).unwrap();
    scan::clear_placeholders();
    for p in [&seg, &hooked, &stub] {
        assert!(
            entries.contains(&Entry::NotArrived {
                path: p.clone(),
                why: NotArrivedWhy::Placeholder
            }),
            "{p:?} in {entries:?}"
        );
    }
    assert_eq!(entries.len(), 3);
    // Hydrated: the same files now decode (stateless rescan).
    assert!(matches!(
        scan(d.path()).unwrap()[..],
        [
            Entry::Segment { .. },
            Entry::NotArrived { .. },
            Entry::Damaged { .. }
        ]
    ));
}

#[test]
fn newer_file_reported() {
    let d = root();
    let mut h = header("main", 1, 1, 1);
    h.format_version = 99;
    h.min_reader_version = 99;
    let (bytes, hash) = segment::encode(&h, &[b"{}"]);
    let name = SegmentName::new(1, 1, 1, names::hash12(&hash).unwrap()).unwrap();
    let dir = d.path().join(".finisher/segments/main");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name.to_string()), bytes).unwrap();
    let entries = scan(d.path()).unwrap();
    assert!(
        matches!(&entries[..], [Entry::Newer { min_reader: 99, .. }]),
        "{entries:?}"
    );
}

#[test]
fn valid_segments_and_snapshots_decode_per_branch() {
    let d = root();
    put_segment(d.path(), "main", 1, 1);
    put_segment(d.path(), "br2", 2, 4);
    let (bytes, hash) = snap_bytes("main");
    let name = SnapName::new(5, names::hash12(&hash).unwrap()).unwrap();
    let dir = d.path().join(".finisher/snapshots/main");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(name.to_string()), bytes).unwrap();
    let entries = scan(d.path()).unwrap();
    let segs: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Segment { branch, name, seg } => {
                Some((branch.as_str(), name.epoch(), seg.header.first_seq))
            }
            _ => None,
        })
        .collect();
    assert_eq!(segs, [("br2", 2, 4), ("main", 1, 1)]);
    assert!(entries.iter().any(
        |e| matches!(e, Entry::Snapshot { branch, header, hash: h, .. }
        if branch == "main" && header.seq == 5 && *h == hash)
    ));
}

#[test]
fn truncated_is_not_arrived_and_damage_is_reported_statelessly() {
    let d = root();
    let path = put_segment(d.path(), "main", 1, 1);
    let good = fs::read(&path).unwrap();
    fs::write(&path, &good[..good.len() - 10]).unwrap();
    assert!(matches!(
        scan(d.path()).unwrap()[..],
        [Entry::NotArrived {
            why: NotArrivedWhy::Incomplete,
            ..
        }]
    ));
    let mut bad = good.clone();
    bad[40] ^= 1;
    fs::write(&path, &bad).unwrap();
    assert!(matches!(
        scan(d.path()).unwrap()[..],
        [Entry::Damaged { .. }]
    ));
    fs::write(&path, &good).unwrap();
    assert!(matches!(
        scan(d.path()).unwrap()[..],
        [Entry::Segment { .. }]
    ));
}

#[test]
fn finisher_file_under_wrong_name_is_unknown_conflict_copy() {
    let d = root();
    let path = put_segment(d.path(), "main", 1, 1);
    let dir = path.parent().unwrap();
    fs::copy(&path, dir.join("renamed-by-someone.fseg")).unwrap();
    fs::copy(
        &path,
        dir.join("00000001-000000000001-000000000001-ffffffffffff.fseg"),
    )
    .unwrap();
    fs::write(dir.join("readme.txt"), "hi").unwrap();
    let entries = scan(d.path()).unwrap();
    assert_eq!(
        foreign(&entries, "renamed-by-someone.fseg"),
        Some(Some(Provider::Unknown))
    );
    assert_eq!(
        foreign(
            &entries,
            "00000001-000000000001-000000000001-ffffffffffff.fseg"
        ),
        Some(Some(Provider::Unknown))
    );
    assert_eq!(foreign(&entries, "readme.txt"), Some(None));
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Segment { .. }))
            .count(),
        1
    );
}

#[test]
fn missing_finisher_dir_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert!(scan(d.path()).unwrap().is_empty());
}
