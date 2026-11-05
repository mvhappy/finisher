//! Project-folder file primitives and the segment/snapshot codecs (sub-project 4 spec §3.4).

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use fin_core::new_project;
use fin_store::fsio;
use fin_store::names::{self, SegmentName, SnapName};
use fin_store::segment::{self, Header, SegError};
use fin_store::snapfile::{self, SnapHeader};

/// A format-1 header: what an edit-only segment carries (4B writes 2 only for its kinds).
fn header() -> Header {
    Header {
        format_version: 1,
        min_reader_version: 1,
        project_id: "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        branch: "main".into(),
        writer_id: "01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
        epoch: 3,
        first_seq: 7,
        last_seq: 9,
        prev_segment_hash: Some(format!("blake3:{}", "ab".repeat(32))),
        op_schema_versions: [1, 2],
        created_at: 1_793_787_330_042,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
        pack: false,
        replaces: vec![],
    }
}

/// Record bytes with odd spacing and non-ASCII text: the codec must keep them byte-for-byte.
const RECORDS: [&[u8]; 3] = [
    br#"{"txId":"t7","seq":7}"#,
    "{\"txId\":\"t8\", \"seq\" : 8, \"author\":\"Ołha\"}".as_bytes(),
    "{\"txId\":\"t9\",\"seq\":9,\"note\":\"ъ\\n\"}".as_bytes(),
];

fn small_segment() -> Vec<u8> {
    segment::encode(&header(), &RECORDS).0
}

/// The file with its trailer line replaced by `{"crc32c":<crc of body>,"hash":<hash>}`.
fn with_trailer_hash(bytes: &[u8], hash: &str) -> Vec<u8> {
    let body_end = bytes[..bytes.len() - 1]
        .iter()
        .rposition(|&b| b == b'\n')
        .unwrap()
        + 1;
    let body = &bytes[..body_end];
    let mut out = body.to_vec();
    out.extend_from_slice(
        format!(
            "{{\"crc32c\":\"{:08x}\",\"hash\":\"{hash}\"}}\n",
            crc32c::crc32c(body)
        )
        .as_bytes(),
    );
    out
}

#[test]
fn segment_roundtrip_preserves_record_bytes() {
    let (bytes, hash) = segment::encode(&header(), &RECORDS);
    assert!(
        hash.starts_with("blake3:") && hash.len() == 7 + 64,
        "{hash}"
    );
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.starts_with("{\"magic\":\"finisher-segment\",\"formatVersion\":1,"));
    assert_eq!(text.lines().count(), 1 + RECORDS.len() + 1);
    assert!(
        !text.contains("\"pack\""),
        "a plain segment omits pack fields"
    );

    let seg = segment::decode(&bytes).unwrap();
    assert_eq!(seg.header, header());
    assert_eq!(seg.hash, hash);
    let records: Vec<&[u8]> = seg.records.iter().map(Vec::as_slice).collect();
    assert_eq!(records, RECORDS);

    let mut pack = header();
    pack.pack = true;
    pack.prev_segment_hash = None;
    pack.replaces = vec![segment::Replaced {
        hash: format!("blake3:{}", "01".repeat(32)),
        last_tx_hash: format!("blake3:{}", "02".repeat(32)),
    }];
    let (bytes, hash) = segment::encode(&pack, &RECORDS);
    let seg = segment::decode(&bytes).unwrap();
    assert_eq!((seg.header, seg.hash), (pack, hash));
}

#[test]
fn trailer_covers_all_prior_bytes() {
    let bytes = small_segment();
    for offset in 0..bytes.len() {
        for mask in [0x01u8, 0x02, 0x04, 0x20, 0x80, 0xFF] {
            let mut damaged = bytes.clone();
            damaged[offset] ^= mask;
            match segment::decode(&damaged) {
                Err(SegError::Corrupt(_)) => {}
                other => panic!(
                    "flip {mask:#04x} at {offset} ({:?}): {other:?}",
                    bytes[offset] as char
                ),
            }
        }
    }
}

#[test]
fn truncated_is_incomplete_not_corrupt() {
    let bytes = small_segment();
    for cut in 0..bytes.len() {
        match segment::decode(&bytes[..cut]) {
            Err(SegError::Incomplete) => {}
            other => panic!("cut at {cut} of {}: {other:?}", bytes.len()),
        }
    }
}

