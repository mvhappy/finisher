//! The scan batch file (plan 5D Decision 26): NDJSON, each line one generated `ScanBatch`,
//! the format `catalog/batch-file.ts` reads.

use fin_library::batchfile::{MAX_ENTRIES, decode, encode, file_name};
use fin_library::{DirDone, EntryType, RawRel, ScanBatch, ScanEntry};
use serde_json::{Value, json};

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

fn batch(n: usize) -> ScanBatch {
    ScanBatch {
        seq: 4,
        entries: (0..n).map(|i| entry(&format!("f{i}.mov"))).collect(),
        dirs_done: vec![DirDone {
            dir: RawRel::root(),
            mtime_ns: Some(5),
            complete: true,
            code: None,
            entries: n as u32,
            volume: None,
        }],
    }
}

#[test]
fn a_file_is_one_line_holding_one_scan_batch() {
    let bytes = encode(&batch(2)).unwrap();
    assert_eq!(bytes.iter().filter(|b| **b == b'\n').count(), 1);
    assert_eq!(*bytes.last().unwrap(), b'\n');
    let line: Value = serde_json::from_slice(&bytes).unwrap();
    // The generated type, key for key (camelCase, nanoseconds as decimal strings).
    assert_eq!(line["seq"], 4);
    assert_eq!(line["entries"][0]["name"], "ZjAubW92");
    assert_eq!(line["entries"][0]["mtimeNs"], "1700000000123456789");
    assert_eq!(line["entries"][0]["entryType"], "file");
    assert_eq!(line["entries"][0]["fileId"], "AAAAAAAAAAc=");
    assert_eq!(
        line["dirsDone"][0],
        json!({ "dir": { "components": [] }, "mtimeNs": "5", "complete": true, "entries": 2 })
    );
    assert_eq!(decode(bytes.as_slice()).unwrap(), vec![batch(2)]);
    assert_eq!(file_name(4), "scan-00000004.ndjson");
}

#[test]
fn several_lines_decode_in_order() {
    let mut bytes = encode(&batch(1)).unwrap();
    bytes.extend(encode(&batch(2)).unwrap());
    let got = decode(bytes.as_slice()).unwrap();
    assert_eq!(
        got.iter().map(|b| b.entries.len()).collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn the_caps_and_bad_lines_are_refused() {
    assert!(encode(&batch(MAX_ENTRIES)).is_ok());
    assert!(
        encode(&batch(MAX_ENTRIES + 1)).is_err(),
        "over 4,096 entries"
    );
    let mut two = encode(&batch(MAX_ENTRIES)).unwrap();
    two.extend(encode(&batch(1)).unwrap());
    assert!(
        decode(two.as_slice()).is_err(),
        "4,097 entries over two lines"
    );
    assert!(decode("".as_bytes()).is_err(), "no batch");
    assert!(decode("\n".as_bytes()).is_err(), "a blank line");
    assert!(decode("{\"seq\":1}\n".as_bytes()).is_err(), "missing keys");
    let extra = String::from_utf8(encode(&batch(1)).unwrap())
        .unwrap()
        .replacen("{\"seq\"", "{\"extra\":1,\"seq\"", 1);
    assert!(decode(extra.as_bytes()).is_err(), "an unknown key");
}
