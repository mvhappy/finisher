//! `lease.json` (spec §3.5 "Record"; parent spec §27.9 v3 "Record"): a fixed 512-byte record,
//! created once (published whole, never replacing) and then always rewritten **in place**,
//! never renamed over (after a Mac
//! rename-over, Windows reported the file missing for up to 5 s).
//!
//! Layout: compact JSON body, `\n`, the CRC-32C of the body as 8 lowercase hex digits, `\n`,
//! then ASCII spaces up to 512 bytes. A fixed size means an in-place rewrite always covers the
//! whole previous record; the CRC catches a torn or partial read.

use std::io::{self, ErrorKind};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::fsio;

pub const RECORD_SIZE: usize = 512;
pub const MAGIC: &str = "finisher-lease";
pub const VERSION: u32 = 1;
/// The record's file name in `.finisher/`.
pub const FILE: &str = "lease.json";

/// The fields of `lease.json`, serialised in this order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaseRecord {
    pub magic: String,
    pub version: u32,
    pub server_id: String,
    pub epoch: u64,
    /// Incremented by every heartbeat of this epoch (0 when acquired).
    pub heartbeat: u64,
    /// Who holds it, for people: machine name and app version.
    pub holder: String,
    /// The holder's server time ([`super::clock::ServerClock::now_server_ms`]) when it wrote
    /// this record. Observers judge staleness against their own server time, never their
    /// wall clock.
    pub probe_server_ms: i64,
}

impl LeaseRecord {
    pub fn new(
        server_id: &str,
        epoch: u64,
        heartbeat: u64,
        holder: &str,
        probe_server_ms: i64,
    ) -> Self {
        LeaseRecord {
            magic: MAGIC.to_string(),
            version: VERSION,
            server_id: server_id.to_string(),
            epoch,
            heartbeat,
            holder: holder.to_string(),
            probe_server_ms,
        }
    }
}

/// The 512 bytes of `r`. A holder description too long to fit is cut at a character
/// boundary (server ids are checked by [`super::valid_server_id`], so the rest always fits).
pub fn encode(r: &LeaseRecord) -> [u8; RECORD_SIZE] {
    let mut r = r.clone();
    let body = loop {
        let body = serde_json::to_vec(&r).unwrap_or_default();
        if body.len() + 10 <= RECORD_SIZE || r.holder.is_empty() {
            break body;
        }
        let keep = r.holder.chars().count().saturating_sub(1);
        r.holder = r.holder.chars().take(keep).collect();
    };
    let mut out = [b' '; RECORD_SIZE];
    let n = body.len().min(RECORD_SIZE - 10);
    out[..n].copy_from_slice(&body[..n]);
    out[n] = b'\n';
    out[n + 1..n + 9].copy_from_slice(format!("{:08x}", crc32c::crc32c(&body[..n])).as_bytes());
    out[n + 9] = b'\n';
    out
}

/// The record in `bytes`, or `None` unless they are exactly one well-formed record: 512 bytes,
/// a matching CRC, space padding, our magic and version 1. A newer version is `None` too:
/// the caller treats it as unreadable (held when acquiring, lost when fencing).
pub fn decode(bytes: &[u8]) -> Option<LeaseRecord> {
    if bytes.len() != RECORD_SIZE {
        return None;
    }
    let n = bytes.iter().position(|b| *b == b'\n')?;
    let (body, rest) = (&bytes[..n], &bytes[n + 1..]);
    let crc = rest.get(..8)?;
    if rest.get(8) != Some(&b'\n') || !rest[9..].iter().all(|b| *b == b' ') {
        return None;
    }
    if !crc
        .iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
        || std::str::from_utf8(crc).ok()? != format!("{:08x}", crc32c::crc32c(body))
    {
        return None;
    }
    let r: LeaseRecord = serde_json::from_slice(body).ok()?;
    (r.magic == MAGIC && r.version == VERSION).then_some(r)
}

/// What a read of `lease.json` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Seen {
    Missing,
    /// Present, but not one valid record (bad CRC, torn, newer version): the raw bytes.
    Corrupt(Vec<u8>),
    Valid(LeaseRecord),
}

/// Reads `path` uncached ([`fsio::read_uncached`], with delete sharing on Windows), retrying
/// transient errors ([`fsio::is_transient`]) for up to [`fsio::RETRIES`] × [`fsio::RETRY_DELAY`].
/// `Err` is "unreadable": held when acquiring; when fencing, that look cannot tell (`fence`).
pub(crate) fn read(path: &Path) -> io::Result<Seen> {
    let mut attempt = 0;
    loop {
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(seen) = injected(path) {
            return seen;
        }
        match fsio::read_uncached(path) {
            Ok(bytes) => {
                return Ok(match decode(&bytes) {
                    Some(r) => Seen::Valid(r),
                    None => Seen::Corrupt(bytes),
                });
            }
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(Seen::Missing),
            Err(e) if attempt < fsio::RETRIES && fsio::is_transient(&e) => {
                attempt += 1;
                std::thread::sleep(fsio::RETRY_DELAY);
            }
            Err(e) => return Err(e),
        }
    }
}

/// A holder's look at its own record re-reads one that does not decode this many more
/// times…
const SETTLE_READS: u32 = 10;
/// …this far apart (the S10 spike's values): a torn or short SMB read, or another server's
/// in-place write caught halfway, settles into one valid record.
const SETTLE_DELAY: std::time::Duration = std::time::Duration::from_millis(20);

/// [`read`], re-reading a [`Seen::Corrupt`] up to [`SETTLE_READS`] times. Still corrupt
/// after that is returned as such; the holder then cannot tell, and retries (the self-fence
/// deadline bounds it).
pub(crate) fn read_settled(path: &Path) -> io::Result<Seen> {
    let mut seen = read(path);
    for _ in 0..SETTLE_READS {
        if !matches!(seen, Ok(Seen::Corrupt(_))) {
            break;
        }
        std::thread::sleep(SETTLE_DELAY);
        seen = read(path);
    }
    seen
}

/// A read scripted by [`super::hooks::fault_record_reads_for_test`], if any.
#[cfg(any(test, feature = "test-hooks"))]
fn injected(path: &Path) -> Option<io::Result<Seen>> {
    use super::hooks::{ReadFault, faults};
    let dir = path.parent()?;
    match faults(dir, |f| f.reads.pop_front())? {
        ReadFault::Real => None,
        ReadFault::Error(kind) => Some(Err(io::Error::new(kind, "injected read failure"))),
        ReadFault::Torn => {
            let mut bytes = fsio::read_uncached(path).unwrap_or_default();
            bytes.truncate(bytes.len() / 2);
            Some(Ok(Seen::Corrupt(bytes)))
        }
        ReadFault::Stale => {
            let mut r = decode(&fsio::read_uncached(path).ok()?)?;
            r.heartbeat = r.heartbeat.checked_sub(1)?;
            Some(Ok(Seen::Valid(r)))
        }
    }
}
