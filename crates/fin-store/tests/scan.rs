//! Folder scan and classification (sub-project 4 spec §3.4, §3.6, §3.7).

use std::fs;
use std::path::{Path, PathBuf};

use fin_core::new_project;
use fin_store::conflict_names::{Provider, classify};
use fin_store::names::{self, SegmentName, SnapName};
use fin_store::scan::{self, Entry, ForeignContent, NotArrivedWhy, Scanner, scan};
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
            ..
        } if path.file_name().unwrap() == file => Some(*conflict_copy),
        _ => None,
    })
}

#[test]
fn classifies_each_provider_pattern() {
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
        ("notes 2".into(), None),
        ("Untitled 2.txt".into(), None),
        ("photo (1).jpg".into(), None),
        (
            format!("{seg} (Case Conflict).fseg"),
            Some(Provider::Dropbox),
        ),
        (
            format!("{seg} (Selective Sync Conflict 1).fseg"),
            Some(Provider::Dropbox),
        ),
        (format!("{seg}-desktop-1.fseg"), Some(Provider::OneDrive)),
        (format!("{seg}-MAC.fseg"), Some(Provider::OneDrive)),
        ("lease-mac.json".into(), Some(Provider::OneDrive)),
        (
            "lease-my-very-long-host.json".into(),
            Some(Provider::OneDrive),
        ),
        ("lease.claim-5-HOST".into(), Some(Provider::OneDrive)),
        (format!("{seg} (1).fseg"), Some(Provider::GoogleDrive)),
        (format!("{seg}(12).fseg"), Some(Provider::GoogleDrive)),
        (
            format!("{seg} [Conflict].fseg"),
            Some(Provider::GoogleDrive),
        ),
        // Near misses.
        (format!("{seg}.fseg.bak"), None),
        (format!("{seg} 2x.fseg"), None),
        (format!("{seg}-other.fseg"), Some(Provider::OneDrive)),
        (format!("{seg}().fseg"), None),
        (format!("{seg}(a).fseg"), None),
        ("lease.json.old".into(), None),
        (" 2".into(), None),
        ("segments-old".into(), None),
    ];
    for (name, want) in table {
        assert_eq!(classify(name), *want, "{name}");
    }
}

#[test]
fn own_names_are_never_foreign() {
    let hash = |i: u64| format!("{:012x}", i.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 16);
    let mut own: Vec<String> = vec![
        "lease.json".into(),
        "lease.claim-7".into(),
        "lease.claim-00000012".into(),
        "lease.probe-01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
    ];
    for i in 0..300u64 {
        let h = hash(i);
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
        assert_eq!(classify(n), None, "{n}");
    }
    // The reparse logic does see through every marker applied to our names.
    for n in own
        .iter()
        .filter(|n| n.ends_with(".fseg") || n.ends_with(".fsnap"))
        .take(40)
    {
        let (stem, ext) = n.rsplit_once('.').unwrap();
        for (copy, p) in [
            (
                format!("{stem} (A's conflicted copy 2026-01-01).{ext}"),
                Provider::Dropbox,
            ),
            (
                format!("{stem}.sync-conflict-20260101-000000-ABCDEFG.{ext}"),
                Provider::Syncthing,
            ),
            (format!("{stem} 2.{ext}"), Provider::ICloud),
            (format!("{stem}-WIN-HOST-1.{ext}"), Provider::OneDrive),
            (format!("{stem} (3).{ext}"), Provider::GoogleDrive),
            (format!("{stem} [Conflict].{ext}"), Provider::GoogleDrive),
        ] {
            assert_eq!(classify(&copy), Some(p), "{copy}");
        }
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
fn only_lease_names_are_own_in_the_root() {
    let d = root();
    let f = d.path().join(".finisher");
    let (bytes, hash) = segment::encode(&header("main", 1, 1, 1), &[b"{}"]);
    let name = SegmentName::new(1, 1, 1, names::hash12(&hash).unwrap())
        .unwrap()
        .to_string();
    fs::write(f.join(&name), bytes).unwrap();
    let entries = scan(d.path()).unwrap();
    assert!(
        matches!(
            &entries[..],
            [Entry::Foreign {
                conflict_copy: Some(Provider::Unknown),
                content: Some(ForeignContent::Segment(_)),
                ..
            }]
        ),
        "{entries:?}"
    );
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

/// Runs `scan` on a thread so a regression that blocks fails instead of hanging the suite.
fn scan_with_timeout(dir: &Path) -> Vec<Entry> {
    let (tx, rx) = std::sync::mpsc::channel();
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        let _ = tx.send(scan(&dir));
    });
    rx.recv_timeout(std::time::Duration::from_secs(10))
        .expect("scan blocked")
        .unwrap()
}

#[cfg(unix)]
#[test]
fn symlink_is_foreign_and_never_followed() {
    let d = root();
    let outside = tempfile::tempdir().unwrap();
    let real = put_segment(outside.path(), "main", 1, 1);
    let dir = d.path().join(".finisher/segments/main");
    fs::create_dir_all(&dir).unwrap();
    let link = dir.join(real.file_name().unwrap());
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let entries = scan(d.path()).unwrap();
    assert_eq!(
        entries,
        [Entry::Foreign {
            path: link,
            conflict_copy: None,
            content: None
        }]
    );
}

#[cfg(unix)]
#[test]
fn fifo_with_canonical_name_does_not_block() {
    use std::os::unix::ffi::OsStrExt;
    let d = root();
    let good = put_segment(d.path(), "main", 1, 1);
    let fifo = good
        .parent()
        .unwrap()
        .join("00000001-000000000002-000000000002-0123456789ab.fseg");
    let c = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0);
    let entries = scan_with_timeout(d.path());
    assert!(entries.contains(&Entry::Foreign {
        path: fifo,
        conflict_copy: None,
        content: None
    }));
    assert!(entries.iter().any(|e| matches!(e, Entry::Segment { .. })));
}