#[test]
fn newer_min_reader_is_reported() {
    let mut h = header();
    h.format_version = 3;
    h.min_reader_version = 3;
    let (bytes, _) = segment::encode(&h, &RECORDS);
    assert!(matches!(
        segment::decode(&bytes),
        Err(SegError::Newer { min_reader: 3 })
    ));

    // Readable by us although written by a newer format revision.
    let mut h = header();
    h.format_version = 3;
    let (bytes, hash) = segment::encode(&h, &RECORDS);
    assert_eq!(segment::decode(&bytes).unwrap().hash, hash);
}

#[test]
fn unknown_hash_prefix_is_unsupported() {
    let other = format!("sha256:{}", "cd".repeat(32));
    let mut h = header();
    h.format_version = 3;
    let bytes = with_trailer_hash(&segment::encode(&h, &RECORDS).0, &other);
    assert!(matches!(
        segment::decode(&bytes),
        Err(SegError::UnsupportedHash)
    ));
    // Formats 1 and 2 always use BLAKE3, so another algorithm there is damage.
    let mut h = header();
    h.format_version = 2;
    let bytes = with_trailer_hash(&segment::encode(&h, &RECORDS).0, &other);
    assert!(matches!(segment::decode(&bytes), Err(SegError::Corrupt(_))));
    let bytes = with_trailer_hash(&small_segment(), &other);
    assert!(matches!(segment::decode(&bytes), Err(SegError::Corrupt(_))));
    // A wrong BLAKE3 digest with a good CRC is damage too.
    let bytes = with_trailer_hash(&small_segment(), &format!("blake3:{}", "00".repeat(32)));
    assert!(matches!(segment::decode(&bytes), Err(SegError::Corrupt(_))));
}

#[test]
fn bad_headers_and_lines_are_corrupt() {
    let reencode = |line: &str| -> Vec<u8> {
        let mut body = line.as_bytes().to_vec();
        body.extend_from_slice(b"\n{\"txId\":\"t7\"}\n{\"txId\":\"t8\"}\n{\"txId\":\"t9\"}\n");
        let hash = format!("blake3:{}", blake3::hash(&body).to_hex());
        let mut out = body.clone();
        out.extend_from_slice(b"{}\n");
        with_trailer_hash(&out, &hash)
    };
    let good = String::from_utf8(small_segment()).unwrap();
    let head = good.lines().next().unwrap();
    for bad in [
        head.replace("finisher-segment", "finisher-snapshot"),
        head.replace("\"firstSeq\":7", "\"firstSeq\":10"),
        // Three records, but the range says two or four.
        head.replace("\"firstSeq\":7", "\"firstSeq\":8"),
        head.replace("\"lastSeq\":9", "\"lastSeq\":10"),
        head.replace("\"firstSeq\":7", "\"firstSeq\":0")
            .replace("\"lastSeq\":9", "\"lastSeq\":18446744073709551615"),
        head.replace("\"opSchemaVersions\":[1,2]", "\"opSchemaVersions\":[2,1]"),
        head.replace(
            "\"prevSegmentHash\":\"blake3:ab",
            "\"prevSegmentHash\":\"blake3:AB",
        ),
        head.replace("\"branch\":\"main\"", "\"branch\":\"\""),
        head.replace(",\"createdAt\"", ",\"replaces\":[\"x\"],\"createdAt\""),
        "[]".to_string(),
    ] {
        assert!(
            matches!(segment::decode(&reencode(&bad)), Err(SegError::Corrupt(_))),
            "{bad}"
        );
    }
    assert!(segment::decode(&reencode(head)).is_ok());
    let same = head.replace("\"opSchemaVersions\":[1,2]", "\"opSchemaVersions\":[2,2]");
    assert!(segment::decode(&reencode(&same)).is_ok());
    // A segment needs at least one record.
    let (bytes, _) = segment::encode(&header(), &RECORDS[..1]);
    let lines: Vec<&str> = std::str::from_utf8(&bytes).unwrap().lines().collect();
    let body = format!("{}\n", lines[0]);
    let hash = format!("blake3:{}", blake3::hash(body.as_bytes()).to_hex());
    let empty = with_trailer_hash(format!("{body}{{}}\n").as_bytes(), &hash);
    assert!(matches!(segment::decode(&empty), Err(SegError::Corrupt(_))));
}

