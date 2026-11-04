//! Strict file names under `.finisher/` (sub-project 4 spec §3.4). Anything that does not
//! match exactly is a foreign file, so a sync provider's renamed copy (`… 2.fseg`,
//! `… (1).fseg`, `… (conflicted copy).fseg`) never passes for one of ours.

use std::fmt;

/// `<epoch:08>-<firstSeq:012>-<lastSeq:012>-<hash12>.fseg`.
///
/// The widths are exact, so values must fit them (epochs below 10^8, seqs below 10^12): a
/// wider number would display as a name that does not parse.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SegmentName {
    pub epoch: u64,
    pub first_seq: u64,
    pub last_seq: u64,
    pub hash12: String,
}

impl SegmentName {
    pub fn parse(name: &str) -> Option<Self> {
        let rest = name.strip_suffix(".fseg")?;
        let mut parts = rest.split('-');
        let epoch = digits(parts.next()?, 8)?;
        let first_seq = digits(parts.next()?, 12)?;
        let last_seq = digits(parts.next()?, 12)?;
        let hash12 = hex12(parts.next()?)?;
        (parts.next().is_none() && first_seq <= last_seq).then_some(SegmentName {
            epoch,
            first_seq,
            last_seq,
            hash12,
        })
    }
}

impl fmt::Display for SegmentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:08}-{:012}-{:012}-{}.fseg",
            self.epoch, self.first_seq, self.last_seq, self.hash12
        )
    }
}

/// `<seq:012>-<hash12>.fsnap`. The same width rule as [`SegmentName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapName {
    pub seq: u64,
    pub hash12: String,
}

impl SnapName {
    pub fn parse(name: &str) -> Option<Self> {
        let (seq, hash) = name.strip_suffix(".fsnap")?.split_once('-')?;
        Some(SnapName {
            seq: digits(seq, 12)?,
            hash12: hex12(hash)?,
        })
    }
}

impl fmt::Display for SnapName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:012}-{}.fsnap", self.seq, self.hash12)
    }
}

/// The first 12 hex characters of a `<algorithm>:<hex>` file hash.
pub fn hash12(hash: &str) -> Option<&str> {
    let (_, hex) = hash.split_once(':')?;
    let short = hex.get(..12)?;
    short
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        .then_some(short)
}

const TEMP_PREFIX: &str = ".~fin-";
const TEMP_SUFFIX: &str = ".tmp";

/// A fresh `.~fin-<ulid>.tmp` name.
pub fn temp_name() -> String {
    format!("{TEMP_PREFIX}{}{TEMP_SUFFIX}", ulid::Ulid::new())
}

/// Whether `name` is a [`temp_name`]: the prefix, a 26-character Crockford base32 ULID, `.tmp`.
pub fn is_temp(name: &str) -> bool {
    name.strip_prefix(TEMP_PREFIX)
        .and_then(|s| s.strip_suffix(TEMP_SUFFIX))
        .is_some_and(|id| id.len() == 26 && ulid::Ulid::from_string(id).is_ok())
}

/// Exactly `width` ASCII digits.
fn digits(s: &str, width: usize) -> Option<u64> {
    (s.len() == width && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// Exactly 12 lowercase hex digits.
fn hex12(s: &str) -> Option<String> {
    (s.len() == 12
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    .then(|| s.to_string())
}
