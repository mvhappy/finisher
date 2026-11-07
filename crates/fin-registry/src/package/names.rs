//! Entry names (spec §6.4, §14): relative NFC UTF-8 paths that mean the same file on every
//! OS. Refused: empty, `.` and `..` components, a leading `/`, `\`, NUL and other control
//! characters, `:` (drive letters, NTFS streams) and Windows' other forbidden characters,
//! Windows reserved device names in any case and with any extension, trailing dots and spaces,
//! components over 255 bytes and paths over 1,024, names that are not NFC, NTFS 8.3 short
//! names (`PROGRA~1`, `SOMETH~12.JSO`: on Windows one may be another entry's alias), and two
//! paths that differ only by case (a file and a directory included).

use std::collections::{BTreeMap, BTreeSet};

use unicode_normalization::is_nfc;

use super::{Limits, PackageError};

/// Windows device names, reserved in any case and with any extension (`aux.txt`).
const RESERVED: &[&str] = &[
    "CON",
    "PRN",
    "AUX",
    "NUL",
    "CONIN$",
    "CONOUT$",
    "CLOCK$",
    "COM0",
    "COM1",
    "COM2",
    "COM3",
    "COM4",
    "COM5",
    "COM6",
    "COM7",
    "COM8",
    "COM9",
    "COM\u{b9}",
    "COM\u{b2}",
    "COM\u{b3}",
    "LPT0",
    "LPT1",
    "LPT2",
    "LPT3",
    "LPT4",
    "LPT5",
    "LPT6",
    "LPT7",
    "LPT8",
    "LPT9",
    "LPT\u{b9}",
    "LPT\u{b2}",
    "LPT\u{b3}",
];

/// Characters Windows refuses in a name (besides control characters and `\`).
const WINDOWS_FORBIDDEN: &[char] = &['<', '>', ':', '"', '|', '?', '*'];

/// Whether `c` has the shape of an NTFS 8.3 short name with a numeric tail: a base of at most
/// 8 characters ending in `~<digits>`, then at most one extension of at most 3 characters.
/// Windows may give an entry with another name this alias, so the two would be one file.
fn is_short_name(c: &str) -> bool {
    let (base, ext) = match c.split_once('.') {
        Some((b, e)) => (b, Some(e)),
        None => (c, None),
    };
    if ext.is_some_and(|e| e.is_empty() || e.len() > 3 || e.contains('.')) {
        return false;
    }
    let Some((stem, tail)) = base.rsplit_once('~') else {
        return false;
    };
    base.len() <= 8
        && !stem.is_empty()
        && !tail.is_empty()
        && tail.bytes().all(|b| b.is_ascii_digit())
}

/// Checks one path component; `Err` is `(reason, why)`.
pub fn check_component(c: &str, limits: &Limits) -> Result<(), (&'static str, String)> {
    if c.is_empty() {
        return Err(("emptyComponent", "an empty path component".into()));
    }
    if c == "." || c == ".." {
        return Err(("dotComponent", format!("a {c:?} component")));
    }
    if c.len() > limits.component_bytes {
        return Err((
            "componentTooLong",
            format!(
                "a component of {} bytes; at most {}",
                c.len(),
                limits.component_bytes
            ),
        ));
    }
    if let Some(ch) = c.chars().find(|ch| WINDOWS_FORBIDDEN.contains(ch)) {
        return Err(("windowsChar", format!("{ch:?} is not allowed in a name")));
    }
    if c.ends_with('.') || c.ends_with(' ') {
        return Err((
            "trailingDotOrSpace",
            "a name ending in a dot or a space".into(),
        ));
    }
    if is_short_name(c) {
        return Err((
            "shortName",
            format!("{c:?} has the shape of a Windows 8.3 short name"),
        ));
    }
    let stem = c.split('.').next().unwrap_or(c).trim_end_matches(' ');
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(stem)) {
        return Err(("reservedName", format!("{stem:?} is a Windows device name")));
    }
    Ok(())
}

