//! Volume rules (plan 5D Task 1, Decisions 15, 17).

use fin_library::volume::{
    SAMPLE_MAX, is_mount_root, key, linux_case_sensitive, sample, sample_matches, strip_userinfo,
};
use fin_library::{
    EntryType, RawEntry, RootStat, RootState, SampleEntry, VolumeIdentity, classify_root,
};

fn s(id: u8, size: u64) -> SampleEntry {
    SampleEntry {
        file_id: Some(vec![id]),
        size,
        mtime_ns: i64::from(id) * 1000,
    }
}

fn samples(n: u8) -> Vec<SampleEntry> {
    (0..n).map(|i| s(i, u64::from(i) + 100)).collect()
}

fn now(key: &str) -> RootStat {
    RootStat {
        key: key.into(),
        kind: "smbfs".into(),
        dev_id: "50".into(),
        parent_dev_id: Some("1".into()),
        mount_root_attr: None,
        case_sensitive: false,
        enumeration: "getattrlistbulk".into(),
        remote: true,
        root_file_id: Some(vec![2]),
        sample: samples(16),
        entries: 40,
    }
}

fn stored() -> VolumeIdentity {
    now("k1").identity()
}

#[test]
fn root_state_classify() {
    let st = stored();
    assert!(st.mount_root);
    assert_eq!(
        classify_root(&st, &now("k1")),
        RootState::Online(now("k1").identity())
    );
    // Unmounted by losing the mount-root attribute (Linux)…
    let mut n = now("k1");
    n.mount_root_attr = Some(false);
    assert_eq!(classify_root(&st, &n), RootState::Unmounted);
    // …and by the device equalling the parent's (macOS, Windows): the empty mount point,
    // even when its key still matches, is never scanned.
    let mut n = now("k1");
    n.dev_id = "1".into();
    assert_eq!(classify_root(&st, &n), RootState::Unmounted);
    // A bind mount on the same device is still a mount root by the attribute.
    let mut n = now("k1");
    n.dev_id = "1".into();
    n.mount_root_attr = Some(true);
    assert!(matches!(classify_root(&st, &n), RootState::Online(_)));
    // A root that never was a mount root cannot unmount.
    let mut plain = stored();
    plain.mount_root = false;
    let mut n = now("k1");
    n.dev_id = "1".into();
    assert!(matches!(classify_root(&plain, &n), RootState::Online(_)));
    // Empty: no entries now, entries at the last measurement.
    let mut n = now("k1");
    n.entries = 0;
    n.sample.clear();
    assert_eq!(classify_root(&st, &n), RootState::Empty);
    let mut empty_at_add = stored();
    empty_at_add.sample.clear();
    assert!(matches!(
        classify_root(&empty_at_add, &n),
        RootState::Online(_)
    ));
    // The device id alone is never compared.
    let mut n = now("k1");
    n.dev_id = "77".into();
    assert!(matches!(classify_root(&st, &n), RootState::Online(id) if id.dev_id == "77"));

    assert!(is_mount_root(Some(true), "1", Some("1")));
    assert!(!is_mount_root(Some(false), "2", Some("1")));
    assert!(is_mount_root(None, "2", Some("1")));
    assert!(!is_mount_root(None, "1", Some("1")));
    assert!(is_mount_root(None, "1", None));
}

