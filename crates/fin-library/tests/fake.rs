//! The fake source (plan 5D Task 1, feature `test-hooks`).

use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::time::{Duration, Instant};

use fin_library::source::fake::{FakeSource, FakeStats};
use fin_library::{
    Attrs, EntryType, Excludes, Expect, OpenError, Placeholder, RawRel, RootState, Source,
    classify_root, listing,
};
use serde_json::json;

fn spec() -> String {
    json!({
        "volume": { "key": "k1", "kind": "apfs", "devId": "7", "parentDevId": "1",
                    "caseSensitive": false, "rootFileId": "02" },
        "children": [
            { "name": "a.mov", "size": 10, "mtimeNs": 5, "fileId": "0a" },
            { "nameHex": "ff2e6d6f76", "content": "bytes", "mtimeNs": 6 },
            { "nameWide": [55296, 46, 109], "windowsAttributes": 32 },
            { "name": "cloud", "type": "dir", "macosFlags": 1073741824, "children": [
                { "name": "x.mov", "size": 3 } ] },
            { "name": "evicted.mov", "macosFlags": 1073741824, "size": 99, "mtimeNs": 1 },
            { "name": "link", "type": "symlink" },
            { "name": "junction", "type": "dir", "reparseTag": "A0000003", "children": [] },
            { "name": "net", "type": "dir", "automount": true, "children": [] },
            { "name": "disk2", "dev": "9", "mountRoot": true, "children": [
                { "name": "slow", "readDelayMs": 40, "children": [ { "name": "s.wav" } ] },
                { "name": "locked", "error": "permissionDenied", "children": [] },
                { "name": "gone", "openError": "notFound", "children": [] } ] }
        ]
    })
    .to_string()
}

#[test]
fn fake_source_reports_and_counts() {
    let src = FakeSource::from_json(&spec()).unwrap();
    let root = src.stat_root().unwrap();
    assert_eq!(
        (root.key.as_str(), root.kind.as_str(), root.entries),
        ("k1", "apfs", 9)
    );
    assert!(root.mount_root());
    assert_eq!(root.root_file_id, Some(vec![2]));
    assert_eq!(root.sample.len(), 8); // the symlink is not sampled
    assert!(matches!(
        classify_root(&root.identity(), &root),
        RootState::Online(_)
    ));

    let top = src.open_dir(&RawRel::root()).unwrap();
    let entries = src.read_dir(&top).unwrap();
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[0].name, b"a.mov");
    assert_eq!(entries[0].file_id, Some(vec![10]));
    assert_eq!(entries[0].dev.as_deref(), Some("7"));
    assert_eq!(entries[1].name, b"\xff.mov");
    assert_eq!(entries[1].size, 5);
    assert_eq!(entries[2].name, vec![0xED, 0xA0, 0x80, b'.', b'm']);
    assert_eq!(entries[2].attrs, Attrs::Windows { attributes: 32 });
    assert_eq!(entries[3].attrs, Attrs::Macos { flags: 1 << 30 });
    assert_eq!(entries[5].entry_type, EntryType::Symlink);
    assert_eq!(entries[6].reparse_tag, Some(0xA000_0003));
    assert!(entries[7].automount);
    assert!(entries[8].mount_root);

    // The listing rules over a fake listing: placeholders, links, automounts, case keys.
    let out = listing::entries(&RawRel::root(), entries, &Excludes::defaults(false));
    let cloud = out.iter().find(|e| e.name == b"cloud").unwrap();
    assert_eq!(cloud.placeholder, Some(Placeholder::Dataless));
    let junction = out.iter().find(|e| e.name == b"junction").unwrap();
    assert_eq!(junction.entry_type, EntryType::Symlink);
    let net = out.iter().find(|e| e.name == b"net").unwrap();
    assert_eq!(net.excluded.as_deref(), Some("AUTOMOUNT"));

    // Nested mount: its own device, inherited by its children.
    let disk2 = src.open_dir(&RawRel::root().join(b"disk2")).unwrap();
    let inner = src.read_dir(&disk2).unwrap();
    assert!(inner.iter().all(|e| e.dev.as_deref() == Some("9")));
    let v = src.volume(&disk2).unwrap();
    assert_eq!((v.dev_id.as_str(), v.mount_root), ("9", true));

    // Delays, errors.
    let slow = src
        .open_dir(&RawRel::root().join(b"disk2").join(b"slow"))
        .unwrap();
    let t = Instant::now();
    assert_eq!(src.read_dir(&slow).unwrap().len(), 1);
    assert!(t.elapsed() >= Duration::from_millis(40));
    let locked = src
        .open_dir(&RawRel::root().join(b"disk2").join(b"locked"))
        .unwrap();
    assert_eq!(
        src.read_dir(&locked).unwrap_err().kind(),
        ErrorKind::PermissionDenied
    );
    let gone = src.open_dir(&RawRel::root().join(b"disk2").join(b"gone"));
    assert_eq!(gone.unwrap_err().kind(), ErrorKind::NotFound);
    assert_eq!(
        src.open_dir(&RawRel::root().join(b"nope"))
            .unwrap_err()
            .kind(),
        ErrorKind::NotFound
    );
    for not_dir in [&b"a.mov"[..], b"link", b"junction"] {
        let e = src.open_dir(&RawRel::root().join(not_dir)).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::NotADirectory);
    }

    // Nothing so far touched a placeholder or an automount.
    let st = src.stats();
    assert_eq!(
        (st.placeholder_touches, st.automount_triggers, st.file_opens),
        (0, 0, 0)
    );
    assert_eq!(st.stat_roots, 1);
    assert_eq!(st.dir_reads, 4);
    assert_eq!(st.opens_by_dir[&RawRel::root().join(b"disk2")], 1);

    // A walk that wrongly descends counts it.
    let c = src.open_dir(&RawRel::root().join(b"cloud")).unwrap();
    src.read_dir(&c).unwrap();
    src.open_dir(&RawRel::root().join(b"net")).unwrap();
    let st = src.stats();
    assert_eq!((st.placeholder_touches, st.automount_triggers), (2, 1));
}

