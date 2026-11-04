//! Strict file names under `.finisher/` (sub-project 4 spec §3.4). Anything that does not
//! match exactly is a foreign file, so a sync provider's renamed copy (`… 2.fseg`,
//! `… (1).fseg`, `… (conflicted copy).fseg`) never passes for one of ours.

use std::fmt;

/// The largest epoch an 8-digit name field holds.
pub const MAX_EPOCH: u64 = 99_999_999;
/// The largest seq a 12-digit name field holds.
pub const MAX_SEQ: u64 = 999_999_999_999;

/// `<epoch:08>-<firstSeq:012>-<lastSeq:012>-<hash12>.fseg`.
///
/// Built only by [`SegmentName::new`] or [`SegmentName::parse`], which share one set of
/// checks, so every value displays as a name that parses back.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SegmentName {
    epoch: u64,
    first_seq: u64,
    last_seq: u64,
    hash12: String,
}

impl SegmentName {
    /// `None` unless `epoch ≤ MAX_EPOCH`, `first_seq ≤ last_seq ≤ MAX_SEQ` and `hash12` is 12
    /// lowercase hex digits.
    pub fn new(epoch: u64, first_seq: u64, last_seq: u64, hash12: &str) -> Option<Self> {
        (epoch <= MAX_EPOCH && first_seq <= last_seq && last_seq <= MAX_SEQ && is_hex12(hash12))
            .then(|| SegmentName {
                epoch,
                first_seq,
                last_seq,
                hash12: hash12.to_string(),
            })
    }

    pub fn parse(name: &str) -> Option<Self> {
        let rest = name.strip_suffix(".fseg")?;
        let mut parts = rest.split('-');
        let epoch = digits(parts.next()?, 8)?;
        let first_seq = digits(parts.next()?, 12)?;
        let last_seq = digits(parts.next()?, 12)?;
        let hash12 = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        SegmentName::new(epoch, first_seq, last_seq, hash12)
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    pub fn first_seq(&self) -> u64 {
        self.first_seq
    }

    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    pub fn hash12(&self) -> &str {
        &self.hash12
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

/// `<seq:012>-<hash12>.fsnap`. Built only through checks, as [`SegmentName`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SnapName {
    seq: u64,
    hash12: String,
}

impl SnapName {
    /// `None` unless `seq ≤ MAX_SEQ` and `hash12` is 12 lowercase hex digits.
    pub fn new(seq: u64, hash12: &str) -> Option<Self> {
        (seq <= MAX_SEQ && is_hex12(hash12)).then(|| SnapName {
            seq,
            hash12: hash12.to_string(),
        })
    }

    pub fn parse(name: &str) -> Option<Self> {
        let (seq, hash) = name.strip_suffix(".fsnap")?.split_once('-')?;
        SnapName::new(digits(seq, 12)?, hash)
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn hash12(&self) -> &str {
        &self.hash12
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
    is_hex12(short).then_some(short)
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
fn is_hex12(s: &str) -> bool {
    s.len() == 12
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
