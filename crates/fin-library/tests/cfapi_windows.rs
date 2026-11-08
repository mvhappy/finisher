//! A real Cloud Files sync root (plan 5D Task 3 Step 0 and Decisions 12–13, 19): our own
//! provider registered on a temporary NTFS directory, connected with callbacks that count
//! every `FETCH_DATA` and `FETCH_PLACEHOLDERS` (and refuse them, so nothing ever downloads
//! or hangs). Scans and opens must raise none.
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(windows)]

mod win_support;

use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use fin_library::source::NativeSource;
use fin_library::{EntryType, OpenError, ScanEntry, Source, open_for_read};
use win_support::{err, expect_of, read, rel, scan, skip, tempdir};
use windows_sys::Win32::Storage::CloudFilters::*;
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL, FILE_BASIC_INFO,
};
use windows_sys::core::GUID;

/// What the provider saw.
#[derive(Default)]
struct Counts {
    fetch_data: AtomicU32,
    fetch_placeholders: AtomicU32,
}

const STATUS_UNSUCCESSFUL: i32 = 0xC000_0001_u32 as i32;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn wpath(p: &Path) -> Vec<u16> {
    wide(p.to_str().unwrap())
}

/// Refuses a data request (the file stays a placeholder; the reader gets an error).
unsafe extern "system" fn on_fetch_data(
    info: *const CF_CALLBACK_INFO,
    params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: the platform passes valid pointers for the callback's duration; the context
    // is the leaked `Counts` of this connection.
    unsafe {
        let info = &*info;
        (*(info.CallbackContext as *const Counts))
            .fetch_data
            .fetch_add(1, Ordering::SeqCst);
        let fetch = (*params).Anonymous.FetchData;
        let op = CF_OPERATION_INFO {
            StructSize: size_of::<CF_OPERATION_INFO>() as u32,
            Type: CF_OPERATION_TYPE_TRANSFER_DATA,
            ConnectionKey: info.ConnectionKey,
            TransferKey: info.TransferKey,
            CorrelationVector: info.CorrelationVector,
            SyncStatus: ptr::null(),
            RequestKey: info.RequestKey,
        };
        let mut p: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        p.ParamSize = (std::mem::offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_0>()) as u32;
        p.Anonymous.TransferData = CF_OPERATION_PARAMETERS_0_0 {
            Flags: 0,
            CompletionStatus: STATUS_UNSUCCESSFUL,
            Buffer: ptr::null(),
            Offset: fetch.RequiredFileOffset,
            Length: fetch.RequiredLength,
        };
        CfExecute(&op, &mut p);
    }
}

/// Answers a population request with no entries and population turned off.
unsafe extern "system" fn on_fetch_placeholders(
    info: *const CF_CALLBACK_INFO,
    _params: *const CF_CALLBACK_PARAMETERS,
) {
    // SAFETY: as in `on_fetch_data`.
    unsafe {
        let info = &*info;
        (*(info.CallbackContext as *const Counts))
            .fetch_placeholders
            .fetch_add(1, Ordering::SeqCst);
        let op = CF_OPERATION_INFO {
            StructSize: size_of::<CF_OPERATION_INFO>() as u32,
            Type: CF_OPERATION_TYPE_TRANSFER_PLACEHOLDERS,
            ConnectionKey: info.ConnectionKey,
            TransferKey: info.TransferKey,
            CorrelationVector: info.CorrelationVector,
            SyncStatus: ptr::null(),
            RequestKey: info.RequestKey,
        };
        let mut p: CF_OPERATION_PARAMETERS = std::mem::zeroed();
        p.ParamSize = (std::mem::offset_of!(CF_OPERATION_PARAMETERS, Anonymous)
            + size_of::<CF_OPERATION_PARAMETERS_0_4>()) as u32;
        p.Anonymous.TransferPlaceholders = CF_OPERATION_PARAMETERS_0_4 {
            Flags: CF_OPERATION_TRANSFER_PLACEHOLDERS_FLAG_DISABLE_ON_DEMAND_POPULATION,
            CompletionStatus: 0,
            PlaceholderTotalCount: 0,
            PlaceholderArray: ptr::null_mut(),
            PlaceholderCount: 0,
            EntriesProcessed: 0,
        };
        CfExecute(&op, &mut p);
    }
}

