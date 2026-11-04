use super::*;
use std::path::PathBuf;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn macos_path_table() {
    let home = p("/Users/Zoë Müller");
    let cases: &[(&str, Option<Provider>)] = &[
        (
            "/Users/Zoë Müller/Library/Mobile Documents/com~apple~CloudDocs/Edit",
            Some(Provider::ICloud),
        ),
        (
            "/Users/Zoë Müller/Library/Mobile Documents",
            Some(Provider::ICloud),
        ),
        (
            "/users/zoë müller/library/mobile documents/x",
            Some(Provider::ICloud),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/OneDrive-Personal/My Film/",
            Some(Provider::OneDrive),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/OneDrive-Contoso Ltd/a",
            Some(Provider::OneDrive),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/GoogleDrive-me@example.com/My Drive/p",
            Some(Provider::GoogleDrive),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/Dropbox/p",
            Some(Provider::Dropbox),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/Dropbox-Work/p",
            Some(Provider::Dropbox),
        ),
        (
            "/Users/Zoë Müller/Library/CloudStorage/SomethingNew-x/p",
            Some(Provider::Unknown),
        ),
        ("/Users/Zoë Müller/Library/CloudStorage", None),
        ("/Users/Zoë Müller/Library/Caches/x", None),
        ("/Users/Zoë Müller/Movies/OneDrive-Personal", None),
        ("/Users/Zoë Müller2/Library/Mobile Documents/x", None),
        ("/Volumes/Share/Library/Mobile Documents", None),
    ];
    for (path, want) in cases {
        assert_eq!(classify_macos_path(&p(path), &home), *want, "{path}");
    }
    // A trailing slash on home or path changes nothing.
    assert_eq!(
        classify_macos_path(&p("/Users/a/Library/Mobile Documents/"), &p("/Users/a/")),
        Some(Provider::ICloud)
    );
    assert_eq!(classify_macos_path(&p("/x"), &p("")), None);
}

#[test]
fn macos_fs_table() {
    for n in ["smbfs", "nfs", "afpfs", "webdav", "SMBFS"] {
        assert_eq!(classify_macos_fs(n), StorageKind::Network, "{n}");
    }
    for n in ["apfs", "hfs", "exfat", "msdos", "", "ntfs", "macfuse"] {
        assert_eq!(classify_macos_fs(n), StorageKind::Local, "{n}");
    }
}

#[test]
fn linux_fs_table() {
    let none = SyncMarkers::default();
    for t in [
        SMB2_MAGIC_NUMBER,
        CIFS_MAGIC_NUMBER,
        NFS_SUPER_MAGIC,
        SMB_SUPER_MAGIC,
        FUSE_SUPER_MAGIC,
    ] {
        assert_eq!(classify_linux_fs(t, none), StorageKind::Network, "{t:#x}");
    }
    for t in [0xEF53u64, 0x9123_683E, 0x0102_1994, 0x5846_5342, 0] {
        assert_eq!(classify_linux_fs(t, none), StorageKind::Local, "{t:#x}");
    }
    let st = SyncMarkers {
        syncthing: true,
        dropbox: false,
    };
    let db = SyncMarkers {
        syncthing: false,
        dropbox: true,
    };
    let both = SyncMarkers {
        syncthing: true,
        dropbox: true,
    };
    assert_eq!(
        classify_linux_fs(0xEF53, st),
        StorageKind::Sync(Provider::Syncthing)
    );
    assert_eq!(
        classify_linux_fs(0xEF53, db),
        StorageKind::Sync(Provider::Dropbox)
    );
    assert_eq!(
        classify_linux_fs(0xEF53, both),
        StorageKind::Sync(Provider::Syncthing)
    );
    // Sync wins over the file system, including FUSE mounts such as the Dropbox client's.
    assert_eq!(
        classify_linux_fs(FUSE_SUPER_MAGIC, db),
        StorageKind::Sync(Provider::Dropbox)
    );
    assert_eq!(
        classify_linux_fs(NFS_SUPER_MAGIC, st),
        StorageKind::Sync(Provider::Syncthing)
    );
}

