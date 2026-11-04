use super::*;
use std::path::PathBuf;

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

#[test]
fn macos_path_table() {
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
        // Not trusting $HOME: a relocated home or a FileVault volume path still matches.
        (
            "/Volumes/Data/home/a/Library/Mobile Documents/x",
            Some(Provider::ICloud),
        ),
        ("/Library/CloudStorage/Box-Box/p", Some(Provider::Unknown)),
        ("/Users/Zoë Müller/Library/CloudStorage", None),
        ("/Users/Zoë Müller/Library/Caches/x", None),
        ("/Users/Zoë Müller/Movies/OneDrive-Personal", None),
        ("/Users/Zoë Müller/Mobile Documents/x", None),
    ];
    for (path, want) in cases {
        assert_eq!(classify_macos_path(&p(path)), *want, "{path}");
    }
    assert_eq!(
        classify_macos_path(&p("/Users/a/Library/Mobile Documents/")),
        Some(Provider::ICloud)
    );
}

#[test]
fn path_under_table() {
    assert!(path_under(&p("/a/B/c"), &p("/a/b"), true));
    assert!(!path_under(&p("/a/B/c"), &p("/a/b"), false));
    assert!(path_under(&p("/a/b/"), &p("/a/b"), false));
    assert!(path_under(&p("/a/b"), &p("/a/b/"), false));
    assert!(!path_under(&p("/a/bc"), &p("/a/b"), false));
    assert!(!path_under(&p("/a"), &p("/a/b"), false));
    assert!(
        !path_under(&p("/a"), &p(""), false),
        "an empty root matches nothing"
    );
    assert!(path_under(&p("/Ünï/x y/z"), &p("/ünï/x y"), true));
}

#[test]
fn icloud_desktop_and_documents_redirect() {
    // ~/Documents and ~/Desktop are symlinks out of CloudDocs; the wrapper canonicalises the
    // CloudDocs entries and this decides.
    let targets = vec![
        p("/Volumes/Data/Users/a/Library/Mobile Documents/com~apple~CloudDocs/Documents"),
        p("/mnt/elsewhere/Desk"),
    ];
    assert_eq!(
        classify_icloud_redirect(
            &p("/Volumes/Data/Users/a/Library/Mobile Documents/com~apple~CloudDocs/Documents/Film"),
            &targets
        ),
        Some(Provider::ICloud)
    );
    assert_eq!(
        classify_icloud_redirect(&p("/mnt/ELSEWHERE/desk/x"), &targets),
        Some(Provider::ICloud)
    );
    assert_eq!(
        classify_icloud_redirect(&p("/mnt/elsewhere/Other"), &targets),
        None
    );
    assert_eq!(classify_icloud_redirect(&p("/x"), &[]), None);
}

#[test]
fn dataless_flag() {
    assert!(is_dataless(SF_DATALESS));
    assert!(is_dataless(SF_DATALESS | 0x1));
    assert!(!is_dataless(0));
    assert!(!is_dataless(0x0000_0020));
}

#[test]
fn home_folder_names() {
    let home = p("/home/Zoë");
    let cases: &[(&str, Option<Provider>)] = &[
        ("/home/Zoë/Dropbox/Film", Some(Provider::Dropbox)),
        ("/home/Zoë/Dropbox (Personal)", Some(Provider::Dropbox)),
        ("/home/Zoë/OneDrive - Contoso/x", Some(Provider::OneDrive)),
        ("/home/Zoë/Google Drive/x", Some(Provider::GoogleDrive)),
        ("/home/Zoë/My Drive/x", Some(Provider::GoogleDrive)),
        ("/home/Zoë/iCloudDrive/x", Some(Provider::ICloud)),
        ("/home/Zoë/Nextcloud/x", Some(Provider::Unknown)),
        ("/home/Zoë/ownCloud", Some(Provider::Unknown)),
        ("/home/Zoë/MEGA/x", Some(Provider::Unknown)),
        ("/home/Zoë/Box/x", Some(Provider::Unknown)),
        ("/home/zoë/dropbox/x", Some(Provider::Dropbox)),
        ("/home/Zoë/Projects/Dropbox", None),
        ("/home/Zoë/Megaman/x", None),
        ("/home/Zoë/Boxes/x", None),
        ("/home/Zoë", None),
        ("/home/other/Dropbox", None),
    ];
    for (path, want) in cases {
        assert_eq!(classify_home_folder(&p(path), &home, true), *want, "{path}");
    }
    assert_eq!(
        classify_home_folder(&p("/home/zoë/Dropbox"), &home, false),
        None
    );
}

