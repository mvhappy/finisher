//! Raw paths, WTF-8 and the wire forms of the frozen types (plan 5D Task 1).

use fin_library::raw::{from_wide, to_wide};
use fin_library::{
    DirDone, EntryType, Expect, Placeholder, RawRel, RootState, SampleEntry, ScanBatch, ScanEntry,
    VolumeIdentity,
};
use serde_json::{Value, json};

#[test]
fn raw_rel_round_trips_non_utf8_and_wtf8() {
    // A lone high surrogate, then "a"; and a lone low surrogate.
    let lone_high = from_wide(&[0xD800, u16::from(b'a')]);
    let lone_low = from_wide(&[0xDC00]);
    assert_eq!(lone_high, vec![0xED, 0xA0, 0x80, b'a']);
    let rel = RawRel::new(vec![
        b"\xff\xfeclip".to_vec(),
        lone_high.clone(),
        lone_low.clone(),
        "e\u{301}.mov".as_bytes().to_vec(),
    ])
    .unwrap();
    let v = serde_json::to_value(&rel).unwrap();
    assert_eq!(
        v,
        json!({"components": ["//5jbGlw", "7aCAYQ==", "7bCA", "ZcyBLm1vdg=="]})
    );
    let back: RawRel = serde_json::from_value(v).unwrap();
    assert_eq!(back, rel);
    // The raw bytes are untouched: no normalisation (NFD stays NFD), no decoding.
    assert_eq!(back.components()[3], "e\u{301}.mov".as_bytes());
    assert_eq!(
        to_wide(&back.components()[1]).unwrap(),
        vec![0xD800, u16::from(b'a')]
    );

    // Every u16 sequence round-trips through WTF-8.
    let cases: [&[u16]; 6] = [
        &[],
        &[0x41, 0xE9, 0x4E2D],
        &[0xD83C, 0xDFAC],         // a pair: one supplementary scalar
        &[0xDFAC, 0xD83C],         // reversed: two lone surrogates
        &[0xD800, 0xD800, 0x2E],   // two lone highs
        &[0xDBFF, 0xDFFF, 0xDC00], // max scalar, then a lone low
    ];
    for w in cases {
        let b = from_wide(w);
        assert_eq!(to_wide(&b).as_deref(), Some(w), "{w:x?}");
    }
    // A valid pair encodes as one 4-byte sequence, i.e. plain UTF-8.
    assert_eq!(from_wide(&[0xD83C, 0xDFAC]), "🎬".as_bytes());
    // Not WTF-8: invalid UTF-8, and a pair written as two 3-byte surrogates.
    assert_eq!(to_wide(b"\xff"), None);
    assert_eq!(to_wide(b"\xc0\x80"), None);
    assert_eq!(to_wide(&[0xED, 0xA0, 0xBC, 0xED, 0xBE, 0xAC]), None);
    assert_eq!(to_wide(b"\xe2\x82"), None);
}

#[test]
fn raw_rel_refuses_anything_but_single_names() {
    for bad in [
        json!({"components": [""]}),
        json!({"components": ["Lg=="]}), // "."
        json!({"components": ["Li4="]}), // ".."
        json!({"components": ["YS9i"]}), // "a/b"
        json!({"components": ["YQBi"]}), // "a\0b"
        json!({"components": ["not base64!"]}),
        json!({"parts": []}),
    ] {
        assert!(
            serde_json::from_value::<RawRel>(bad.clone()).is_err(),
            "{bad}"
        );
    }
    let root: RawRel = serde_json::from_value(json!({"components": []})).unwrap();
    assert!(root.is_root());
    assert_eq!(root.parent(), None);
    let a = root.join(b"a").join(b"\xff");
    assert_eq!(a.depth(), 2);
    assert_eq!(a.name(), Some(&b"\xff"[..]));
    assert_eq!(a.parent().unwrap(), root.join(b"a"));
    assert!(a.starts_with(&root.join(b"a")));
    assert_eq!(a.display(), "a/\u{FFFD}");
    // Backslashes are names on Unix, separators on Windows.
    assert_eq!(RawRel::new(vec![b"a\\b".to_vec()]).is_ok(), !cfg!(windows));
}

fn entry() -> ScanEntry {
    ScanEntry {
        dir: RawRel::root().join(b"Media"),
        name: b"\xffclip.mov".to_vec(),
        name_nfc: "\u{FFFD}clip.mov".into(),
        name_lossy: true,
        rel_key: "media/\u{FFFD}clip.mov".into(),
        entry_type: EntryType::File,
        size: 1 << 40,
        mtime_ns: 1_762_500_000_123_456_789,
        file_id: Some(vec![0; 16]),
        dev: Some("64769".into()),
        mount_root: false,
        automount: false,
        reparse_tag: Some(0x9000_001A),
        placeholder: Some(Placeholder::RecallOnData),
        excluded: None,
    }
}

