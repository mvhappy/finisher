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
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use super::clock::unix_ms;
use crate::fsio;
use crate::names::MAX_EPOCH;

/// How many successive epochs one acquisition tries.
pub const TRIES: u64 = 4;

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

/// The claim files in `dir`, by epoch.
pub fn list(dir: &Path) -> io::Result<Vec<ClaimFile>> {
    let mut out = vec![];
    for e in fs::read_dir(dir)? {
        let e = e?;
        let Some(epoch) = e.file_name().to_str().and_then(parse_claim) else {
            continue;
        };
        let server_ms = e.metadata().and_then(|m| m.modified()).ok().map(unix_ms);
        out.push(ClaimFile { epoch, server_ms });
    }
    out.sort_by_key(|c| c.epoch);
    Ok(out)
}

/// What [`claim_next`] got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub epoch: u64,
    /// A lower name in the range was already taken: someone else claimed it, possibly a
    /// moment ago (a fresh claim can be invisible to the listing the base came from).
    pub contended: bool,
}

/// Claims the first free epoch of `base+1..=base+4` for `server_id`. `AlreadyExists` if all
/// four are taken; any other error as is.
pub fn claim_next(dir: &Path, base: u64, server_id: &str) -> io::Result<Claimed> {
    for epoch in base + 1..=base + TRIES {
        if epoch > MAX_EPOCH {
            return Err(io::Error::other(format!(
                "lease epoch {epoch} is above {MAX_EPOCH}"
            )));
        }
        match create(&dir.join(claim_name(epoch)), server_id) {
            Ok(()) => {
                return Ok(Claimed {
                    epoch,
                    contended: epoch > base + 1,
                });
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        ErrorKind::AlreadyExists,
        format!("epochs {} to {} are all claimed", base + 1, base + TRIES),
    ))
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
    let _ = fs::remove_file(dir.join(claim_name(epoch)));
}

/// Removes every claim below `epoch`, so a missing `lease.json` beside an old claim does not
/// read as "held" forever. Best effort: what fails is retried after the next acquisition.
pub fn remove_below(dir: &Path, epoch: u64) {
    for c in list(dir).unwrap_or_default() {
        if c.epoch < epoch {
            remove(dir, c.epoch);
        }
    }
}