#[test]
fn markers_look_at_every_ancestor() {
    let exists = |want: &'static str| move |q: &Path| q == Path::new(want);
    let deep = p("/home/a/Sync/Films/Ünï cödé/proj");
    assert_eq!(
        find_markers(&deep, exists("/home/a/Sync/.stfolder")),
        SyncMarkers {
            syncthing: true,
            dropbox: false
        }
    );
    assert_eq!(
        find_markers(&deep, exists("/home/a/.dropbox")),
        SyncMarkers {
            syncthing: false,
            dropbox: true
        }
    );
    assert_eq!(
        find_markers(&deep, exists("/home/b/.dropbox")),
        SyncMarkers::default()
    );
    assert_eq!(
        find_markers(&deep, exists("/home/a/Sync/Films/proj/.stfolder")),
        SyncMarkers::default()
    );
    assert_eq!(
        find_markers(&p("/"), exists("/.stfolder")).provider(),
        Some(Provider::Syncthing)
    );
}

fn win(path: &str, drive: Option<u32>, attrs: Option<u32>) -> StorageKind {
    let od = vec![
        r"C:\Users\Zoë\OneDrive".to_string(),
        r"C:\Users\Zoë\OneDrive - Contoso".to_string(),
    ];
    let db = vec![r"C:\Users\Zoë\Dropbox (Personal)".to_string()];
    classify_windows(path, &od, &db, drive, attrs)
}

#[test]
fn windows_table() {
    use StorageKind::*;
    let cases: &[(&str, Option<u32>, Option<u32>, StorageKind)] = &[
        (r"C:\Projects\Film", Some(3), Some(0x10), Local),
        (r"D:\Projects", Some(2), None, Local),
        (r"\\nas\share\Film", Some(3), None, Network),
        (r"//nas/share/Film", None, None, Network),
        (r"\\?\UNC\nas\share\Film", None, None, Network),
        (r"\\?\C:\Projects", Some(3), None, Local),
        (r"\\.\C:\Projects", Some(3), None, Local),
        (r"Z:\Film", Some(DRIVE_REMOTE), None, Network),
        (
            r"c:\users\zoë\onedrive\Film\",
            Some(3),
            None,
            Sync(Provider::OneDrive),
        ),
        (
            r"C:\USERS\ZOË\ONEDRIVE",
            Some(3),
            None,
            Sync(Provider::OneDrive),
        ),
        (
            r"\\?\C:\Users\Zoë\OneDrive - Contoso\a b\c",
            Some(3),
            None,
            Sync(Provider::OneDrive),
        ),
        (r"C:\Users\Zoë\OneDrive2\Film", Some(3), None, Local),
        (r"C:\Users\Zoë\OneDriveX", Some(3), None, Local),
        (
            r"C:\Users\Zoë\Dropbox (Personal)\Film",
            Some(3),
            None,
            Sync(Provider::Dropbox),
        ),
        (
            r"C:\Users\Zoë\Dropbox (Personal)",
            Some(3),
            None,
            Sync(Provider::Dropbox),
        ),
        (r"C:\Users\Zoë\Dropbox\Film", Some(3), None, Local),
        (
            r"E:\Cloud\Film",
            Some(3),
            Some(FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS),
            Sync(Provider::Unknown),
        ),
        (
            r"E:\Cloud\Film",
            Some(3),
            Some(FILE_ATTRIBUTE_RECALL_ON_OPEN | 0x10),
            Sync(Provider::Unknown),
        ),
        (r"E:\Cloud\Film", Some(3), Some(0x1000), Local),
    ];
    for (path, drive, attrs, want) in cases {
        assert_eq!(win(path, *drive, *attrs), *want, "{path}");
    }
    // No roots configured and nothing known: Local.
    assert_eq!(
        classify_windows(r"C:\a", &[], &[String::new()], None, None),
        Local
    );
    assert_eq!(
        classify_windows(r"C:\a", &[String::new()], &[], None, None),
        Local,
        "an empty root matches nothing"
    );
}