#[test]
fn wire_forms_are_exact_in_javascript() {
    let e = entry();
    let v = serde_json::to_value(&e).unwrap();
    // Nanoseconds past 2^53 travel as strings; raw bytes as base64; absent options are absent.
    assert_eq!(v["mtimeNs"], json!("1762500000123456789"));
    assert_eq!(v["name"], json!("/2NsaXAubW92"));
    assert_eq!(v["fileId"], json!("AAAAAAAAAAAAAAAAAAAAAA=="));
    assert_eq!(v["placeholder"], json!("recallOnData"));
    assert_eq!(v["entryType"], json!("file"));
    assert!(v.get("excluded").is_none());
    assert_eq!(serde_json::from_value::<ScanEntry>(v).unwrap(), e);

    let batch = ScanBatch {
        seq: 7,
        entries: vec![e.clone()],
        dirs_done: vec![
            DirDone {
                dir: e.dir.clone(),
                mtime_ns: Some(-1),
                complete: true,
                code: None,
                entries: 1,
                volume: None,
            },
            DirDone {
                dir: RawRel::root(),
                mtime_ns: None,
                complete: false,
                code: Some("TIMEOUT".into()),
                entries: 0,
                volume: None,
            },
        ],
    };
    let line = serde_json::to_string(&batch).unwrap();
    assert!(line.contains(r#""dirsDone""#) && line.contains(r#""mtimeNs":"-1""#));
    assert_eq!(serde_json::from_str::<ScanBatch>(&line).unwrap(), batch);
    // Unknown fields and numeric nanoseconds are refused (the indexer validates batch lines).
    let mut bad: Value = serde_json::to_value(&e).unwrap();
    bad["extra"] = json!(1);
    assert!(serde_json::from_value::<ScanEntry>(bad).is_err());
    let mut bad: Value = serde_json::to_value(&e).unwrap();
    bad["mtimeNs"] = json!(5);
    assert!(serde_json::from_value::<ScanEntry>(bad).is_err());
    let mut bad: Value = serde_json::to_value(&e).unwrap();
    bad["mtimeNs"] = json!("+5");
    assert!(serde_json::from_value::<ScanEntry>(bad).is_err());

    let x = Expect {
        size: 10,
        mtime_ns: i64::MIN,
        file_id: None,
        dev: None,
    };
    let v = serde_json::to_value(&x).unwrap();
    assert_eq!(v, json!({"size": 10, "mtimeNs": "-9223372036854775808"}));
    assert_eq!(serde_json::from_value::<Expect>(v).unwrap(), x);
}

#[test]
fn expect_matches_by_size_mtime_and_file_id_when_both_have_one() {
    let x = Expect {
        size: 10,
        mtime_ns: 5,
        file_id: Some(vec![1]),
        dev: None,
    };
    assert!(x.matches(10, 5, Some(&[1])));
    assert!(x.matches(10, 5, None));
    assert!(!x.matches(10, 5, Some(&[2])));
    assert!(!x.matches(11, 5, Some(&[1])));
    assert!(!x.matches(10, 6, None));
    let no_id = Expect { file_id: None, ..x };
    assert!(no_id.matches(10, 5, Some(&[9])));
}

#[test]
fn root_state_and_identity_wire_forms() {
    let id = VolumeIdentity {
        key: "k".into(),
        kind: "smbfs".into(),
        dev_id: "1".into(),
        mount_root: true,
        case_sensitive: false,
        enumeration: "getattrlistbulk".into(),
        remote: true,
        root_file_id: Some(vec![2]),
        sample: vec![SampleEntry {
            file_id: None,
            size: 3,
            mtime_ns: 4,
        }],
    };
    let v = serde_json::to_value(RootState::Online(id.clone())).unwrap();
    assert_eq!(v["state"], json!("online"));
    assert_eq!(v["devId"], json!("1"));
    assert_eq!(v["rootFileId"], json!("Ag=="));
    assert_eq!(v["sample"], json!([{"size": 3, "mtimeNs": "4"}]));
    assert_eq!(
        serde_json::from_value::<RootState>(v).unwrap(),
        RootState::Online(id)
    );
    for (s, j) in [
        (
            RootState::Unreachable {
                code: "EHOSTDOWN".into(),
            },
            json!({"state": "unreachable", "code": "EHOSTDOWN"}),
        ),
        (RootState::Unmounted, json!({"state": "unmounted"})),
        (
            RootState::VolumeChanged { key: "k2".into() },
            json!({"state": "volumeChanged", "key": "k2"}),
        ),
        (RootState::Empty, json!({"state": "empty"})),
        (RootState::Timeout, json!({"state": "timeout"})),
    ] {
        assert_eq!(serde_json::to_value(&s).unwrap(), j);
        assert_eq!(serde_json::from_value::<RootState>(j).unwrap(), s);
    }
    let all = [
        Placeholder::Dataless,
        Placeholder::IcloudStub,
        Placeholder::Nextcloud,
        Placeholder::Owncloud,
        Placeholder::RecallOnOpen,
        Placeholder::RecallOnData,
        Placeholder::Offline,
        Placeholder::Unpinned,
    ];
    for p in all {
        assert_eq!(serde_json::to_value(p).unwrap(), json!(p.as_str()));
    }
}

#[test]
fn schemas_name_the_frozen_types() {
    let s = serde_json::to_value(schemars::schema_for!(ScanBatch)).unwrap();
    let defs = s["$defs"].as_object().unwrap();
    for name in ["ScanEntry", "DirDone", "RawRel", "EntryType", "Placeholder"] {
        assert!(defs.contains_key(name), "{name} in {defs:?}");
    }
    assert_eq!(
        defs["RawRel"]["properties"]["components"]["items"]["type"],
        json!("string")
    );
    assert_eq!(
        defs["ScanEntry"]["properties"]["mtimeNs"]["type"],
        json!("string")
    );
}

#[test]
fn unknown_fields_are_refused() {
    let r = serde_json::from_value::<RawRel>(json!({"components": ["YQ=="], "extra": 1}));
    assert!(r.is_err(), "{r:?}");
    for j in [
        json!({"state": "timeout", "extra": 1}),
        json!({"state": "unmounted", "extra": 1}),
        json!({"state": "online", "key": "k", "extra": 1}),
        json!({"state": "unreachable", "code": "IO", "extra": 1}),
        json!({"state": "volumeChanged", "key": "k", "extra": 1}),
    ] {
        let r = serde_json::from_value::<RootState>(j.clone());
        assert!(r.is_err(), "{j} → {r:?}");
    }
}