#[test]
fn volume_change_needs_confirmation() {
    let st = stored();
    // A new key with the same root id and sample: the same volume under a new source string.
    match classify_root(&st, &now("k2")) {
        RootState::Online(id) => assert_eq!(id.key, "k2"),
        other => panic!("{other:?}"),
    }
    // A different root id: changed, whatever the sample says.
    let mut n = now("k2");
    n.root_file_id = Some(vec![3]);
    assert_eq!(
        classify_root(&st, &n),
        RootState::VolumeChanged { key: "k2".into() }
    );
    // Fewer than half of the sample still matching: changed; exactly half: the same volume.
    let mut n = now("k2");
    n.sample = samples(7);
    assert_eq!(
        classify_root(&st, &n),
        RootState::VolumeChanged { key: "k2".into() }
    );
    n.sample = samples(8);
    assert!(matches!(classify_root(&st, &n), RootState::Online(_)));
    // Root ids unknown on either side: the sample decides alone.
    let mut n = now("k2");
    n.root_file_id = None;
    n.sample = samples(16).into_iter().skip(8).collect();
    assert!(matches!(classify_root(&st, &n), RootState::Online(_)));
    n.sample = samples(16).into_iter().skip(9).collect();
    assert!(matches!(
        classify_root(&st, &n),
        RootState::VolumeChanged { .. }
    ));
    // A volume with the same sizes and mtimes but other file ids does not match.
    let mut n = now("k2");
    n.sample = st
        .sample
        .iter()
        .map(|e| SampleEntry {
            file_id: Some(vec![200]),
            ..e.clone()
        })
        .collect();
    assert!(matches!(
        classify_root(&st, &n),
        RootState::VolumeChanged { .. }
    ));
    // An unchanged key is never questioned (a root id change alone is no volume change).
    let mut n = now("k1");
    n.root_file_id = Some(vec![9]);
    n.sample = samples(1);
    assert!(matches!(classify_root(&st, &n), RootState::Online(_)));
    // A changed volume that is empty: changed, not empty.
    let mut n = now("k3");
    n.entries = 0;
    n.sample.clear();
    assert_eq!(
        classify_root(&st, &n),
        RootState::VolumeChanged { key: "k3".into() }
    );
}

#[test]
fn sample_matching() {
    let a = vec![s(1, 10), s(2, 20)];
    assert_eq!(sample_matches(&a, &a), 2);
    // Ids are compared only when both sides have one; each entry matches once.
    let no_ids: Vec<SampleEntry> = a
        .iter()
        .map(|e| SampleEntry {
            file_id: None,
            ..e.clone()
        })
        .collect();
    assert_eq!(sample_matches(&a, &no_ids), 2);
    assert_eq!(sample_matches(&[s(1, 10), s(1, 10)], &[s(1, 10)]), 1);
    assert_eq!(sample_matches(&a, &[s(1, 11)]), 0);

    let mut entries: Vec<RawEntry> = (0..20u8)
        .rev()
        .map(|i| RawEntry::new(format!("n{i:02}"), EntryType::File, u64::from(i), 1))
        .collect();
    entries.push(RawEntry::new("a-link", EntryType::Symlink, 1, 1));
    entries.push(RawEntry::new("a-dir", EntryType::Dir, 0, 1));
    let got = sample(&entries);
    assert_eq!(got.len(), SAMPLE_MAX);
    // Sorted by raw name: the directory first, then n00…n14; the link is left out.
    assert_eq!(got[0].size, 0);
    assert_eq!(
        got[1..].iter().map(|e| e.size).collect::<Vec<_>>(),
        (0..15).collect::<Vec<u64>>()
    );
}

#[test]
fn keys_and_sources() {
    let k = key(&[b"smbfs", b"//nas/media"]);
    assert_eq!(k.len(), 32);
    assert!(k.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(k, key(&[b"smbfs", b"//nas/media"]));
    assert_ne!(key(&[b"ab", b"c"]), key(&[b"a", b"bc"]));
    assert_ne!(k, key(&[b"smbfs", b"//nas/media2"]));

    for (src, want) in [
        ("//user:secret@nas/media", "//nas/media"),
        ("//user;WORKGROUP@nas/media", "//nas/media"),
        ("//nas/media", "//nas/media"),
        ("//nas/a@b", "//nas/a@b"),
        ("user@host:/srv/media", "host:/srv/media"),
        ("nas:/export/media", "nas:/export/media"),
        ("/dev/sda1", "/dev/sda1"),
        ("", ""),
    ] {
        assert_eq!(strip_userinfo(src), want, "{src}");
    }

    for (t, o, cs) in [
        ("ext4", "rw,relatime", true),
        ("vfat", "rw", false),
        ("exfat", "rw", false),
        ("ntfs3", "rw", false),
        ("cifs", "rw,vers=3.1.1", true),
        ("cifs", "rw,nocase,vers=3.1.1", false),
        ("smb3", "ignorecase", false),
        ("nfs4", "rw", true),
    ] {
        assert_eq!(linux_case_sensitive(t, o), cs, "{t} {o}");
    }
}