fn snap_header() -> SnapHeader {
    SnapHeader {
        format_version: snapfile::FORMAT_VERSION,
        min_reader_version: snapfile::MIN_READER_VERSION,
        project_id: "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        branch: "main".into(),
        seq: 1200,
        tx_hash: format!("blake3:{}", "9f".repeat(32)),
        model_schema_version: fin_core::SCHEMA_VERSION,
        writer_id: "01JBZ8K3X4Y5Z6A7B8C9D0E1F3".into(),
        epoch: 3,
        genesis_hash: format!("blake3:{}", "0".repeat(64)),
    }
}

#[test]
fn snapshot_roundtrip_flip_and_truncate() {
    let project = new_project(
        "01jbz8k3x4y5z6a7b8c9d0e1f2".into(),
        "Сон".into(),
        fin_core::Rational::from_int(25),
    )
    .unwrap();
    let (bytes, hash) = snapfile::encode(&snap_header(), &project);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.starts_with("{\"magic\":\"finisher-snapshot\","));
    assert_eq!(text.lines().count(), 3);
    let snap = snapfile::decode(&bytes).unwrap();
    assert_eq!((&snap.header, &snap.hash), (&snap_header(), &hash));
    assert_eq!(snap.body, serde_json::to_vec(&project).unwrap());
    let back = fin_core::upcast_snapshot(serde_json::from_slice(&snap.body).unwrap()).unwrap();
    assert_eq!(back, project);

    for offset in 0..bytes.len() {
        let mut damaged = bytes.clone();
        damaged[offset] ^= 0x01;
        assert!(
            matches!(snapfile::decode(&damaged), Err(SegError::Corrupt(_))),
            "flip at {offset}"
        );
        assert!(
            matches!(
                snapfile::decode(&bytes[..offset]),
                Err(SegError::Incomplete)
            ),
            "cut at {offset}"
        );
    }
    // A segment is not a snapshot.
    assert!(matches!(
        snapfile::decode(&small_segment()),
        Err(SegError::Corrupt(_))
    ));
    let mut h = snap_header();
    h.min_reader_version = 5;
    h.format_version = 5;
    let (bytes, _) = snapfile::encode(&h, &project);
    assert!(matches!(
        snapfile::decode(&bytes),
        Err(SegError::Newer { min_reader: 5 })
    ));
}

