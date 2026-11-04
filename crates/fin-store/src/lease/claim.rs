//! Claim files (spec §3.5 "Claims and epochs"): `lease.claim-<epoch:08>`, created with
//! `create_new` and holding the claimant's server id. The exclusivity of the name is what
//! makes epochs unique: `create_new` is decided by the file server, and had exactly one
//! winner in 100/100 races even from the Windows SMB client (parent spec §27.9).
//!
//! The epoch is always written with 8 digits, as in segment names, and only that spelling
//! parses: `lease.claim-12` and `lease.claim-00000012` would otherwise be two names for one
//! epoch, and both could be created.
//!
//! Claim files are a secondary signal: a new one can stay invisible to Windows `stat`/readdir
//! for up to 9.9 s. The record's epoch is authoritative.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

use super::clock::unix_ms;
use crate::fsio;
use crate::names::MAX_EPOCH;

pub fn claim_name(epoch: u64) -> String {
    format!("lease.claim-{epoch:08}")
}

/// The epoch of a canonical claim name.
pub fn parse_claim(name: &str) -> Option<u64> {
    let digits = name.strip_prefix("lease.claim-")?;
    (digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
}

/// A claim file found in the folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimFile {
    pub epoch: u64,
    /// Its mtime in server time (claims are never modified, so even a cached stat is right),
    /// `None` if it could not be read.
    pub server_ms: Option<i64>,
}

/// The claim files in `dir`, by epoch. Transient errors ([`fsio::is_transient`]) are
/// retried as [`super::record`] reads are, for up to [`fsio::RETRIES`] × [`fsio::RETRY_DELAY`].
pub fn list(dir: &Path) -> io::Result<Vec<ClaimFile>> {
    let mut attempt = 0;
    loop {
        match list_once(dir) {
            Err(e) if attempt < fsio::RETRIES && fsio::is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(fsio::RETRY_DELAY);
            }
            result => return result,
        }
    }
}

fn list_once(dir: &Path) -> io::Result<Vec<ClaimFile>> {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(kind) = hook(dir, |h| h.fail_list.take()) {
        return Err(io::Error::new(kind, "injected listing failure"));
    }
    let mut out = vec![];
    for e in fs::read_dir(dir)? {
        let e = e?;
        let Some(epoch) = e.file_name().to_str().and_then(parse_claim) else {
            continue;
        };
        let mtime = e.metadata().and_then(|m| m.modified()).ok();
        #[cfg(any(test, feature = "test-hooks"))]
        if hidden(dir, mtime) {
            continue;
        }
        out.push(ClaimFile {
            epoch,
            server_ms: mtime.map(unix_ms),
        });
    }
    out.sort_by_key(|c| c.epoch);
    Ok(out)
}

/// Per-folder test hooks.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Default)]
struct Hooks {
    lag: Option<std::time::Duration>,
    fail_list: Option<io::ErrorKind>,
    keep_backed_off: bool,
}

#[cfg(any(test, feature = "test-hooks"))]
fn hook<T>(dir: &Path, f: impl FnOnce(&mut Hooks) -> T) -> T {
    static HOOKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<std::path::PathBuf, Hooks>>,
    > = std::sync::OnceLock::new();
    let mut all = super::clock::lock(HOOKS.get_or_init(Default::default));
    f(all.entry(dir.to_path_buf()).or_default())
}

/// Test hook: listings of `dir` leave out claims younger than `lag` (real time), as the
/// Windows SMB client does for up to 9.9 s. Creating a claim still finds the name taken.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn hide_young_claims(dir: &Path, lag: std::time::Duration) {
    hook(dir, |h| h.lag = Some(lag));
}

/// Test hook: the next listing of `dir` fails with `kind` (once).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn fail_next_list_for_test(dir: &Path, kind: io::ErrorKind) {
    hook(dir, |h| h.fail_list = Some(kind));
}

/// Test hook: an acquisition that backs off in `dir` never gets to remove its claim (it
/// crashed, or its delete is slow).
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn keep_backed_off_claims_for_test(dir: &Path) {
    hook(dir, |h| h.keep_backed_off = true);
}

#[cfg(any(test, feature = "test-hooks"))]
fn hidden(dir: &Path, mtime: Option<std::time::SystemTime>) -> bool {
    let Some(lag) = hook(dir, |h| h.lag) else {
        return false;
    };
    mtime.is_none_or(|t| t.elapsed().map_or(true, |age| age < lag))
}

/// What [`claim_next`] got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub epoch: u64,
}

/// Claims epoch `base + 1` for `server_id`, and only that one. `AlreadyExists` if it is
/// taken: someone else is acquiring (or acquired) from the same view, possibly with a claim
/// our listing cannot see yet. The caller backs off **without creating anything**: a claim
/// at `base + 2` could never lead to an acquisition (the winner's record changes what it
/// judged), but it would fence the winner, since a claim above a holder's epoch loses it the
/// lease.
pub fn claim_next(dir: &Path, base: u64, server_id: &str) -> io::Result<Claimed> {
    let epoch = base + 1;
    if epoch > MAX_EPOCH {
        return Err(io::Error::other(format!(
            "lease epoch {epoch} is above {MAX_EPOCH}"
        )));
    }
    create(&dir.join(claim_name(epoch)), server_id)?;
    Ok(Claimed { epoch })
}

fn create(path: &Path, server_id: &str) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let result = file
        .write_all(server_id.as_bytes())
        .and_then(|()| fsio::sync_file(&file));
    if result.is_err() {
        // The name is ours either way; a claim without content is still a claim.
        let _ = fs::remove_file(path);
    }
    result
}

/// Who wrote claim `epoch`, if readable.
pub fn owner(dir: &Path, epoch: u64) -> Option<String> {
    let bytes = fsio::read_uncached(&dir.join(claim_name(epoch))).ok()?;
    let s = String::from_utf8(bytes).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Removes our own claim `epoch` (an acquisition that backed off). Best effort.
pub fn remove(dir: &Path, epoch: u64) {
    #[cfg(any(test, feature = "test-hooks"))]
    if hook(dir, |h| h.keep_backed_off) {
        return;
    }
    let _ = fs::remove_file(dir.join(claim_name(epoch)));
}

/// Removes every claim below `epoch`, so a missing `lease.json` beside an old claim does not
/// read as "held" forever. Best effort: what fails is retried after the next acquisition.
pub fn remove_below(dir: &Path, epoch: u64) {
    for c in list(dir).unwrap_or_default() {
        if c.epoch < epoch {
            let _ = fs::remove_file(dir.join(claim_name(c.epoch)));
        }
    }
}

/// The highest epoch in a segment name under `<dir>/segments/*/` (0 if none or unreadable).
/// With every lease file gone (deleted, or a damaged record whose claims were removed), the
/// segments still carry the epochs used, so a new one never repeats them.
pub(super) fn highest_segment_epoch(dir: &Path) -> u64 {
    let Ok(branches) = std::fs::read_dir(dir.join("segments")) else {
        return 0;
    };
    branches
        .flatten()
        .filter_map(|b| std::fs::read_dir(b.path()).ok())
        .flat_map(|files| files.flatten())
        .filter_map(|f| {
            crate::names::SegmentName::parse(f.file_name().to_str()?).map(|n| n.epoch())
        })
        .max()
        .unwrap_or(0)
}
