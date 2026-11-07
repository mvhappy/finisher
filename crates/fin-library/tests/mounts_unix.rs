//! Volume identity, mount states and the dataless policy on Linux and macOS (plan 5D Task 2,
//! Decisions 6, 15, 17). Linux mounts run in a user namespace (`support::userns`); macOS uses
//! `hdiutil` disk images and, opt-in, iCloud Drive (`FIN_TEST_ICLOUD=1`).
#![cfg(unix)]

mod support;

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use fin_library::process::{self, DatalessPolicy};
use fin_library::{RootState, VolumeIdentity, classify_root, identity, root_state, volume};
#[cfg(target_os = "macos")]
use support::skip;

const T: Duration = Duration::from_secs(60);

/// The dataless policy is process-wide: tests that set or rely on it take this lock.
static POLICY: Mutex<()> = Mutex::new(());

fn online(s: RootState) -> VolumeIdentity {
    match s {
        RootState::Online(v) => v,
        other => panic!("expected online, got {other:?}"),
    }
}

/// Which identity fields stayed equal (Step 0's record), printed with `--nocapture`.
fn report(what: &str, a: &VolumeIdentity, b: &VolumeIdentity) {
    eprintln!(
        "{what}: key {} | kind {} | devId {} | rootFileId {} | sample {}",
        if a.key == b.key { "same" } else { "CHANGED" },
        if a.kind == b.kind { "same" } else { "CHANGED" },
        if a.dev_id == b.dev_id {
            "same"
        } else {
            "CHANGED"
        },
        if a.root_file_id == b.root_file_id {
            "same"
        } else {
            "CHANGED"
        },
        if a.sample == b.sample {
            "same"
        } else {
            "CHANGED"
        },
    );
}

#[test]
fn policy_off_in_process() {
    let _g = POLICY.lock().unwrap_or_else(|p| p.into_inner());
    let (off, on) = if cfg!(target_os = "macos") {
        (DatalessPolicy::Off, DatalessPolicy::On)
    } else {
        (DatalessPolicy::NotApplicable, DatalessPolicy::NotApplicable)
    };
    process::deny_dataless_materialisation().unwrap();
    assert_eq!(process::dataless_policy().unwrap(), off);
    process::allow_dataless_materialisation().unwrap();
    assert_eq!(process::dataless_policy().unwrap(), on);
    process::deny_dataless_materialisation().unwrap();
    assert_eq!(process::dataless_policy().unwrap(), off);
}

/// Case sensitivity the way a test may learn it: by writing, in a sibling directory.
fn case_sensitive_by_writing(dir: &Path) -> bool {
    let probe = tempfile::tempdir_in(dir).unwrap();
    fs::write(probe.path().join("a"), b"").unwrap();
    !probe.path().join("A").exists()
}

