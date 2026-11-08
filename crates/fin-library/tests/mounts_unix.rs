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
    use fin_library::{EntryType, Expect, OpenError, RawRel, Source, open_for_read};
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
    fn nested_mount_reported_and_crossed() {
        if !userns("linux::nested_mount_reported_and_crossed") {
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
        // Nested mounts are catalogued, so the opener crosses them: the handle is checked
        // against the row's device (a bind mount shares it; another device is `Changed`). A
        // row without a device must stay on the root's mount.
        let x = expect_of(&root.join("sub/x.mov"));
        let y = expect_of(&root.join("file.mov"));
        let sub = entries.iter().find(|e| e.name == b"sub").unwrap();
        assert_eq!(x.dev, sub.dev, "the row's device is the listing's");
        // The walker catalogues the nested mount with its volume.
        let mut dones = Vec::new();
        fin_library::walk(
            std::sync::Arc::new(NativeSource::new(&root)),
            fin_library::WalkMode::Full {
                generation: 1,
                known: None,
            },
            &fin_library::WalkOpts::new(fin_library::Excludes::defaults(true)),
            &mut |b| {
                dones.extend(b.dirs_done);
                Ok(())
            },
            &fin_library::CancelToken::new(),
        )
        .unwrap();
        let done = dones.iter().find(|d| d.dir == rel(&[b"sub"])).unwrap();
        assert!(done.complete && done.entries == 2, "{done:?}");
        assert!(
            done.volume.as_ref().is_some_and(|v| v.mount_root),
            "{done:?}"
        );
        let check = || {
            let xr = rel(&[b"sub", b"x.mov"]);
            let yr = rel(&[b"file.mov"]);
            let mut h = open_for_read(&root, &xr, &x, false).unwrap();
            let mut got = Vec::new();
            std::io::Read::read_to_end(&mut h, &mut got).unwrap();
            assert_eq!(got, b"x");
            assert!(open_for_read(&root, &yr, &y, false).is_ok());
            let other = Expect {
                dev: Some("0:0".into()),
                ..x.clone()
            };
            let r = open_for_read(&root, &xr, &other, false);
            assert!(matches!(r, Err(OpenError::Changed)), "{r:?}");
            for (r_, e) in [(&xr, &x), (&yr, &y)] {
                let no_dev = Expect {
                    dev: None,
                    ..e.clone()
                };
                let r = open_for_read(&root, r_, &no_dev, false);
                assert!(matches!(r, Err(OpenError::Escapes)), "{r:?}");
            }
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
    use support::image::image;

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

/// Real dataless files, and a dataless folder when the OS makes one (iCloud Drive, a File
/// Provider domain): seen through `SF_DATALESS` without being materialised, refused by the
/// opener, never listed by the walker, and with the policy off a raw read (or a listing of
/// the folder) fails `EDEADLK` instead of downloading. Opt-in: it writes and evicts a test
/// folder in the signed-in user's iCloud Drive, removed when the test ends.
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
    /// Removes the test folder however the test ends: materialisation back on first (a
    /// dataless folder cannot be listed, so not removed, with the policy off).
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = process::allow_dataless_materialisation();
            if let Err(e) = fs::remove_dir_all(&self.0) {
                eprintln!("could not remove {:?}: {e}", self.0);
            }
        }
    }
    let dir = drive.join(format!("finisher-test-{nanos}"));
    fs::create_dir_all(dir.join("sub")).unwrap();
    let _cleanup = Cleanup(dir.clone());
    let data: Vec<u8> = (0..256 * 1024u32)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
        .collect();
    fs::write(dir.join("f.bin"), &data).unwrap();
    fs::write(dir.join("sub/g.bin"), &data).unwrap();
    let flags = |p: &Path| fs::symlink_metadata(p).unwrap().st_flags();
    let dataless = |p: &Path| flags(p) & SF_DATALESS != 0;
    let evict = |p: &Path, wait: Duration| {
        let deadline = std::time::Instant::now() + wait;
        while !dataless(p) {
            if std::time::Instant::now() >= deadline {
                return false;
            }
            let _ = support::run("brctl", &["evict", p.to_str().unwrap()]);
            std::thread::sleep(Duration::from_secs(2));
        }
        true
    };
    // Eviction fails until the item is uploaded: up to 5 minutes for the files. `brctl evict`
    // of a folder evicts its files but leaves the folder itself materialised (macOS 27); only
    // the OS's own storage optimisation makes a folder dataless, so that half is checked when
    // it happens.
    for p in [dir.join("f.bin"), dir.join("sub/g.bin")] {
        assert!(
            evict(&p, Duration::from_secs(300)),
            "iCloud did not evict {p:?} in time"
        );
    }
    let sub_dataless = evict(&dir.join("sub"), Duration::from_secs(20));
    if !sub_dataless {
        skip(
            test,
            "the folder stayed materialised (brctl evicts only its files): the dataless-folder half is not run",
        );
    }
    let evicted = |dir: &Path| {
        dataless(&dir.join("f.bin"))
            && (sub_dataless && dataless(&dir.join("sub")) || dataless(&dir.join("sub/g.bin")))
    };
    process::deny_dataless_materialisation().unwrap();

    let src = NativeSource::new(&dir);
    let d = src.open_dir(&RawRel::root()).unwrap();
    let raw = src.read_dir(&d).unwrap();
    let f = raw.iter().find(|e| e.name == b"f.bin").unwrap().clone();
    assert!(matches!(f.attrs, Attrs::Macos { flags } if flags & SF_DATALESS != 0));
    assert_eq!(
        f.size,
        data.len() as u64,
        "a dataless file keeps its logical size"
    );
    let entries = listing::entries(&RawRel::root(), raw, &Excludes::defaults(false));
    let placeholder = |n: &str| {
        entries
            .iter()
            .find(|e| e.name_nfc == n)
            .unwrap()
            .placeholder
    };
    assert_eq!(placeholder("f.bin"), Some(Placeholder::Dataless));
    assert_eq!(
        placeholder("sub"),
        sub_dataless.then_some(Placeholder::Dataless)
    );
    assert!(evicted(&dir), "listing materialised");

    let expect = Expect {
        size: f.size,
        mtime_ns: f.mtime_ns,
        file_id: f.file_id.clone(),
        dev: f.dev.clone(),
    };
    let r = open_for_read(&dir, &RawRel(vec![b"f.bin".to_vec()]), &expect, false);
    assert!(matches!(r, Err(OpenError::CloudOnly)), "{r:?}");
    let raw_read = fs::File::open(dir.join("f.bin")).and_then(|mut h| h.read(&mut [0u8; 16]));
    assert_eq!(
        raw_read.err().and_then(|e| e.raw_os_error()),
        Some(libc::EDEADLK),
        "with the policy off a read fails instead of downloading"
    );
    if sub_dataless {
        // Listing a dataless folder is refused, and the code says why (M4).
        let sub = src.open_dir(&RawRel(vec![b"sub".to_vec()])).unwrap();
        let e = src
            .read_dir(&sub)
            .expect_err("a dataless folder is not listed");
        assert_eq!(e.raw_os_error(), Some(libc::EDEADLK), "{e}");
        assert_eq!(volume::unreachable_code(&e), "CLOUD_ONLY");
    }

    // The walker reports every dataless item as a placeholder, never lists a dataless folder
    // (closed incomplete, `CLOUD_ONLY`) and materialises nothing.
    let mut dones = Vec::new();
    let mut found = Vec::new();
    let s = fin_library::walk(
        std::sync::Arc::new(NativeSource::new(&dir)),
        fin_library::WalkMode::Full {
            generation: 1,
            known: None,
        },
        &fin_library::WalkOpts::new(Excludes::defaults(false)),
        &mut |b| {
            dones.extend(b.dirs_done);
            found.extend(b.entries);
            Ok(())
        },
        &fin_library::CancelToken::new(),
    )
    .unwrap();
    let sub_done = dones
        .iter()
        .find(|d| d.dir.components() == [b"sub".to_vec()])
        .unwrap();
    if sub_dataless {
        assert_eq!(s.listed, 1, "only the root is listed");
        assert!(!sub_done.complete && sub_done.code.as_deref() == Some("CLOUD_ONLY"));
    } else {
        assert_eq!(s.listed, 2);
        assert!(sub_done.complete, "{sub_done:?}");
        let g = found.iter().find(|e| e.name == b"g.bin").unwrap();
        assert_eq!(g.placeholder, Some(Placeholder::Dataless));
        assert_eq!(g.size, data.len() as u64);
    }
    assert!(evicted(&dir), "the walk materialised");
}