/// A registered, connected sync root; disconnected and unregistered on drop.
struct SyncRoot {
    _dir: tempfile::TempDir,
    path: PathBuf,
    key: CF_CONNECTION_KEY,
    counts: &'static Counts,
}

const TIME: i64 = 133_000_000_000_000_000; // 2022-06-21 in 100 ns since 1601

impl SyncRoot {
    /// `None` (with a notice) where Cloud Files cannot register a root here.
    fn new(test: &str) -> Option<SyncRoot> {
        let dir = tempdir();
        let path = dir.path().join("root");
        std::fs::create_dir(&path).unwrap();
        let identity = b"finisher-test";
        let name = wide("FinisherTest");
        let version = wide("1.0");
        let reg = CF_SYNC_REGISTRATION {
            StructSize: size_of::<CF_SYNC_REGISTRATION>() as u32,
            ProviderName: name.as_ptr(),
            ProviderVersion: version.as_ptr(),
            SyncRootIdentity: identity.as_ptr().cast(),
            SyncRootIdentityLength: identity.len() as u32,
            FileIdentity: ptr::null(),
            FileIdentityLength: 0,
            ProviderId: GUID::from_u128(0x5d3a_77f1_9c2e_4b8a_a1d6_3e0f_c0de_5d03),
        };
        let policies = CF_SYNC_POLICIES {
            StructSize: size_of::<CF_SYNC_POLICIES>() as u32,
            Hydration: CF_HYDRATION_POLICY {
                Primary: CF_HYDRATION_POLICY_FULL,
                Modifier: CF_HYDRATION_POLICY_MODIFIER_NONE,
            },
            Population: CF_POPULATION_POLICY {
                Primary: CF_POPULATION_POLICY_FULL,
                Modifier: CF_POPULATION_POLICY_MODIFIER_NONE,
            },
            InSync: CF_INSYNC_POLICY_NONE,
            HardLink: CF_HARDLINK_POLICY_NONE,
            PlaceholderManagement: CF_PLACEHOLDER_MANAGEMENT_POLICY_DEFAULT,
        };
        let p = wpath(&path);
        // SAFETY: valid structures and a NUL-terminated path.
        let hr = unsafe { CfRegisterSyncRoot(p.as_ptr(), &reg, &policies, CF_REGISTER_FLAG_NONE) };
        if hr < 0 {
            skip(
                test,
                &format!("Cloud Files could not register a sync root ({hr:#x})"),
            );
            return None;
        }
        let counts: &'static Counts = Box::leak(Box::default());
        let table = [
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_DATA,
                Callback: Some(on_fetch_data),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_FETCH_PLACEHOLDERS,
                Callback: Some(on_fetch_placeholders),
            },
            CF_CALLBACK_REGISTRATION {
                Type: CF_CALLBACK_TYPE_NONE,
                Callback: None,
            },
        ];
        let mut key: CF_CONNECTION_KEY = 0;
        // SAFETY: the table ends with CF_CALLBACK_TYPE_NONE; the context outlives the
        // connection (leaked).
        let hr = unsafe {
            CfConnectSyncRoot(
                p.as_ptr(),
                table.as_ptr(),
                ptr::from_ref(counts).cast(),
                CF_CONNECT_FLAG_NONE,
                &mut key,
            )
        };
        if hr < 0 {
            // SAFETY: the root was registered above.
            unsafe { CfUnregisterSyncRoot(p.as_ptr()) };
            skip(test, &format!("Cloud Files could not connect ({hr:#x})"));
            return None;
        }
        Some(SyncRoot {
            _dir: dir,
            path,
            key,
            counts,
        })
    }

    fn fetches(&self) -> (u32, u32) {
        (
            self.counts.fetch_data.load(Ordering::SeqCst),
            self.counts.fetch_placeholders.load(Ordering::SeqCst),
        )
    }

    /// Cloud-only placeholders below `dir` (relative): files of `size` bytes, or (size
    /// `None`) a directory left for on-demand population.
    fn placeholders(&self, dir: &str, items: &[(&str, Option<i64>)]) {
        let base = wpath(&self.path.join(dir));
        let names: Vec<Vec<u16>> = items.iter().map(|(n, _)| wide(n)).collect();
        let mut infos: Vec<CF_PLACEHOLDER_CREATE_INFO> = items
            .iter()
            .zip(&names)
            .map(|((n, size), w)| CF_PLACEHOLDER_CREATE_INFO {
                RelativeFileName: w.as_ptr(),
                FsMetadata: CF_FS_METADATA {
                    BasicInfo: FILE_BASIC_INFO {
                        CreationTime: TIME,
                        LastAccessTime: TIME,
                        LastWriteTime: TIME,
                        ChangeTime: TIME,
                        FileAttributes: if size.is_some() {
                            FILE_ATTRIBUTE_NORMAL
                        } else {
                            FILE_ATTRIBUTE_DIRECTORY
                        },
                    },
                    FileSize: size.unwrap_or(0),
                },
                FileIdentity: n.as_ptr().cast(),
                FileIdentityLength: n.len() as u32,
                Flags: if size.is_some() {
                    CF_PLACEHOLDER_CREATE_FLAG_MARK_IN_SYNC
                } else {
                    CF_PLACEHOLDER_CREATE_FLAG_NONE
                },
                Result: 0,
                CreateUsn: 0,
            })
            .collect();
        let mut done = 0u32;
        // SAFETY: the infos and names live across the call.
        let hr = unsafe {
            CfCreatePlaceholders(
                base.as_ptr(),
                infos.as_mut_ptr(),
                infos.len() as u32,
                CF_CREATE_FLAG_NONE,
                &mut done,
            )
        };
        assert!(hr >= 0, "CfCreatePlaceholders: {hr:#x}");
        assert_eq!(done as usize, items.len());
    }

    /// A regular file with `data`, converted into an in-sync (hydrated) placeholder.
    fn hydrated(&self, name: &str, data: &[u8]) {
        let p = self.path.join(name);
        std::fs::write(&p, data).unwrap();
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&p)
            .unwrap();
        // SAFETY: a valid handle and identity.
        let hr = unsafe {
            CfConvertToPlaceholder(
                f.as_raw_handle(),
                name.as_ptr().cast(),
                name.len() as u32,
                CF_CONVERT_FLAG_MARK_IN_SYNC,
                ptr::null_mut(),
                ptr::null_mut(),
            )
        };
        assert!(hr >= 0, "CfConvertToPlaceholder: {hr:#x}");
    }
}