#[test]
fn provider_names() {
    for (n, want) in [
        ("OneDrive - Contoso", Provider::OneDrive),
        ("OneDrive", Provider::OneDrive),
        ("Dropbox", Provider::Dropbox),
        ("Google Drive", Provider::GoogleDrive),
        ("GoogleDriveFS", Provider::GoogleDrive),
        ("iCloud Drive", Provider::ICloud),
        ("Apple.iCloud", Provider::ICloud),
        ("Box", Provider::Unknown),
        ("", Provider::Unknown),
    ] {
        assert_eq!(provider_from_name(n), want, "{n:?}");
    }
}

#[test]
fn macos_fs_table() {
    for n in ["smbfs", "nfs", "afpfs", "webdav", "SMBFS"] {
        assert_eq!(classify_macos_fs(n), StorageKind::Network, "{n}");
    }
    for n in ["macfuse", "osxfuse", "fuse", "macfuse_fs"] {
        assert_eq!(
            classify_macos_fs(n),
            StorageKind::Sync(Provider::Unknown),
            "{n}"
        );
    }
    for n in ["apfs", "hfs", "exfat", "msdos", "", "ntfs"] {
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
        NINEP_MAGIC,
        CEPH_SUPER_MAGIC,
        AFS_SUPER_MAGIC,
    ] {
        assert_eq!(
            classify_linux_fs(t, none, None),
            StorageKind::Network,
            "{t:#x}"
        );
    }
    for t in [0xEF53u64, 0x9123_683E, 0x0102_1994, 0x5846_5342, 0] {
        assert_eq!(
            classify_linux_fs(t, none, None),
            StorageKind::Local,
            "{t:#x}"
        );
    }
    // FUSE: sshfs is a remote share, a block-backed FUSE (ntfs-3g) is a local disk, and
    // everything else (rclone, Dropbox, gvfs, unknown) is treated as a sync folder.
    let fuse = |sub| classify_linux_fs(FUSE_SUPER_MAGIC, none, sub);
    assert_eq!(fuse(Some("fuse.sshfs")), StorageKind::Network);
    assert_eq!(fuse(Some("fuseblk")), StorageKind::Local);
    assert_eq!(
        fuse(Some("fuse.rclone")),
        StorageKind::Sync(Provider::Unknown)
    );
    assert_eq!(fuse(Some("fuse")), StorageKind::Sync(Provider::Unknown));
    assert_eq!(fuse(None), StorageKind::Sync(Provider::Unknown));
    let st = SyncMarkers {
        syncthing: true,
        ..none
    };
    let db = SyncMarkers {
        dropbox: true,
        ..none
    };
    let other = SyncMarkers {
        other: true,
        ..none
    };
    let both = SyncMarkers {
        syncthing: true,
        dropbox: true,
        other: true,
    };
    assert_eq!(
        classify_linux_fs(0xEF53, st, None),
        StorageKind::Sync(Provider::Syncthing)
    );
    assert_eq!(
        classify_linux_fs(0xEF53, db, None),
        StorageKind::Sync(Provider::Dropbox)
    );
    assert_eq!(
        classify_linux_fs(0xEF53, other, None),
        StorageKind::Sync(Provider::Unknown)
    );
    assert_eq!(
        classify_linux_fs(0xEF53, both, None),
        StorageKind::Sync(Provider::Syncthing)
    );
    assert_eq!(
        classify_linux_fs(FUSE_SUPER_MAGIC, db, Some("fuse.sshfs")),
        StorageKind::Sync(Provider::Dropbox)
    );
    assert_eq!(
        classify_linux_fs(NFS_SUPER_MAGIC, st, None),
        StorageKind::Sync(Provider::Syncthing)
    );
}

const MOUNTINFO: &str = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
40 22 0:35 / /home/a/mnt/My\\040Share rw,nosuid shared:5 - fuse.sshfs a@host:/srv rw,user_id=1000
41 22 0:36 / /home/a/Dropbox rw shared:6 - fuse Dropbox rw
42 22 8:3 / /media/disk rw - fuseblk /dev/sdb1 rw
43 40 0:37 / /home/a/mnt/My\\040Share/nested rw - nfs4 srv:/x rw
";