#[test]
fn foreign_copies_carry_their_decoded_content() {
    let d = root();
    let seg = put_segment(d.path(), "main", 1, 1);
    let dir = seg.parent().unwrap();
    let good = fs::read(&seg).unwrap();
    fs::write(dir.join("a (Bob's conflicted copy).fseg"), &good).unwrap();
    fs::write(dir.join("b.fseg"), &good[..good.len() / 2]).unwrap();
    let mut bad = good.clone();
    bad[60] ^= 1;
    fs::write(dir.join("c.fseg"), &bad).unwrap();
    let (snap, _) = snap_bytes("main");
    fs::write(dir.join("d.bak"), &snap).unwrap();
    fs::write(dir.join("e.txt"), "plain text\n").unwrap();
    // Loose files in the root and directly under segments/.
    fs::write(d.path().join(".finisher/x.fseg"), &good).unwrap();
    fs::write(d.path().join(".finisher/segments/y.fseg"), &good).unwrap();
    let entries = scan(d.path()).unwrap();
    let content = |f: &str| {
        entries.iter().find_map(|e| match e {
            Entry::Foreign { path, content, .. } if path.file_name().unwrap() == f => {
                Some(content.clone())
            }
            _ => None,
        })
    };
    assert!(matches!(
        content("a (Bob's conflicted copy).fseg"),
        Some(Some(ForeignContent::Segment(_)))
    ));
    assert!(matches!(
        content("b.fseg"),
        Some(Some(ForeignContent::Damaged(_)))
    ));
    assert!(matches!(
        content("c.fseg"),
        Some(Some(ForeignContent::Damaged(_)))
    ));
    assert!(matches!(
        content("d.bak"),
        Some(Some(ForeignContent::Snapshot(_)))
    ));
    assert_eq!(content("e.txt"), Some(None));
    assert!(matches!(
        content("x.fseg"),
        Some(Some(ForeignContent::Segment(_)))
    ));
    assert!(matches!(
        content("y.fseg"),
        Some(Some(ForeignContent::Segment(_)))
    ));
}

#[cfg(unix)]
#[test]
fn unreadable_file_does_not_abort_the_scan() {
    use std::os::unix::fs::PermissionsExt;
    let d = root();
    let a = put_segment(d.path(), "main", 1, 1);
    let b = put_segment(d.path(), "main", 1, 2);
    fs::set_permissions(&a, fs::Permissions::from_mode(0o0)).unwrap();
    if fs::File::open(&a).is_ok() {
        return; // running as root
    }
    let entries = scan(d.path()).unwrap();
    fs::set_permissions(&a, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        matches!(&entries[..],
        [Entry::Unreadable { path, .. }, Entry::Segment { .. }] if *path == a),
        "{entries:?}"
    );
    let _ = b;
}

#[test]
fn only_main_and_br_digits_are_branches() {
    let d = root();
    let good = put_segment(d.path(), "main", 1, 1);
    let bytes = fs::read(&good).unwrap();
    let name = good.file_name().unwrap().to_owned();
    let seg_root = d.path().join(".finisher/segments");
    for b in ["br12", "br", "br1x", "Main", "other"] {
        fs::create_dir_all(seg_root.join(b)).unwrap();
        fs::write(seg_root.join(b).join(&name), &bytes).unwrap();
    }
    let entries = scan(d.path()).unwrap();
    let foreign_dirs: Vec<_> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Foreign {
                path,
                conflict_copy: None,
                content: None,
            } => Some(path.file_name().unwrap().to_str().unwrap().to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(foreign_dirs, ["Main", "br", "br1x", "other"]);
    // `br12` is valid but its header says `main`: a mismatch, not a segment.
    assert!(entries.iter().any(|e| matches!(e, Entry::Foreign { path, conflict_copy: Some(Provider::Unknown), .. } if path.parent().unwrap().ends_with("br12"))));
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, Entry::Segment { .. }))
            .count(),
        1
    );
}

