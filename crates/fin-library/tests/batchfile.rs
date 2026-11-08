//! The scan batch file (plan 5D Decision 26): NDJSON with a counted header.

use fin_library::batchfile::{decode, encode, file_name};
use fin_library::{DirDone, EntryType, RawRel, ScanBatch, ScanEntry};

fn entry(name: &str) -> ScanEntry {
    ScanEntry {
        dir: RawRel::root(),
        name: name.as_bytes().to_vec(),
        name_nfc: name.into(),
        name_lossy: false,
        rel_key: name.into(),
        entry_type: EntryType::File,
        size: 3,
        mtime_ns: 1_700_000_000_123_456_789,
        file_id: Some(vec![0, 0, 0, 0, 0, 0, 0, 7]),
        dev: Some("8:1".into()),
        mount_root: false,
        automount: false,
        reparse_tag: None,
        placeholder: None,
        excluded: None,
    }
}

fn batch() -> ScanBatch {
    ScanBatch {
        seq: 4,
        entries: vec![entry("a.mov"), entry("b\u{e9}.mov")],
        dirs_done: vec![DirDone {
            dir: RawRel::root(),
            mtime_ns: Some(5),
            complete: true,
            code: None,
            entries: 2,
            volume: None,
        }],
    }
}

#[test]
fn a_batch_round_trips_line_by_line() {
    let bytes = encode(&batch()).unwrap();
    assert_eq!(
        bytes.iter().filter(|b| **b == b'\n').count(),
        4,
        "header + 2 + 1"
    );
    assert_eq!(decode(bytes.as_slice()).unwrap(), batch());
    assert_eq!(file_name(4), "scan-00000004.ndjson");
}

#[test]
fn an_empty_batch_is_one_line() {
    let empty = ScanBatch {
        seq: 0,
        entries: vec![],
        dirs_done: vec![],
    };
    let bytes = encode(&empty).unwrap();
    assert_eq!(decode(bytes.as_slice()).unwrap(), empty);
}

#[test]
fn a_truncated_padded_or_garbled_file_is_refused_whole() {
    let bytes = String::from_utf8(encode(&batch()).unwrap()).unwrap();
    let lines: Vec<&str> = bytes.lines().collect();
    let cut = lines[..3].join("\n");
    assert!(decode(cut.as_bytes()).is_err(), "a missing dirDone");
    let padded = format!("{bytes}{}\n", lines[1]);
    assert!(decode(padded.as_bytes()).is_err(), "a line past the counts");
    let garbled = bytes
        .replace("a.mov", "")
        .replacen("\"size\":3", "\"size\":\"x\"", 1);
    assert!(decode(garbled.as_bytes()).is_err(), "a schema-invalid line");
    assert!(decode("".as_bytes()).is_err(), "no header");
}