#[test]
fn case_sensitivity_detected_without_writing() {
    let parent = tempfile::tempdir().unwrap();
    let t = parent.path().join("lib");
    fs::create_dir(&t).unwrap();
    fs::write(t.join("x.mov"), b"x").unwrap();
    let before = fs::metadata(&t).unwrap();
    let id = identity(&t).unwrap();
    let after = fs::metadata(&t).unwrap();
    assert_eq!(
        (
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec()
        ),
        (
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec()
        ),
        "measuring wrote nothing"
    );
    assert_eq!(fs::read_dir(&t).unwrap().count(), 1);
    assert_eq!(
        id.case_sensitive,
        case_sensitive_by_writing(parent.path()),
        "{}",
        id.kind
    );
    #[cfg(target_os = "macos")]
    mac::case_sensitive_images();
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use fin_library::open::hooks::{self, Forced};
    use fin_library::source::NativeSource;
    use fin_library::{EntryType, OpenError, RawRel, Source, open_for_read};
    use support::{bind, expect_of, rel, tmpfs, umount, userns};

    fn tree(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let src = base.join("src");
        let m = base.join("m");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.mov"), b"a").unwrap();
        fs::write(src.join("b.mov"), b"bb").unwrap();
        fs::create_dir(&m).unwrap();
        (src, m)
    }

    #[test]
    fn bind_mount_unmounted_is_offline() {
        if !userns("linux::bind_mount_unmounted_is_offline") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let (src, m) = tree(t.path());
        bind(&src, &m);
        let stored = identity(&m).unwrap();
        assert!(
            stored.mount_root,
            "a bind mount on the root's own device is a mount root"
        );
        assert_eq!(
            stored.dev_id,
            identity(t.path()).unwrap().dev_id,
            "same device: st_dev alone could not see this mount"
        );
        online(root_state(&m, &stored, T));
        umount(&m);
        assert_eq!(root_state(&m, &stored, T), RootState::Unmounted);
        bind(&src, &m);
        online(root_state(&m, &stored, T));
        umount(&m);
    }

    #[test]
    fn nested_mount_reported_and_not_crossed() {
        if !userns("linux::nested_mount_reported_and_not_crossed") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("root");
        let other = t.path().join("other");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("x.mov"), b"x").unwrap();
        fs::write(other.join("y.mov"), b"y").unwrap();
        fs::write(root.join("file.mov"), b"").unwrap();
        bind(&other, &root.join("sub"));
        bind(&other.join("y.mov"), &root.join("file.mov"));
        let src = NativeSource::new(&root);
        let d = src.open_dir(&RawRel::root()).unwrap();
        let entries = src.read_dir(&d).unwrap();
        let sub = entries.iter().find(|e| e.name == b"sub").unwrap();
        assert!(sub.mount_root && sub.entry_type == EntryType::Dir);
        assert!(
            entries
                .iter()
                .find(|e| e.name == b"file.mov")
                .unwrap()
                .mount_root
        );
        let x = expect_of(&root.join("sub/x.mov"));
        let y = expect_of(&root.join("file.mov"));
        let check = || {
            let r = open_for_read(&root, &rel(&[b"sub", b"x.mov"]), &x, false);
            assert!(matches!(r, Err(OpenError::Escapes)), "{r:?}");
            let r = open_for_read(&root, &rel(&[b"file.mov"]), &y, false);
            assert!(matches!(r, Err(OpenError::Escapes)), "{r:?}");
        };
        check();
        for errno in [libc::ENOSYS, libc::EPERM] {
            let forced = Forced {
                openat2_errno: Some(errno),
                noatime_eperm: false,
            };
            let ((), counts) = hooks::with(forced, check);
            assert!(counts.fallbacks > 0);
        }
        umount(&root.join("file.mov"));
        umount(&root.join("sub"));
    }

    #[test]
    fn identity_stable_across_remount() {
        if !userns("linux::identity_stable_across_remount") {
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let (src, m) = tree(t.path());
        bind(&src, &m);
        let stored = identity(&m).unwrap();
        umount(&m);
        bind(&src, &m);
        let now = volume::root_stat(&m).unwrap();
        report("bind remount", &stored, &now.identity());
        assert_eq!(now.key, stored.key);
        assert_eq!(now.root_file_id, stored.root_file_id);
        assert_eq!(now.sample, stored.sample);
        assert_eq!(
            classify_root(&stored, &now),
            RootState::Online(now.identity())
        );
        umount(&m);

        // A new tmpfs is a new volume: its f_fsid changes and its content does not confirm.
        tmpfs(&m);
        fs::write(m.join("a.mov"), b"a").unwrap();
        let first = identity(&m).unwrap();
        umount(&m);
        std::thread::sleep(Duration::from_millis(20));
        tmpfs(&m);
        fs::write(m.join("a.mov"), b"a").unwrap();
        let now = volume::root_stat(&m).unwrap();
        report("tmpfs remount", &first, &now.identity());
        assert_ne!(now.key, first.key, "tmpfs f_fsid is per mount");
        assert!(matches!(
            classify_root(&first, &now),
            RootState::VolumeChanged { .. }
        ));
        umount(&m);
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::path::PathBuf;
    use support::run;

    /// A disk image attached at a private mount point; detached on drop.
    struct Image {
        dmg: PathBuf,
        mnt: PathBuf,
        attached: bool,
    }

    impl Image {
        fn create(dir: &Path, fs_name: &str) -> Option<Image> {
            let dmg = dir.join("t.dmg");
            run(
                "hdiutil",
                &[
                    "create",
                    "-quiet",
                    "-ov",
                    "-size",
                    "16m",
                    "-fs",
                    fs_name,
                    "-volname",
                    "FinTest",
                    dmg.to_str()?,
                ],
            )?;
            let mnt = dir.join("m");
            fs::create_dir_all(&mnt).ok()?;
            let mut img = Image {
                dmg,
                mnt,
                attached: false,
            };
            img.attach().then_some(img)
        }

        fn attach(&mut self) -> bool {
            self.attached = run(
                "hdiutil",
                &[
                    "attach",
                    "-quiet",
                    "-nobrowse",
                    "-mountpoint",
                    self.mnt.to_str().unwrap(),
                    self.dmg.to_str().unwrap(),
                ],
            )
            .is_some();
            self.attached
        }

        fn detach(&mut self) {
            let m = self.mnt.to_str().unwrap();
            if run("hdiutil", &["detach", "-quiet", m]).is_none() {
                run("hdiutil", &["detach", "-quiet", "-force", m]);
            }
            self.attached = false;
        }
    }

    impl Drop for Image {
        fn drop(&mut self) {
            if self.attached {
                self.detach();
            }
        }
    }

    fn image(test: &str, dir: &Path, fs_name: &str) -> Option<Image> {
        let img = Image::create(dir, fs_name);
        if img.is_none() {
            skip(
                test,
                "hdiutil could not create and attach a disk image here",
            );
        }
        img
    }

    #[test]
    fn disk_image_detached_is_offline() {
        let t = tempfile::tempdir().unwrap();
        let Some(mut img) = image("disk_image_detached_is_offline", t.path(), "APFS") else {
            return;
        };
        fs::write(img.mnt.join("a.mov"), b"a").unwrap();
        let stored = identity(&img.mnt).unwrap();
        assert!(stored.mount_root);
        assert!(!stored.remote);
        online(root_state(&img.mnt, &stored, T));
        img.detach();
        assert_eq!(root_state(&img.mnt, &stored, T), RootState::Unmounted);
        assert!(img.attach());
        online(root_state(&img.mnt, &stored, T));
    }

    #[test]
    fn identity_stable_across_remount() {
        let t = tempfile::tempdir().unwrap();
        let Some(mut img) = image("identity_stable_across_remount", t.path(), "APFS") else {
            return;
        };
        fs::write(img.mnt.join("a.mov"), b"a").unwrap();
        fs::create_dir(img.mnt.join("d")).unwrap();
        let stored = identity(&img.mnt).unwrap();
        let fields = volume::macos::fields_of(&img.mnt).unwrap();
        img.detach();
        assert!(img.attach());
        let now = volume::root_stat(&img.mnt).unwrap();
        let fields2 = volume::macos::fields_of(&img.mnt).unwrap();
        report("disk image detach/attach", &stored, &now.identity());
        eprintln!(
            "ATTR_VOL_UUID {} | f_mntfromname {} | f_fsid {}",
            if fields.uuid == fields2.uuid {
                "same"
            } else {
                "CHANGED"
            },
            if fields.mntfrom == fields2.mntfrom {
                "same"
            } else {
                "CHANGED"
            },
            if fields.fsid == fields2.fsid {
                "same"
            } else {
                "CHANGED"
            },
        );
        assert!(fields.uuid.is_some(), "an APFS volume has a UUID");
        assert_eq!(now.key, stored.key, "keyed by ATTR_VOL_UUID");
        assert_eq!(
            classify_root(&stored, &now),
            RootState::Online(now.identity())
        );
    }

    pub(super) fn case_sensitive_images() {
        for (fs_name, want) in [("Case-sensitive APFS", true), ("APFS", false)] {
            let t = tempfile::tempdir().unwrap();
            let Some(img) = image(
                "case_sensitivity_detected_without_writing",
                t.path(),
                fs_name,
            ) else {
                return;
            };
            assert_eq!(
                identity(&img.mnt).unwrap().case_sensitive,
                want,
                "{fs_name}"
            );
        }
    }
}

/// A real dataless file and folder (iCloud Drive, a File Provider domain): seen through
/// `SF_DATALESS` without being materialised, refused by the opener, and with the policy off a
/// raw read fails `EDEADLK` instead of downloading. Opt-in: it writes and evicts a test folder
/// in the signed-in user's iCloud Drive.
#[cfg(target_os = "macos")]
#[test]
fn dataless_dir_not_descended() {
    use std::io::Read;
    use std::os::macos::fs::MetadataExt as _;

    use fin_library::placeholder::SF_DATALESS;
    use fin_library::source::NativeSource;
    use fin_library::{Attrs, Excludes, Expect, OpenError, Placeholder, RawRel, Source};
    use fin_library::{listing, open_for_read};

    let test = "dataless_dir_not_descended";
    let _g = POLICY.lock().unwrap_or_else(|p| p.into_inner());
    if std::env::var("FIN_TEST_ICLOUD").as_deref() != Ok("1") {
        skip(
            test,
            "set FIN_TEST_ICLOUD=1 on a Mac signed in to iCloud Drive (writes and evicts a test folder there)",
        );
        return;
    }
    let home = std::env::var("HOME").unwrap();
    let drive = Path::new(&home).join("Library/Mobile Documents/com~apple~CloudDocs");
    if !drive.is_dir() {
        skip(test, "no iCloud Drive folder for this user");
        return;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = drive.join(format!("finisher-test-{nanos}"));
    fs::create_dir_all(dir.join("sub")).unwrap();
    let data: Vec<u8> = (0..256 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    fs::write(dir.join("f.bin"), &data).unwrap();
    fs::write(dir.join("sub/g.bin"), &data).unwrap();
    let flags = |p: &Path| fs::symlink_metadata(p).unwrap().st_flags();
    let dataless = |p: &Path| flags(p) & SF_DATALESS != 0;
    // Eviction fails until the item is uploaded: retry for up to 5 minutes.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    for p in [dir.join("f.bin"), dir.join("sub")] {
        while !dataless(&p) {
            assert!(
                std::time::Instant::now() < deadline,
                "iCloud did not evict {p:?} in time"
            );
            let _ = support::run("brctl", &["evict", p.to_str().unwrap()]);
            std::thread::sleep(Duration::from_secs(2));
        }
    }
    process::deny_dataless_materialisation().unwrap();

    let src = NativeSource::new(&dir);
    let d = src.open_dir(&RawRel::root()).unwrap();
    let raw = src.read_dir(&d).unwrap();
    for name in [&b"f.bin"[..], b"sub"] {
        let e = raw.iter().find(|e| e.name == name).unwrap();
        assert!(matches!(e.attrs, Attrs::Macos { flags } if flags & SF_DATALESS != 0));
    }
    let f = raw.iter().find(|e| e.name == b"f.bin").unwrap().clone();
    assert_eq!(
        f.size,
        data.len() as u64,
        "a dataless file keeps its logical size"
    );
    let entries = listing::entries(&RawRel::root(), raw, &Excludes::defaults(false));
    for e in &entries {
        assert_eq!(e.placeholder, Some(Placeholder::Dataless), "{}", e.name_nfc);
    }
    assert!(
        dataless(&dir.join("f.bin")) && dataless(&dir.join("sub")),
        "listing materialised"
    );

    let expect = Expect {
        size: f.size,
        mtime_ns: f.mtime_ns,
        file_id: f.file_id.clone(),
    };
    let r = open_for_read(&dir, &RawRel(vec![b"f.bin".to_vec()]), &expect, false);
    assert!(matches!(r, Err(OpenError::CloudOnly)), "{r:?}");
    let raw_read = fs::File::open(dir.join("f.bin")).and_then(|mut h| h.read(&mut [0u8; 16]));
    assert_eq!(
        raw_read.err().and_then(|e| e.raw_os_error()),
        Some(libc::EDEADLK),
        "with the policy off a read fails instead of downloading"
    );
    assert!(dataless(&dir.join("f.bin")) && dataless(&dir.join("sub")));
    fs::remove_dir_all(&dir).unwrap();
}