#[test]
fn mountinfo_picks_the_deepest_mount() {
    let f = |path: &str| mount_fstype(MOUNTINFO, &p(path));
    assert_eq!(f("/home/a/mnt/My Share/film"), Some("fuse.sshfs".into()));
    assert_eq!(f("/home/a/mnt/My Share"), Some("fuse.sshfs".into()));
    assert_eq!(f("/home/a/mnt/My Share/nested/x"), Some("nfs4".into()));
    assert_eq!(f("/home/a/Dropbox/x"), Some("fuse".into()));
    assert_eq!(f("/media/disk/x"), Some("fuseblk".into()));
    assert_eq!(f("/home/a/other"), Some("ext4".into()));
    assert_eq!(f("/home/a/mnt/My Sharex"), Some("ext4".into()));
    assert_eq!(mount_fstype("garbage\n\n", &p("/x")), None);
    assert_eq!(mount_fstype("", &p("/x")), None);
}

#[test]
fn wsl_drive_mount_paths() {
    let f = |s: &str| classify_wsl_path(&p(s));
    assert_eq!(
        f("/mnt/c/Users/Zoë/OneDrive - Contoso/Film"),
        Some(Provider::OneDrive)
    );
    assert_eq!(f("/mnt/d/Users/Zoë/Dropbox/x"), Some(Provider::Dropbox));
    assert_eq!(
        f("/mnt/c/Users/Zoë/My Drive/x"),
        Some(Provider::GoogleDrive)
    );
    assert_eq!(
        f("/mnt/c/Users/Zoë/AppData/Local/Google/DriveFS/x"),
        Some(Provider::GoogleDrive)
    );
    assert_eq!(f("/mnt/c/Users/Zoë/Documents/x"), None);
    assert_eq!(f("/mnt/cc/Users/Zoë/OneDrive/x"), None);
    assert_eq!(f("/home/Zoë/OneDrive"), None);
}

#[test]
fn markers_look_at_every_ancestor() {
    // `stat`: Some(is_dir) for the listed entries; `list`: names in a directory.
    let finder = |entries: &'static [(&'static str, bool)]| {
        move |path: &Path| {
            find_markers(
                path,
                |q| {
                    entries
                        .iter()
                        .find(|(e, _)| Path::new(e) == q)
                        .map(|(_, d)| *d)
                },
                |dir| {
                    entries
                        .iter()
                        .filter_map(|(e, _)| Path::new(e).strip_prefix(dir).ok())
                        .filter(|r| r.components().count() == 1)
                        .map(|r| r.to_string_lossy().into_owned())
                        .collect()
                },
            )
        }
    };
    let deep = p("/home/a/Sync/Films/Ünï cödé/proj");
    let m = |entries| finder(entries)(&deep);
    assert_eq!(
        m(&[("/home/a/Sync/.stfolder", true)]).provider(),
        Some(Provider::Syncthing)
    );
    assert_eq!(
        m(&[("/home/a/Sync/.stignore", false)]).provider(),
        Some(Provider::Syncthing)
    );
    assert_eq!(
        m(&[("/home/a/Sync/.stversions", true)]).provider(),
        Some(Provider::Syncthing)
    );
    // `.dropbox` counts only as a regular file: ~/.dropbox is the client's config directory.
    assert_eq!(
        m(&[("/home/a/.dropbox", false)]).provider(),
        Some(Provider::Dropbox)
    );
    assert_eq!(m(&[("/home/a/.dropbox", true)]).provider(), None);
    assert_eq!(
        m(&[
            ("/home/a/.dropbox", true),
            ("/home/a/.dropbox/info.json", false)
        ])
        .provider(),
        None
    );
    assert_eq!(
        m(&[("/home/a/Sync/.sync_1234abcd.db", false)]).provider(),
        Some(Provider::Unknown)
    );
    assert_eq!(
        m(&[("/home/a/Sync/._sync_1234abcd.db", false)]).provider(),
        Some(Provider::Unknown)
    );
    assert_eq!(
        m(&[("/home/a/Sync/.sync", true)]).provider(),
        Some(Provider::Unknown)
    );
    assert_eq!(
        m(&[("/home/a/.megaignore", false)]).provider(),
        Some(Provider::Unknown)
    );
    assert_eq!(
        m(&[("/home/a/.debris", true)]).provider(),
        Some(Provider::Unknown)
    );
    assert_eq!(m(&[("/home/b/.stfolder", true)]).provider(), None);
    assert_eq!(
        m(&[("/home/a/Sync/Films/proj/.stfolder", true)]).provider(),
        None,
        "only ancestors"
    );
    assert_eq!(
        m(&[
            ("/home/a/Sync/.syncfoo", false),
            ("/home/a/Sync/sync.db", false)
        ])
        .provider(),
        None
    );
    assert_eq!(
        finder(&[("/.stfolder", true)])(&p("/")).provider(),
        Some(Provider::Syncthing)
    );
}