#[test]
fn names_parse_strict() {
    let seg = SegmentName::new(3, 7, 9, "0123456789ab").unwrap();
    assert_eq!(
        (seg.epoch(), seg.first_seq(), seg.last_seq(), seg.hash12()),
        (3, 7, 9, "0123456789ab")
    );
    let shown = seg.to_string();
    assert_eq!(
        shown,
        "00000003-000000000007-000000000009-0123456789ab.fseg"
    );
    assert_eq!(SegmentName::parse(&shown), Some(seg));
    for bad in [
        "00000003-000000000007-000000000009-0123456789ab 2.fseg",
        "00000003-000000000007-000000000009-0123456789ab (1).fseg",
        "00000003-000000000007-000000000009-0123456789ab (conflicted copy).fseg",
        "00000003-000000000007-000000000009-0123456789ab.fseg.tmp",
        "0000003-000000000007-000000000009-0123456789ab.fseg",
        "000000003-000000000007-000000000009-0123456789ab.fseg",
        "00000003-00000000007-000000000009-0123456789ab.fseg",
        "00000003-000000000007-0000000000009-0123456789ab.fseg",
        "00000003-000000000007-000000000009-0123456789a.fseg",
        "00000003-000000000007-000000000009-0123456789abc.fseg",
        "00000003-000000000007-000000000009-0123456789AB.fseg",
        "00000003-000000000007-000000000009-0123456789ag.fseg",
        "00000003-000000000009-000000000007-0123456789ab.fseg",
        "+0000003-000000000007-000000000009-0123456789ab.fseg",
        " 0000003-000000000007-000000000009-0123456789ab.fseg",
        "00000003-000000000007-000000000009-0123456789ab.FSEG",
        "00000003-000000000007-000000000009-0123456789ab.fsnap",
        "00000003_000000000007_000000000009_0123456789ab.fseg",
        "",
    ] {
        assert_eq!(SegmentName::parse(bad), None, "{bad:?}");
    }

    let snap = SnapName::new(1200, "fedcba987654").unwrap();
    assert_eq!((snap.seq(), snap.hash12()), (1200, "fedcba987654"));
    let shown = snap.to_string();
    assert_eq!(shown, "000000001200-fedcba987654.fsnap");
    assert_eq!(SnapName::parse(&shown), Some(snap));
    for bad in [
        "000000001200-fedcba987654 2.fsnap",
        "000000001200-fedcba987654(1).fsnap",
        "000000001200-fedcba987654 (conflicted copy).fsnap",
        "00000001200-fedcba987654.fsnap",
        "0000000001200-fedcba987654.fsnap",
        "000000001200-fedcba98765.fsnap",
        "000000001200-fedcba987654.fseg",
        "-00000001200-fedcba987654.fsnap",
    ] {
        assert_eq!(SnapName::parse(bad), None, "{bad:?}");
    }

    // A name is only ever built if it parses back: widths, seq order and hash are checked.
    let max = SegmentName::new(names::MAX_EPOCH, 0, names::MAX_SEQ, "0123456789ab").unwrap();
    assert_eq!(SegmentName::parse(&max.to_string()), Some(max));
    for (epoch, first, last, hash) in [
        (names::MAX_EPOCH + 1, 7, 9, "0123456789ab"),
        (3, 7, names::MAX_SEQ + 1, "0123456789ab"),
        (3, names::MAX_SEQ + 1, names::MAX_SEQ + 1, "0123456789ab"),
        (3, 9, 7, "0123456789ab"),
        (3, 7, 9, "0123456789AB"),
        (3, 7, 9, "0123456789a"),
    ] {
        assert_eq!(SegmentName::new(epoch, first, last, hash), None);
    }
    let max = SnapName::new(names::MAX_SEQ, "fedcba987654").unwrap();
    assert_eq!(SnapName::parse(&max.to_string()), Some(max));
    assert_eq!(SnapName::new(names::MAX_SEQ + 1, "fedcba987654"), None);
    assert_eq!(SnapName::new(1, "fedcba98765g"), None);

    let temp = names::temp_name();
    assert!(names::is_temp(&temp), "{temp}");
    for bad in [
        ".~fin-.tmp",
        ".~fin-x.tmp",
        "~fin-01JBZ8K3X4Y5Z6A7B8C9D0E1F3.tmp",
    ] {
        assert!(!names::is_temp(bad), "{bad}");
    }
    assert!(names::is_temp(".~fin-01JBZ8K3X4Y5Z6A7B8C9D0E1F3.tmp"));
    assert_eq!(
        names::hash12("blake3:0123456789abcdef"),
        Some("0123456789ab")
    );
    assert_eq!(names::hash12("blake3:0123"), None);
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn write_once_never_overwrites() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fsio::write_once(dir, "a.fseg", b"first").unwrap();
    let err = fsio::write_once(dir, "a.fseg", b"second").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::AlreadyExists, "{err}");
    assert_eq!(fs::read(dir.join("a.fseg")).unwrap(), b"first");
    assert_eq!(file_names(dir), ["a.fseg"], "no temp file is left behind");
    let err = fsio::write_once(&dir.join("missing"), "b.fseg", b"x").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound, "{err}");
}

#[test]
fn read_uncached_reads_every_size() {
    // The system temp dir is often tmpfs, the target dir a disk filesystem. Both may accept
    // O_DIRECT (tmpfs does since Linux 6.6); `read_uncached_falls_back_when_direct_is_refused`
    // covers the fallback.
    let tmp = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    for dir in [tmp.path(), target.path()] {
        for len in [0usize, 1, 511, 512, 4095, 4096, 4097, 70_000, (1 << 20) + 3] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 251) as u8).collect();
            let name = format!("f{len}");
            fsio::write_once(dir, &name, &bytes).unwrap();
            assert_eq!(
                fsio::read_uncached(&dir.join(&name)).unwrap(),
                bytes,
                "{len}"
            );
        }
    }
    let err = fsio::read_uncached(&tmp.path().join("missing")).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