impl Drop for SyncRoot {
    fn drop(&mut self) {
        let p = wpath(&self.path);
        // SAFETY: the connection and the registration made in `new`.
        unsafe {
            CfDisconnectSyncRoot(self.key);
            CfUnregisterSyncRoot(p.as_ptr());
        }
    }
}

/// The tree every test uses: two cloud-only files and a not-yet-populated placeholder
/// directory at the top, a hydrated file, and a plain subdirectory holding one more
/// cloud-only file.
fn tree(test: &str) -> Option<SyncRoot> {
    let r = SyncRoot::new(test)?;
    r.placeholders(
        "",
        &[
            ("a.mov", Some(1 << 20)),
            ("b.mov", Some(4096)),
            ("remote", None),
        ],
    );
    r.hydrated("local.mov", b"hydrated bytes");
    std::fs::create_dir(r.path.join("sub")).unwrap();
    r.placeholders("sub", &[("c.mov", Some(10))]);
    Some(r)
}

fn entries(src: Arc<dyn Source>) -> (Vec<ScanEntry>, Vec<fin_library::DirDone>) {
    let batches = scan(src);
    (
        batches.iter().flat_map(|b| b.entries.clone()).collect(),
        batches.iter().flat_map(|b| b.dirs_done.clone()).collect(),
    )
}