#[test]
fn fake_open_file_follows_the_opener_rules() {
    let src = FakeSource::from_json(&spec()).unwrap();
    let x = |size, mtime_ns, file_id: Option<Vec<u8>>| Expect {
        size,
        mtime_ns,
        file_id,
    };
    let rel = RawRel::root().join(b"\xff.mov");
    let mut f = src.open_file(&rel, &x(5, 6, None), false).unwrap();
    let mut s = String::new();
    f.read_to_string(&mut s).unwrap();
    assert_eq!(s, "bytes");
    f.seek(SeekFrom::Start(2)).unwrap();
    let mut b = [0u8; 8];
    assert_eq!(f.read(&mut b).unwrap(), 3);
    assert_eq!(&b[..3], b"tes");
    // Without content: `size` zero bytes.
    let mut f = src
        .open_file(
            &RawRel::root().join(b"a.mov"),
            &x(10, 5, Some(vec![10])),
            false,
        )
        .unwrap();
    let mut all = Vec::new();
    f.read_to_end(&mut all).unwrap();
    assert_eq!(all, vec![0; 10]);

    let a = RawRel::root().join(b"a.mov");
    assert!(matches!(
        src.open_file(&a, &x(10, 5, Some(vec![11])), false),
        Err(OpenError::Changed)
    ));
    assert!(matches!(
        src.open_file(&a, &x(11, 5, None), false),
        Err(OpenError::Changed)
    ));
    let evicted = RawRel::root().join(b"evicted.mov");
    assert!(matches!(
        src.open_file(&evicted, &x(99, 1, None), false),
        Err(OpenError::CloudOnly)
    ));
    let under_dataless = RawRel::root().join(b"cloud").join(b"x.mov");
    assert!(matches!(
        src.open_file(&under_dataless, &x(3, 0, None), false),
        Err(OpenError::CloudOnly)
    ));
    assert!(matches!(
        src.open_file(&RawRel::root().join(b"link"), &x(0, 0, None), false),
        Err(OpenError::NameSurrogate)
    ));
    assert!(matches!(
        src.open_file(&RawRel::root().join(b"missing"), &x(0, 0, None), false),
        Err(OpenError::NotFound)
    ));
    assert!(matches!(
        src.open_file(&RawRel(vec![b"..".to_vec()]), &x(0, 0, None), false),
        Err(OpenError::Escapes)
    ));
    assert_eq!(src.stats().placeholder_touches, 0);
    assert_eq!(src.stats().file_opens, 2);
    // With hydrate, a placeholder is read, and counted.
    src.open_file(&evicted, &x(99, 1, None), true).unwrap();
    assert_eq!(src.stats().placeholder_touches, 1);
}

#[test]
fn fake_reload_keeps_counters_and_injects_root_faults() {
    let src = FakeSource::from_json(&spec()).unwrap();
    src.stat_root().unwrap();
    let unmounted = json!({
        "volume": { "key": "k1", "devId": "1", "parentDevId": "1" },
        "statDelayMs": 30,
        "children": []
    });
    src.reload(&unmounted.to_string()).unwrap();
    let t = Instant::now();
    let now = src.stat_root().unwrap();
    assert!(t.elapsed() >= Duration::from_millis(30));
    let stored = FakeSource::from_json(&spec())
        .unwrap()
        .stat_root()
        .unwrap()
        .identity();
    assert_eq!(classify_root(&stored, &now), RootState::Unmounted);
    assert_eq!(src.stats().stat_roots, 2);

    let dead = json!({ "volume": { "key": "k1" }, "statError": "timedOut", "error": "io" });
    src.reload(&dead.to_string()).unwrap();
    assert_eq!(src.stat_root().unwrap_err().kind(), ErrorKind::TimedOut);
    let top = src.open_dir(&RawRel::root()).unwrap();
    assert_eq!(src.read_dir(&top).unwrap_err().kind(), ErrorKind::Other);
    assert_eq!(
        src.stats(),
        FakeStats {
            stat_roots: 3,
            dir_opens: 1,
            dir_reads: 1,
            opens_by_dir: [(RawRel::root(), 1)].into(),
            reads_by_dir: [(RawRel::root(), 1)].into(),
            ..FakeStats::default()
        }
    );

    for bad in [
        json!({ "volume": { "key": "k" }, "extra": 1 }),
        json!({ "children": [] }),
        json!({ "volume": { "key": "k" }, "children": [ { "name": "a", "nameHex": "61" } ] }),
        json!({ "volume": { "key": "k" }, "children": [ {} ] }),
        json!({ "volume": { "key": "k" }, "children": [ { "name": "a/b" } ] }),
        json!({ "volume": { "key": "k" }, "children": [ { "nameHex": "6" } ] }),
        json!({ "volume": { "key": "k" }, "children": [ { "name": "a", "error": "boom" } ] }),
        json!({ "volume": { "key": "k" }, "children": [ { "name": "a", "macosFlags": 1, "windowsAttributes": 1 } ] }),
    ] {
        assert!(FakeSource::from_json(&bad.to_string()).is_err(), "{bad}");
    }
}