/// The buffered fallback, forced through the test hook: filesystems that refuse O_DIRECT
/// (older tmpfs, some network mounts) or Windows `NO_BUFFERING` still read correctly.
#[cfg(any(target_os = "linux", target_os = "android", windows))]
#[test]
fn read_uncached_falls_back_when_direct_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    for len in [0usize, 1, 4096, 70_000] {
        let bytes: Vec<u8> = (0..len).map(|i| (i * 13 % 251) as u8).collect();
        let name = format!("f{len}");
        fsio::write_once(tmp.path(), &name, &bytes).unwrap();
        let before = fsio::buffered_fallbacks();
        fsio::refuse_next_direct_read();
        assert_eq!(fsio::read_uncached(&tmp.path().join(&name)).unwrap(), bytes);
        assert_eq!(fsio::buffered_fallbacks(), before + 1, "{len}");
        // The hook is one-shot.
        assert_eq!(fsio::read_uncached(&tmp.path().join(&name)).unwrap(), bytes);
        assert_eq!(fsio::buffered_fallbacks(), before + 1, "{len}");
    }
}

#[cfg(unix)]
fn file_id(path: &Path) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    let m = fs::metadata(path).unwrap();
    (m.dev(), m.ino())
}

#[cfg(windows)]
fn file_id(path: &Path) -> (u64, u64) {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle,
    };
    let file = fs::File::open(path).unwrap();
    // SAFETY: the handle is open for the duration of the call and `info` is a plain out struct.
    let info = unsafe {
        let mut info: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
        assert_ne!(
            GetFileInformationByHandle(file.as_raw_handle(), &mut info),
            0
        );
        info
    };
    (
        u64::from(info.dwVolumeSerialNumber),
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

#[test]
fn in_place_write_keeps_inode() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("lease.json");
    // Open-only by default: a deleted lease.json is never silently recreated.
    let err = fsio::write_in_place(&path, &[b'a'; 512]).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound, "{err}");
    assert!(!path.exists());
    fsio::write_in_place_or_create(&path, &[b'a'; 512]).unwrap();
    let before = file_id(&path);
    fsio::write_in_place(&path, &[b'b'; 512]).unwrap();
    assert_eq!(file_id(&path), before);
    assert_eq!(fs::read(&path).unwrap(), vec![b'b'; 512]);
    // Never truncates: a shorter write overwrites the start only.
    fsio::write_in_place(&path, b"cc").unwrap();
    let got = fs::read(&path).unwrap();
    assert_eq!((got.len(), &got[..3]), (512, &b"ccb"[..]));
    assert_eq!(file_id(&path), before);
    fsio::write_in_place_or_create(&path, b"d").unwrap();
    assert_eq!(&fs::read(&path).unwrap()[..3], b"dcb");
    assert_eq!(file_id(&path), before);
    assert_eq!(file_names(tmp.path()), ["lease.json"]);
}

#[test]
fn transient_errors_are_classified() {
    use std::io::Error;
    assert!(fsio::is_transient(&Error::from(ErrorKind::Interrupted)));
    assert!(fsio::is_transient(&Error::from(ErrorKind::WouldBlock)));
    assert!(!fsio::is_transient(&Error::from(ErrorKind::NotFound)));
    assert!(!fsio::is_transient(&Error::from(ErrorKind::AlreadyExists)));
    #[cfg(unix)]
    {
        assert!(fsio::is_transient(&Error::from_raw_os_error(libc::EBUSY)));
        assert!(fsio::is_transient(&Error::from_raw_os_error(libc::ESTALE)));
        assert!(!fsio::is_transient(&Error::from_raw_os_error(libc::EACCES)));
        assert!(!fsio::is_transient(&Error::from_raw_os_error(libc::ENOSPC)));
    }
    #[cfg(windows)]
    for code in [5, 32, 33, 64] {
        assert!(
            fsio::is_transient(&Error::from_raw_os_error(code)),
            "{code}"
        );
    }
}

#[test]
fn ordinary_files_are_not_placeholders() {
    let tmp = tempfile::tempdir().unwrap();
    fs::write(tmp.path().join("x"), b"x").unwrap();
    for entry in fs::read_dir(tmp.path()).unwrap() {
        let entry = entry.unwrap();
        assert!(!fsio::is_placeholder(
            &entry.path(),
            &entry.metadata().unwrap()
        ));
    }
}