fn by_name<'a>(es: &'a [ScanEntry], name: &str) -> &'a ScanEntry {
    es.iter()
        .find(|e| e.name == name.as_bytes())
        .unwrap_or_else(|| panic!("{name} not listed"))
}

/// Step 0's record: what each class lists for each kind of entry.
fn report(es: &[ScanEntry], class: &str) {
    for e in es {
        eprintln!(
            "STEP0 {class}: {} type={:?} tag={:?} placeholder={:?} size={} id={}",
            e.name_nfc,
            e.entry_type,
            e.reparse_tag.map(|t| format!("{t:#010x}")),
            e.placeholder,
            e.size,
            e.file_id.is_some()
        );
    }
}

#[test]
fn cfapi_placeholders_never_fetched() {
    let Some(r) = tree("cfapi_placeholders_never_fetched") else {
        return;
    };
    for refused in 0..3u8 {
        let src = Arc::new(NativeSource::new(&r.path).refuse_classes(refused));
        let (es, dones) = entries(Arc::clone(&src) as Arc<dyn Source>);
        let class = src.enumeration();
        report(&es, class);
        let a = by_name(&es, "a.mov");
        assert!(a.placeholder.is_some(), "{class}: a.mov is cloud-only");
        assert_eq!(a.size, 1 << 20, "{class}: the size comes from the entry");
        assert!(by_name(&es, "c.mov").placeholder.is_some());
        let local = by_name(&es, "local.mov");
        assert_eq!(
            local.placeholder, None,
            "{class}: a hydrated file is ordinary"
        );
        assert!(
            local
                .reparse_tag
                .is_some_and(fin_library::reparse::is_cloud),
            "{class}: the hydrated file keeps its cloud tag: {:?}",
            local.reparse_tag
        );
        let remote = dones.iter().find(|d| d.dir == rel("remote"));
        assert!(
            remote.is_none_or(|d| !d.complete),
            "{class}: the placeholder directory was listed: {remote:?}"
        );
        assert_eq!(
            r.fetches(),
            (0, 0),
            "{class}: the scan made the provider fetch"
        );
    }

    // A probe pass: every file through the opener without hydrate.
    for name in ["a.mov", "b.mov", "sub/c.mov"] {
        let rr = rel(name);
        let got = err(open_for_read(&r.path, &rr, &expect_of(&r.path, &rr), false));
        assert!(matches!(got, OpenError::CloudOnly), "{name}: {got:?}");
    }
    let rr = rel("local.mov");
    let f = open_for_read(&r.path, &rr, &expect_of(&r.path, &rr), false).unwrap();
    assert_eq!(read(f), b"hydrated bytes");
    assert_eq!(
        r.fetches(),
        (0, 0),
        "the probe pass made the provider fetch"
    );
}

#[test]
fn recall_on_open_never_opened() {
    let Some(r) = tree("recall_on_open_never_opened") else {
        return;
    };
    let src = NativeSource::new(&r.path);
    let remote = src
        .read_dir(&src.open_dir(&rel("")).unwrap())
        .unwrap()
        .into_iter()
        .find(|e| e.name == b"remote")
        .unwrap();
    eprintln!(
        "STEP0 placeholder directory: {:?} tag={:?}",
        remote.attrs, remote.reparse_tag
    );
    assert_eq!(remote.entry_type, EntryType::Dir);
    assert!(
        fin_library::placeholder::from_attrs(remote.attrs).is_some(),
        "the unpopulated directory carries a recall bit: {:?}",
        remote.attrs
    );
    // Targeted, it is refused before it is listed; a file below it is refused by the opener.
    let opened = src.open_dir(&rel("remote"));
    assert!(
        opened.is_err(),
        "a placeholder directory was opened for listing"
    );
    let e = fin_library::Expect {
        size: 1,
        mtime_ns: 0,
        file_id: None,
        dev: None,
    };
    let got = err(open_for_read(&r.path, &rel("remote/x.mov"), &e, false));
    assert!(matches!(got, OpenError::CloudOnly), "{got:?}");
    assert_eq!(r.fetches(), (0, 0));
}