#[test]
fn dropbox_info_json() {
    let json = r#"{"personal":{"path":"C:\\Users\\Zoë\\Dropbox","host":1},
                   "business":{"path":"D:\\Work Dropbox","is_team":true}}"#;
    let mut roots = parse_dropbox_info(json);
    roots.sort();
    assert_eq!(roots, [r"C:\Users\Zoë\Dropbox", r"D:\Work Dropbox"]);
    for bad in [
        "",
        "[]",
        "{",
        r#"{"personal":{}}"#,
        r#"{"personal":{"path":3}}"#,
        "null",
    ] {
        assert!(parse_dropbox_info(bad).is_empty(), "{bad}");
    }
}

#[test]
fn serialisation_roundtrip() {
    let all = [
        (StorageKind::Local, "local"),
        (StorageKind::Network, "network"),
        (StorageKind::Sync(Provider::ICloud), "sync:icloud"),
        (StorageKind::Sync(Provider::OneDrive), "sync:onedrive"),
        (StorageKind::Sync(Provider::GoogleDrive), "sync:googledrive"),
        (StorageKind::Sync(Provider::Dropbox), "sync:dropbox"),
        (StorageKind::Sync(Provider::Syncthing), "sync:syncthing"),
        (StorageKind::Sync(Provider::Unknown), "sync:unknown"),
    ];
    for (kind, text) in all {
        assert_eq!(kind.to_string(), text);
        assert_eq!(text.parse::<StorageKind>().unwrap(), kind);
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{text}\""));
        assert_eq!(serde_json::from_str::<StorageKind>(&json).unwrap(), kind);
    }
    assert_eq!(
        " Sync:OneDrive ".parse::<StorageKind>().unwrap(),
        StorageKind::Sync(Provider::OneDrive)
    );
    for bad in [
        "", "sync", "sync:", "sync:box", "cloud", "local:x", "Network1",
    ] {
        assert!(bad.parse::<StorageKind>().is_err(), "{bad:?}");
    }
    assert!(serde_json::from_str::<StorageKind>("\"nope\"").is_err());
}

#[test]
fn override_wins() {
    let dir = tempfile::tempdir().unwrap();
    for kind in [
        StorageKind::Network,
        StorageKind::Sync(Provider::Dropbox),
        StorageKind::Local,
    ] {
        assert_eq!(detect(dir.path(), Some(kind)), kind);
    }
    assert_eq!(
        resolve(dir.path(), Some("sync:icloud")),
        (StorageKind::Sync(Provider::ICloud), None)
    );
    let (kind, warning) = resolve(dir.path(), Some("floppy"));
    assert_eq!(kind, detect(dir.path(), None));
    assert!(warning.unwrap().contains("floppy"));
    assert_eq!(resolve(dir.path(), None).1, None);
}

#[test]
fn detect_never_fails_on_a_missing_folder() {
    // Whatever the platform answers, it answers.
    let _ = detect(Path::new("/definitely/not/here"), None);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_probe_of_a_temp_dir_is_local() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(detect(dir.path(), None), StorageKind::Local);
}

#[cfg(target_os = "linux")]
#[test]
fn linux_probe_sees_a_syncthing_marker() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join(".stfolder")).unwrap();
    let nested = dir.path().join("a b").join("proj");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(
        detect(&nested, None),
        StorageKind::Sync(Provider::Syncthing)
    );
    std::fs::File::create(dir.path().join(".dropbox")).unwrap();
    assert_eq!(
        detect(&nested, None),
        StorageKind::Sync(Provider::Syncthing)
    );
}
