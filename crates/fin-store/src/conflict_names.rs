//! Conflict-copy names of the common sync providers (sub-project 4 spec §3.7).
//!
//! A name is a conflict copy of provider P when removing P's marker leaves one of our own
//! canonical names (segment, snapshot or lease file). Litter that merely looks like a marker
//! (`notes 2`, `Untitled 2.txt`) is therefore not a conflict copy. Matching is
//! case-insensitive and hand-written, so no regex crate is needed.
//!
//! Markers, per provider:
//! - Dropbox: a parenthesised group containing `conflicted copy`, `Case Conflict` or
//!   `Selective Sync Conflict` (`name (Bob's conflicted copy 2026-11-04).ext`,
//!   `name (Case Conflict 1).ext`), with its leading space;
//! - Syncthing: `.sync-conflict-<date>-<time>-<id>` before the extension;
//! - iCloud: ` <n>` before the extension (`name 2.ext`);
//! - OneDrive: `-<hostname>` before the extension. Hostnames contain hyphens, so every `-`
//!   split of the stem is tried;
//! - Google Drive (unverified): `(<n>)` or `[Conflict]`, with a leading space if any.

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

/// `lease.json`, `lease.claim-<epoch>` or `lease.probe-<serverId>`: the files Finisher owns
/// directly in `.finisher/`.
pub fn is_lease_name(name: &str) -> bool {
    let all = |s: &str, f: fn(u8) -> bool| !s.is_empty() && s.bytes().all(f);
    name == "lease.json"
        || name
            .strip_prefix("lease.claim-")
            .is_some_and(|e| all(e, |b| b.is_ascii_digit()))
        || name
            .strip_prefix("lease.probe-")
            .is_some_and(|id| all(id, |b| b.is_ascii_alphanumeric()))
}

/// Any canonical name Finisher writes: lease files, segments, snapshots and temp files.
pub fn is_own_name(name: &str) -> bool {
    is_canonical(name) || names::is_temp(name)
}

fn is_canonical(name: &str) -> bool {
    is_lease_name(name) || SegmentName::parse(name).is_some() || SnapName::parse(name).is_some()
}

/// The provider whose conflict-copy pattern turns one of our names into `name`, or `None`.
/// Our own names never match.
pub fn classify(name: &str) -> Option<Provider> {
    if is_own_name(name) {
        return None;
    }
    classify_with(name, is_canonical)
}

/// A branch directory's conflict copy (`main (conflicted copy …)`, `main 2`, …): removing the
/// marker leaves a branch name (`main`, `br<digits>`).
pub fn classify_branch_dir(name: &str) -> Option<Provider> {
    if crate::scan::valid_branch(name) {
        return None;
    }
    classify_with(name, crate::scan::valid_branch)
}

/// A conflict copy of the project file (`finisher.project (… conflicted copy …).json`, …).
pub fn classify_project_file(name: &str) -> Option<Provider> {
    if name == crate::PROJECT_FILE {
        return None;
    }
    classify_with(name, |c| c == crate::PROJECT_FILE)
}

/// The first provider whose marker, removed from `name`, leaves a name `ours` accepts.
fn classify_with(name: &str, ours: impl Fn(&str) -> bool) -> Option<Provider> {
    // ASCII lowering keeps byte offsets; our canonical names are lowercase (lease probe ids
    // are alphanumeric either way).
    let s = name.to_ascii_lowercase();
    let any = |c: Vec<String>| c.iter().any(|c| ours(c));
    if any(dropbox(&s)) {
        Some(Provider::Dropbox)
    } else if any(syncthing(&s)) {
        Some(Provider::Syncthing)
    } else if any(icloud(&s)) {
        Some(Provider::ICloud)
    } else if any(onedrive(&s)) {
        Some(Provider::OneDrive)
    } else if any(google(&s)) {
        Some(Provider::GoogleDrive)
    } else {
        None
    }
}

/// A conflict copy of a lease probe file: each server writes only its own probe, so a copy
/// is harmless (listed, never read-only).
pub fn is_probe_copy(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("lease.probe-") && !is_lease_name(name)
}

/// `s` without `s[start..end]`, widening `start` over one preceding space.
fn cut(s: &str, start: usize, end: usize) -> String {
    let start = if start > 0 && s.as_bytes()[start - 1] == b' ' {
        start - 1
    } else {
        start
    };
    format!("{}{}", &s[..start], &s[end..])
}

fn dropbox(s: &str) -> Vec<String> {
    const MARKERS: [&str; 3] = [
        "conflicted copy",
        "case conflict",
        "selective sync conflict",
    ];
    s.match_indices('(')
        .filter_map(|(i, _)| {
            let j = i + s[i..].find(')')?;
            MARKERS
                .iter()
                .any(|m| s[i + 1..j].contains(m))
                .then(|| cut(s, i, j + 1))
        })
        .collect()
}

fn syncthing(s: &str) -> Vec<String> {
    s.match_indices(".sync-conflict-")
        .map(|(i, _)| {
            let end = s[i + 1..].find('.').map_or(s.len(), |k| i + 1 + k);
            format!("{}{}", &s[..i], &s[end..])
        })
        .collect()
}

/// `t` without a trailing ` <digits>`.
fn strip_space_digits(t: &str) -> Option<&str> {
    let head = t.trim_end_matches(|c: char| c.is_ascii_digit());
    (head.len() < t.len()).then_some(head)?.strip_suffix(' ')
}

fn icloud(s: &str) -> Vec<String> {
    let mut v: Vec<String> = strip_space_digits(s)
        .map(String::from)
        .into_iter()
        .collect();
    if let Some((head, ext)) = s.rsplit_once('.')
        && !ext.is_empty()
        && let Some(h) = strip_space_digits(head)
    {
        v.push(format!("{h}.{ext}"));
    }
    v
}

fn onedrive(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let with_ext = s.rsplit_once('.').map(|(stem, _)| (stem, &s[stem.len()..]));
    for (stem, ext) in with_ext.into_iter().chain([(s, "")]) {
        for (i, _) in stem.match_indices('-') {
            if i + 1 < stem.len() {
                v.push(format!("{}{ext}", &stem[..i]));
            }
        }
    }
    v
}

fn google(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    for (i, _) in s.match_indices('(') {
        let rest = &s[i + 1..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && rest.as_bytes().get(digits) == Some(&b')') {
            v.push(cut(s, i, i + digits + 2));
        }
    }
    for (i, m) in s.match_indices("[conflict]") {
        v.push(cut(s, i, i + m.len()));
    }
    v
}