#[test]
fn hydrated_cloud_file_is_readable() {
    let Some(r) = tree("hydrated_cloud_file_is_readable") else {
        return;
    };
    let rr = rel("local.mov");
    let exp = expect_of(&r.path, &rr);
    assert_eq!(exp.size, 14);
    let f = open_for_read(&r.path, &rr, &exp, false).unwrap();
    assert_eq!(read(f), b"hydrated bytes");
    assert_eq!(r.fetches(), (0, 0));
}

/// In the fallback classes the tag comes from `EaSize`: the same tag in all three.
#[test]
fn tag_read_from_easize_in_fallbacks() {
    let Some(r) = tree("tag_read_from_easize_in_fallbacks") else {
        return;
    };
    let tags = |refused: u8| {
        let src = NativeSource::new(&r.path).refuse_classes(refused);
        let dir = src.open_dir(&rel("")).unwrap();
        let mut v: Vec<(Vec<u8>, Option<u32>, u32)> = src
            .read_dir(&dir)
            .unwrap()
            .into_iter()
            .map(|e| match e.attrs {
                fin_library::Attrs::Windows { attributes } => (e.name, e.reparse_tag, attributes),
                _ => unreachable!(),
            })
            .collect();
        v.sort();
        v
    };
    let extd = tags(0);
    assert!(extd.iter().any(|(_, t, _)| t.is_some()), "{extd:?}");
    assert_eq!(tags(1), extd, "FileIdBothDirectoryInfo");
    assert_eq!(tags(2), extd, "FileFullDirectoryInfo");
}

/// A directory unpinned by the user ("free up space") is still populated locally: it is
/// descended, and its hydrated files stay readable (Decision 13: only a recall bit makes a
/// placeholder directory).
#[test]
fn unpinned_directory_is_descended() {
    let Some(r) = tree("unpinned_directory_is_descended") else {
        return;
    };
    let d = r.path.join("freed");
    std::fs::create_dir(&d).unwrap();
    std::fs::write(d.join("kept.mov"), b"local").unwrap();
    {
        use std::os::windows::fs::OpenOptionsExt;
        let h = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(0x0200_0000) // FILE_FLAG_BACKUP_SEMANTICS
            .open(&d)
            .unwrap();
        // SAFETY: a valid directory handle.
        unsafe {
            let hr = CfConvertToPlaceholder(
                h.as_raw_handle(),
                b"freed".as_ptr().cast(),
                5,
                CF_CONVERT_FLAG_MARK_IN_SYNC,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            assert!(hr >= 0, "convert: {hr:#x}");
            let hr = CfSetPinState(
                h.as_raw_handle(),
                CF_PIN_STATE_UNPINNED,
                CF_SET_PIN_FLAG_NONE,
                ptr::null_mut(),
            );
            assert!(hr >= 0, "unpin: {hr:#x}");
        }
    }
    let src = NativeSource::new(&r.path);
    let freed = src
        .read_dir(&src.open_dir(&rel("")).unwrap())
        .unwrap()
        .into_iter()
        .find(|e| e.name == b"freed")
        .unwrap();
    eprintln!(
        "STEP0 unpinned directory: {:?} tag={:?}",
        freed.attrs, freed.reparse_tag
    );
    let (es, dones) = entries(Arc::new(src));
    let done = dones
        .iter()
        .find(|d| d.dir == rel("freed"))
        .expect("listed");
    assert!(done.complete, "{done:?}");
    let kept = by_name(&es, "kept.mov");
    eprintln!(
        "STEP0 file in it: placeholder={:?} tag={:?}",
        kept.placeholder, kept.reparse_tag
    );
    let rr = rel("freed/kept.mov");
    if kept.placeholder.is_none() {
        let f = open_for_read(&r.path, &rr, &expect_of(&r.path, &rr), false).unwrap();
        assert_eq!(read(f), b"local");
    }
    assert_eq!(r.fetches(), (0, 0));
}
