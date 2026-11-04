//! Conflict-copy name patterns of the common sync providers (sub-project 4 spec §3.7).
//!
//! Matching is case-insensitive and written by hand, so no regex crate is needed. Our own
//! canonical names are excluded first: none of them contains a space, a parenthesis or a
//! bracket, but a hostname or a hash could still collide with a suffix pattern, so the
//! exclusion is explicit rather than a property of the patterns.

use crate::names::{self, SegmentName, SnapName};

/// Which sync provider's naming a copy follows. `Unknown` marks a Finisher file stored under
/// a name that is not its own (see `scan`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Dropbox,
    Syncthing,
    ICloud,
    OneDrive,
    GoogleDrive,
    Unknown,
}

/// Whether `name` is a file name Finisher itself writes under `.finisher/`: a segment or
/// snapshot name, a temp name, `lease.json`, `lease.claim-<epoch>` or `lease.probe-<serverId>`.
pub fn is_own_name(name: &str) -> bool {
    let all = |s: &str, f: fn(u8) -> bool| !s.is_empty() && s.bytes().all(f);
    name == "lease.json"
        || name
            .strip_prefix("lease.claim-")
            .is_some_and(|e| all(e, |b| b.is_ascii_digit()))
        || name
            .strip_prefix("lease.probe-")
            .is_some_and(|id| all(id, |b| b.is_ascii_alphanumeric()))
        || names::is_temp(name)
        || SegmentName::parse(name).is_some()
        || SnapName::parse(name).is_some()
}

/// The provider whose conflict-copy pattern `name` matches, or `None`. Our own names never
/// match. `hostnames` are this machine's and any known peers' names for the OneDrive
/// `-<hostname>` suffix.
pub fn classify(name: &str, hostnames: &[String]) -> Option<Provider> {
    if is_own_name(name) {
        return None;
    }
    let lower = name.to_lowercase();
    if lower.contains("conflicted copy") {
        Some(Provider::Dropbox)
    } else if lower.contains(".sync-conflict-") {
        Some(Provider::Syncthing)
    } else if is_icloud(&lower) {
        Some(Provider::ICloud)
    } else if is_onedrive(&lower, hostnames) {
        Some(Provider::OneDrive)
    } else if lower.contains("[conflict]") || has_paren_number(&lower) {
        Some(Provider::GoogleDrive)
    } else {
        None
    }
}

/// `^(.*) \d+(\.[^.]+)?$`.
fn is_icloud(s: &str) -> bool {
    let space_digits = |t: &str| {
        let head = t.trim_end_matches(|c: char| c.is_ascii_digit());
        head.len() < t.len() && head.ends_with(' ')
    };
    if space_digits(s) {
        return true;
    }
    s.rsplit_once('.')
        .is_some_and(|(head, ext)| !ext.is_empty() && space_digits(head))
}

/// `-<hostname>` right before the extension (or at the end of an extensionless name).
fn is_onedrive(s: &str, hostnames: &[String]) -> bool {
    let stem = s.rsplit_once('.').map_or(s, |(stem, _)| stem);
    hostnames.iter().filter(|h| !h.is_empty()).any(|h| {
        let suffix = format!("-{}", h.to_lowercase());
        stem.ends_with(&suffix) || s.ends_with(&suffix)
    })
}

/// `\(\d+\)` anywhere.
fn has_paren_number(s: &str) -> bool {
    s.match_indices('(').any(|(i, _)| {
        let rest = &s[i + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        digits > 0 && rest.as_bytes().get(digits) == Some(&b')')
    })
}