fn facts() -> WinFacts {
    WinFacts {
        onedrive_roots: vec![
            r"C:\Users\Zoë\OneDrive".into(),
            r"C:\Users\Zoë\OneDrive - Contoso".into(),
            String::new(),
        ],
        dropbox_roots: vec![r"C:\Users\Zoë\Dropbox (Personal)".into()],
        user_profile: Some(r"C:\Users\Zoë".into()),
        local_app_data: Some(r"C:\Users\Zoë\AppData\Local".into()),
        drive_type: Some(3),
        ..WinFacts::default()
    }
}

fn win(path: &str, f: impl FnOnce(&mut WinFacts)) -> StorageKind {
    let mut w = facts();
    f(&mut w);
    classify_windows(path, &w)
}

#[test]
fn windows_table() {
    use StorageKind::*;
    let same = |_: &mut WinFacts| {};
    let cases: &[(&str, StorageKind)] = &[
        (r"C:\Projects\Film", Local),
        (r"D:\Projects", Local),
        (r"\\nas\share\Film", Network),
        (r"//nas/share/Film", Network),
        (r"\\?\UNC\nas\share\Film", Network),
        (r"\\?\C:\Projects", Local),
        (r"\\.\C:\Projects", Local),
        (r"c:\users\zoë\onedrive\Film\", Sync(Provider::OneDrive)),
        (r"C:\USERS\ZOË\ONEDRIVE", Sync(Provider::OneDrive)),
        (
            r"\\?\C:\Users\Zoë\OneDrive - Contoso\a b\c",
            Sync(Provider::OneDrive),
        ),
        (r"C:\Users\Zoë\OneDrive2\Film", Sync(Provider::OneDrive)),
        (
            r"C:\Users\Zoë\Dropbox (Personal)\Film",
            Sync(Provider::Dropbox),
        ),
        (r"C:\Users\Zoë\Dropbox\Film", Sync(Provider::Dropbox)),
        (r"C:\Users\Zoë\My Drive\Film", Sync(Provider::GoogleDrive)),
        (
            r"C:\Users\Zoë\Google Drive\Film",
            Sync(Provider::GoogleDrive),
        ),
        (
            r"C:\Users\Zoë\AppData\Local\Google\DriveFS\123\mirror",
            Sync(Provider::GoogleDrive),
        ),
        (r"C:\Users\Zoë\iCloudDrive\Film", Sync(Provider::ICloud)),
        (r"C:\Users\Zoë\Box\Film", Sync(Provider::Unknown)),
        (r"C:\Users\Zoë\Nextcloud\Film", Sync(Provider::Unknown)),
        (r"C:\Users\Zoë\Documents\Film", Local),
        (r"C:\Users\Zoë\Projects\Dropbox", Local),
        (r"C:\Users\Other\Dropbox", Local),
    ];
    for (path, want) in cases {
        assert_eq!(win(path, same), *want, "{path}");
    }
    // Drive type: remote is network; folder-mounted volumes use their own drive type.
    assert_eq!(
        win(r"Z:\Film", |w| w.drive_type = Some(DRIVE_REMOTE)),
        Network
    );
    assert_eq!(
        win(r"C:\mnt\nas", |w| w.drive_type = Some(DRIVE_REMOTE)),
        Network
    );
    assert_eq!(
        win(r"C:\a", |w| w.drive_type = Some(0)),
        Network,
        "unknown drive type"
    );
    assert_eq!(win(r"C:\a", |w| w.drive_type = None), Local);
    // Sync evidence beats a remote drive type (redirected profile folders).
    assert_eq!(
        win(r"C:\Users\Zoë\OneDrive\x", |w| w.drive_type =
            Some(DRIVE_REMOTE)),
        Sync(Provider::OneDrive)
    );
}

#[test]
fn windows_cloud_files_and_attributes() {
    use StorageKind::*;
    // A registered sync root (SharePoint/Teams libraries, second accounts, Box, Nextcloud VFS).
    assert_eq!(
        win(r"C:\Corp\Library", |w| w.cloud_root =
            Some("OneDrive".into())),
        Sync(Provider::OneDrive)
    );
    assert_eq!(
        win(r"C:\Corp\Library", |w| w.cloud_root = Some("Box".into())),
        Sync(Provider::Unknown)
    );
    assert_eq!(
        win(r"C:\Corp\Library", |w| w.cloud_root = Some(String::new())),
        Sync(Provider::Unknown)
    );
    // Any folder in the chain carrying a recall or pin attribute.
    for bit in [
        FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
        FILE_ATTRIBUTE_RECALL_ON_OPEN,
        FILE_ATTRIBUTE_PINNED,
        FILE_ATTRIBUTE_UNPINNED,
    ] {
        assert_eq!(
            win(r"E:\Cloud\Film", |w| w.attr_chain = vec![0x10, bit | 0x10]),
            Sync(Provider::Unknown),
            "{bit:#x}"
        );
    }
    assert_eq!(
        win(r"E:\Cloud\Film", |w| w.attr_chain =
            vec![0x10, 0x1000, 0x20]),
        Local
    );
    assert_eq!(win(r"E:\Cloud\Film", |w| w.attr_chain = vec![]), Local);
}

#[test]
fn windows_google_drive_volume() {
    use StorageKind::*;
    let gd = Sync(Provider::GoogleDrive);
    assert_eq!(
        win(r"G:\My Drive\Film", |w| w.volume_label =
            Some("Google Drive".into())),
        gd
    );
    assert_eq!(
        win(r"G:\Shared drives\x", |w| w.volume_label =
            Some("GOOGLE DRIVE".into())),
        gd
    );
    assert_eq!(win(r"G:\x", |w| w.volume_fs = Some("DriveFS".into())), gd);
    assert_eq!(
        win(r"G:\x", |w| w.volume_label = Some("Data".into())),
        Local
    );
    assert_eq!(
        win(r"G:\x", |w| {
            w.volume_label = Some("Data".into());
            w.volume_fs = Some("NTFS".into())
        }),
        Local
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
    for bad in ["", "cloud", "local:x", "Network1", "syncs"] {
        assert!(bad.parse::<StorageKind>().is_err(), "{bad:?}");
    }
    assert!(serde_json::from_str::<StorageKind>("\"nope\"").is_err());
    // A sync provider we do not know fails strict (sync), not open.
    for t in ["sync", "SYNC", "sync:box", "sync:", "sync:nextcloud"] {
        assert_eq!(
            t.parse::<StorageKind>().unwrap(),
            StorageKind::Sync(Provider::Unknown),
            "{t}"
        );
    }
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
    let v = |s: &str| Some(serde_json::Value::String(s.into()));
    assert_eq!(
        resolve(dir.path(), v("sync:icloud").as_ref()),
        (StorageKind::Sync(Provider::ICloud), None)
    );
    let (kind, warning) = resolve(dir.path(), v("floppy").as_ref());
    assert_eq!(kind, detect(dir.path(), None));
    assert!(warning.unwrap().contains("floppy"));
    assert_eq!(resolve(dir.path(), None).1, None);
}

#[test]
fn combine_warns_on_weakening_overrides() {
    use serde_json::json;
    let sync = StorageKind::Sync(Provider::Dropbox);
    let (k, w) = combine(sync, Some(&json!("local")));
    assert_eq!(k, StorageKind::Local, "the override is still honoured");
    assert!(w.unwrap().contains("sync:dropbox"));
    let (k, w) = combine(sync, Some(&json!("network")));
    assert_eq!(k, StorageKind::Network);
    assert!(w.is_some());
    // Not weakening: same or stricter, or nothing detected as sync.
    assert_eq!(
        combine(sync, Some(&json!("sync:onedrive"))),
        (StorageKind::Sync(Provider::OneDrive), None)
    );
    assert_eq!(
        combine(StorageKind::Network, Some(&json!("local"))),
        (StorageKind::Local, None)
    );
    assert_eq!(
        combine(StorageKind::Local, Some(&json!("sync"))),
        (StorageKind::Sync(Provider::Unknown), None)
    );
    assert_eq!(combine(sync, None), (sync, None));
    // Unparsable or non-string: ignored with a warning, detection stands.
    for bad in [
        json!("floppy"),
        json!(7),
        json!(null),
        json!(["local"]),
        json!({}),
    ] {
        let (k, w) = combine(sync, Some(&bad));
        assert_eq!(k, sync, "{bad}");
        assert!(w.unwrap().contains("ignored"), "{bad}");
    }
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

#[cfg(target_os = "linux")]
#[test]
fn linux_probe_dropbox_dir_is_config_not_a_sync_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".dropbox").join("instance1")).unwrap();
    let nested = dir.path().join("proj");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(detect(&nested, None), StorageKind::Local);
    std::fs::remove_dir_all(dir.path().join(".dropbox")).unwrap();
    std::fs::File::create(dir.path().join(".dropbox")).unwrap();
    assert_eq!(detect(&nested, None), StorageKind::Sync(Provider::Dropbox));
}

#[cfg(target_os = "linux")]
#[test]
fn linux_probe_sees_other_sync_markers() {
    for marker in [".sync_5f3a.db", ".megaignore", ".stignore"] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join(marker)).unwrap();
        assert!(detect(dir.path(), None).is_sync(), "{marker}");
    }
}