/// Checks a whole package path (no trailing `/`).
pub fn check_path(p: &str, limits: &Limits) -> Result<(), (&'static str, String)> {
    if p.is_empty() {
        return Err(("emptyName", "an empty name".into()));
    }
    if p.contains('\0') {
        return Err(("nul", "a NUL in the name".into()));
    }
    if p.chars().any(char::is_control) {
        return Err(("control", "a control character in the name".into()));
    }
    if p.contains('\\') {
        return Err(("backslash", "a backslash in the name".into()));
    }
    if p.starts_with('/') {
        return Err(("absolute", "an absolute path".into()));
    }
    if p.len() > limits.path_bytes {
        return Err((
            "pathTooLong",
            format!("a path of {} bytes; at most {}", p.len(), limits.path_bytes),
        ));
    }
    if !is_nfc(p) {
        return Err(("notNfc", "a name that is not in Unicode NFC".into()));
    }
    for c in p.split('/') {
        check_component(c, limits)?;
    }
    Ok(())
}

/// Case-insensitive keys: lower and upper case (`ß` and `SS` meet under upper case).
fn folds(p: &str) -> [String; 2] {
    [p.to_lowercase(), p.to_uppercase()]
}

/// The paths of one package, checked as they are added.
pub(crate) struct Names {
    limits: Limits,
    files: BTreeSet<String>,
    dirs: BTreeSet<String>,
    /// Fold key → the exact path that took it.
    taken: BTreeMap<String, String>,
}

impl Names {
    pub(crate) fn new(limits: Limits) -> Names {
        Names {
            limits,
            files: BTreeSet::new(),
            dirs: BTreeSet::new(),
            taken: BTreeMap::new(),
        }
    }

    fn take(&mut self, path: &str, entry: &str) -> Result<(), PackageError> {
        for key in folds(path) {
            match self.taken.get(&key) {
                Some(have) if have != path => {
                    return Err(PackageError::unsafe_entry(
                        entry,
                        "caseFoldDuplicate",
                        format!("{path:?} and {have:?} differ only by case"),
                    ));
                }
                Some(_) => {}
                None => {
                    self.taken.insert(key, path.to_string());
                }
            }
        }
        Ok(())
    }

    /// Adds an entry (`entry` is its raw name, for errors); `dir` for a directory entry.
    pub(crate) fn add(&mut self, entry: &str, path: &str, dir: bool) -> Result<(), PackageError> {
        check_path(path, &self.limits)
            .map_err(|(reason, why)| PackageError::unsafe_entry(entry, reason, why))?;
        let parts: Vec<&str> = path.split('/').collect();
        for i in 1..parts.len() {
            let parent = parts[..i].join("/");
            if self.files.contains(&parent) {
                return Err(PackageError::unsafe_entry(
                    entry,
                    "fileAsDirectory",
                    format!("{parent:?} is a file and a directory"),
                ));
            }
            self.take(&parent, entry)?;
            self.dirs.insert(parent);
        }
        if self.files.contains(path) {
            return Err(PackageError::unsafe_entry(
                entry,
                "duplicate",
                "a second entry of one path",
            ));
        }
        if dir {
            self.take(path, entry)?;
            self.dirs.insert(path.to_string());
        } else {
            if self.dirs.contains(path) {
                return Err(PackageError::unsafe_entry(
                    entry,
                    "fileAsDirectory",
                    format!("{path:?} is a file and a directory"),
                ));
            }
            self.take(path, entry)?;
            self.files.insert(path.to_string());
        }
        Ok(())
    }

    /// Whether a top-level component (any case) is taken by an entry.
    pub(crate) fn top_level_taken(&self, name: &str) -> bool {
        folds(name).iter().any(|k| self.taken.contains_key(k))
    }
}

#[cfg(test)]
mod tests {
    use super::is_short_name;

    #[test]
    fn short_names() {
        for c in [
            "PROGRA~1",
            "progra~1",
            "A~1",
            "SOMETH~1.JSO",
            "ab~12.js",
            "X~123456",
            "a~1.b",
        ] {
            assert!(is_short_name(c), "{c}");
        }
        for c in [
            "PROGRA~1.json",
            "LONGNAME~1",
            "a~",
            "~1",
            "a~1x",
            "a~1.",
            "a~1.b.c",
            "notes~draft.txt",
            "file.txt",
            "a~b~1.txt.bak",
        ] {
            assert!(!is_short_name(c), "{c}");
        }
    }
}
