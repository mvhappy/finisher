//! Helpers of the Windows source, opener and Cloud Files tests (plan 5D Task 3).
// Tests build and inspect trees by path; never a library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![allow(dead_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use fin_library::source::NativeSource;
use fin_library::{
    CancelToken, Excludes, Expect, OpenError, RawEntry, RawRel, ScanBatch, Source, WalkMode,
    WalkOpts, walk,
};

/// A raw chain from a `/`-joined UTF-8 path (`""` is the root).
pub fn rel(p: &str) -> RawRel {
    if p.is_empty() {
        return RawRel::root();
    }
    RawRel::new(p.split('/').map(|c| c.as_bytes().to_vec()).collect()).unwrap()
}

/// The listed entry at `rel` (the catalogue's view of it).
pub fn listed(root: &Path, rel: &RawRel) -> RawEntry {
    let src = NativeSource::new(root);
    let dir = src.open_dir(&rel.parent().unwrap()).unwrap();
    let name = rel.name().unwrap();
    src.read_dir(&dir)
        .unwrap()
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{} is not listed", rel.display()))
}

/// What the catalogue would expect for the file at `rel`: its listed size, mtime, id, device.
pub fn expect_of(root: &Path, rel: &RawRel) -> Expect {
    let e = listed(root, rel);
    Expect {
        size: e.size,
        mtime_ns: e.mtime_ns,
        file_id: e.file_id,
        dev: e.dev,
    }
}

pub fn read(f: std::fs::File) -> Vec<u8> {
    let mut v = Vec::new();
    let mut f = f;
    f.read_to_end(&mut v).unwrap();
    v
}

pub fn err(r: Result<std::fs::File, OpenError>) -> OpenError {
    r.expect_err("the open should have been refused")
}

/// A path in its verbatim form, so names Win32 would rewrite (`CON`, `a.`, `b `) and long
/// paths reach the file system as written.
pub fn verbatim(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if s.starts_with(r"\\?\") {
        p.to_path_buf()
    } else {
        PathBuf::from(format!(r"\\?\{s}"))
    }
}

/// Prints why a test did not run its OS-facility part (the facility is named).
pub fn skip(test: &str, why: &str) {
    eprintln!("SKIP {test}: {why}");
}

/// Runs a command; `Some(stdout)` when it succeeded.
pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `mklink /J link target` (junctions need no privilege).
pub fn junction(link: &Path, target: &Path) {
    let l = link.to_str().unwrap();
    let t = target.to_str().unwrap();
    run("cmd", &["/C", "mklink", "/J", l, t]).expect("mklink /J");
}

/// Walks `src` fully and collects every batch.
pub fn scan(src: Arc<dyn Source>) -> Vec<ScanBatch> {
    let mut batches = Vec::new();
    let opts = WalkOpts::new(Excludes::defaults(false));
    walk(
        src,
        WalkMode::Full {
            generation: 1,
            known: None,
        },
        &opts,
        &mut |b| {
            batches.push(b);
            Ok(())
        },
        &CancelToken::new(),
    )
    .expect("walk");
    batches
}

/// A temporary directory on the same NTFS volume as the build (the system temp may be a
/// different volume; the Cloud Files and WOF tests need NTFS).
pub fn tempdir() -> tempfile::TempDir {
    let base = Path::new(env!("CARGO_TARGET_TMPDIR"));
    std::fs::create_dir_all(base).unwrap();
    tempfile::tempdir_in(base).unwrap()
}