#[cfg(unix)]
#[test]
fn non_utf8_names_are_foreign() {
    use std::os::unix::ffi::OsStrExt;
    let d = root();
    let dir = d.path().join(".finisher/segments/main");
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(std::ffi::OsStr::from_bytes(b"bad-\xff.fseg"));
    fs::write(&p, "x").unwrap();
    assert_eq!(
        scan(d.path()).unwrap(),
        [Entry::Foreign {
            path: p,
            conflict_copy: None,
            content: None
        }]
    );
}

#[test]
fn litter_is_skipped_and_sync_temps_are_not_arrived() {
    let d = root();
    put_segment(d.path(), "main", 1, 1);
    let dir = d.path().join(".finisher/segments/main");
    for n in [
        ".DS_Store",
        "._00000001-000000000001-000000000001-0123456789ab.fseg",
        "desktop.ini",
        "Thumbs.db",
        ".dropbox",
    ] {
        fs::write(dir.join(n), "x").unwrap();
    }
    fs::write(d.path().join(".finisher/.DS_Store"), "x").unwrap();
    let st = dir.join(".syncthing.foo.fseg.tmp");
    fs::write(&st, "x").unwrap();
    let st2 = dir.join("~syncthing~foo.tmp");
    fs::write(&st2, "x").unwrap();
    let entries = scan(d.path()).unwrap();
    assert_eq!(entries.len(), 3, "{entries:?}");
    for p in [st, st2] {
        assert!(entries.contains(&Entry::NotArrived {
            path: p,
            why: NotArrivedWhy::Temp
        }));
    }
}

#[test]
fn oversized_files_are_never_read() {
    let d = root();
    let dir = d.path().join(".finisher/segments/main");
    fs::create_dir_all(&dir).unwrap();
    let big = dir.join("00000001-000000000001-000000000001-0123456789ab.fseg");
    let f = fs::File::create(&big).unwrap();
    f.set_len(scan::SEGMENT_CAP + 1).unwrap();
    let foreign_big = dir.join("junk.bin");
    fs::File::create(&foreign_big)
        .unwrap()
        .set_len(scan::SEGMENT_CAP + 1)
        .unwrap();
    let before = scan::reads();
    let entries = scan(d.path()).unwrap();
    assert_eq!(scan::reads(), before, "oversized files are never read");
    assert!(
        matches!(&entries[..],
        [Entry::Damaged { reason, .. }, Entry::Foreign { content: None, .. }] if reason == "too large"),
        "{entries:?}"
    );
}

#[test]
fn unchanged_folder_is_not_reread_and_changes_are() {
    let d = root();
    let seg = put_segment(d.path(), "main", 1, 1);
    let snap_dir = d.path().join(".finisher/snapshots/main");
    fs::create_dir_all(&snap_dir).unwrap();
    let (bytes, hash) = snap_bytes("main");
    let sname = SnapName::new(5, names::hash12(&hash).unwrap()).unwrap();
    fs::write(snap_dir.join(sname.to_string()), bytes).unwrap();
    // Not cached: a truncated file stays re-read every time.
    let half = put_segment(d.path(), "main", 1, 2);
    let full = fs::read(&half).unwrap();
    fs::write(&half, &full[..full.len() - 5]).unwrap();

    let mut sc = Scanner::new();
    let r0 = scan::reads();
    let first = sc.scan(d.path()).unwrap();
    assert_eq!(scan::reads() - r0, 3);
    let second = sc.scan(d.path()).unwrap();
    assert_eq!(scan::reads() - r0, 4, "only the incomplete file is re-read");
    assert_eq!(first, second);

    // The truncated file completes (len changes): re-read, now cached.
    fs::write(&half, &full).unwrap();
    assert_eq!(sc.scan(d.path()).unwrap().len(), 3);
    assert_eq!(scan::reads() - r0, 5);
    sc.scan(d.path()).unwrap();
    assert_eq!(scan::reads() - r0, 5);

    // Same length, new mtime: re-read.
    let f = fs::OpenOptions::new().write(true).open(&seg).unwrap();
    f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
        .unwrap();
    drop(f);
    sc.scan(d.path()).unwrap();
    assert_eq!(scan::reads() - r0, 6);

    // A removed file is evicted: if it comes back, it is read again.
    fs::remove_file(&half).unwrap();
    sc.scan(d.path()).unwrap();
    fs::write(&half, &full).unwrap();
    sc.scan(d.path()).unwrap();
    assert_eq!(scan::reads() - r0, 7);
}
