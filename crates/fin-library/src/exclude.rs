//! What a scan leaves out (Decision 16, spec §7.1): the default rules by name, the library's
//! `exclude` and `include` globs over `relKey` ([`crate::glob`]), and the listing rules that
//! [`crate::listing`] applies (automounts, project folders, shadowed stubs).
//!
//! Default rules, on the NFC name (junk names compared ASCII case-insensitively):
//! - dot-files and dot-directories (`.DS_Store`, `.nfs*`, `.fuse_hidden*`, `.Trash`, …);
//! - OS and NAS junk: `Thumbs.db`, `desktop.ini`, `@eaDir`, `#recycle`, `$RECYCLE.BIN`,
//!   `System Volume Information`;
//! - sync and download temp files: `~$*`, `~syncthing~*`, `*.!sync`, `*.crdownload`,
//!   `*.partial`, `*.tmp`;
//! - conflict copies by the **unambiguous** markers of the providers `fin_store`'s
//!   `conflict_names` knows ([`conflict_copy`]).
//!
//! An excluded directory is not descended. `include`, when set, keeps only the **files**
//! that match one of its globs (directories are still descended, since any of them may hold
//! a match); the default rules and `exclude` apply first.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::glob::{Glob, GlobError};

/// Exclusion codes ([`crate::types::ScanEntry::excluded`]).
pub mod code {
    pub const DOTFILE: &str = "DOTFILE";
    pub const OS_JUNK: &str = "OS_JUNK";
    pub const SYNC_TEMP: &str = "SYNC_TEMP";
    pub const CONFLICT_COPY: &str = "CONFLICT_COPY";
    /// A Linux automount point (Decision 5): never descended, so never triggered.
    pub const AUTOMOUNT: &str = "AUTOMOUNT";
    /// Inside a Finisher project folder, anything but `sources/`.
    pub const PROJECT_INTERNAL: &str = "PROJECT_INTERNAL";
    /// A stub whose real file is listed beside it (Decision 12).
    pub const STUB_SHADOWED: &str = "STUB_SHADOWED";
    /// Matched one of the library's `exclude` globs.
    pub const EXCLUDE_GLOB: &str = "EXCLUDE_GLOB";
    /// A file that matched none of the library's `include` globs.
    pub const NOT_INCLUDED: &str = "NOT_INCLUDED";
}

/// The project file that makes a directory a Finisher project (`fin_store::PROJECT_FILE`).
pub const PROJECT_FILE: &[u8] = b"finisher.project.json";
/// The one entry of a project folder a library scans.
pub const PROJECT_SOURCES: &[u8] = b"sources";

const JUNK: [&str; 6] = [
    "thumbs.db",
    "desktop.ini",
    "@eadir",
    "#recycle",
    "$recycle.bin",
    "system volume information",
];
const TEMP_PREFIXES: [&str; 2] = ["~$", "~syncthing~"];
const TEMP_SUFFIXES: [&str; 4] = [".!sync", ".crdownload", ".partial", ".tmp"];

/// The library's patterns, as stored and as sent to a scan worker.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExcludeSpec {
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include: Option<Vec<String>>,
    /// The root's case sensitivity: keys and patterns are folded when `false`.
    pub case_sensitive: bool,
}

/// Compiled [`ExcludeSpec`].
#[derive(Debug, Clone)]
pub struct Excludes {
    exclude: Vec<Glob>,
    include: Option<Vec<Glob>>,
    case_sensitive: bool,
}

impl Excludes {
    pub fn new(spec: &ExcludeSpec) -> Result<Excludes, GlobError> {
        let compile = |ps: &[String]| -> Result<Vec<Glob>, GlobError> {
            ps.iter()
                .map(|p| Glob::new(p, spec.case_sensitive))
                .collect()
        };
        Ok(Excludes {
            exclude: compile(&spec.exclude)?,
            include: spec.include.as_deref().map(compile).transpose()?,
            case_sensitive: spec.case_sensitive,
        })
    }

    /// The default rules only.
    pub fn defaults(case_sensitive: bool) -> Excludes {
        Excludes {
            exclude: Vec::new(),
            include: None,
            case_sensitive,
        }
    }

    pub fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// The exclusion code of an entry by its NFC name and key, or `None` to keep it.
    pub fn check(&self, rel_key: &str, name_nfc: &str, is_dir: bool) -> Option<&'static str> {
        if let Some(c) = default_rule(name_nfc, is_dir) {
            return Some(c);
        }
        if self.exclude.iter().any(|g| g.matches(rel_key)) {
            return Some(code::EXCLUDE_GLOB);
        }
        match &self.include {
            Some(inc) if !is_dir && !inc.iter().any(|g| g.matches(rel_key)) => {
                Some(code::NOT_INCLUDED)
            }
            _ => None,
        }
    }
}

/// The default rules over an NFC name.
pub fn default_rule(name: &str, is_dir: bool) -> Option<&'static str> {
    if name.starts_with('.') {
        return Some(code::DOTFILE);
    }
    let lower = name.to_ascii_lowercase();
    if JUNK.contains(&lower.as_str()) {
        return Some(code::OS_JUNK);
    }
    if !is_dir
        && (TEMP_PREFIXES.iter().any(|p| lower.starts_with(p))
            || TEMP_SUFFIXES
                .iter()
                .any(|s| lower.ends_with(s) && lower.len() > s.len()))
    {
        return Some(code::SYNC_TEMP);
    }
    conflict_copy(name).then_some(code::CONFLICT_COPY)
}

/// A conflict copy by an unambiguous marker (ASCII case-insensitive):
/// - Dropbox and Nextcloud: a parenthesised group holding `conflicted copy`,
///   `case conflict` or `selective sync conflict`;
/// - Syncthing: `.sync-conflict-` followed by its date;
/// - ownCloud: `_conflict-` followed by a date and time of digits (`_conflict-20261107-101500`);
/// - Google Drive: `[Conflict]`.
///
/// `fin_store::conflict_names::classify` also knows iCloud's ` <n>`, OneDrive's `-<host>` and
/// Google's `(<n>)`, but it only accepts them when the unmarked name is one of Finisher's own;
/// over arbitrary media names (`Take 2.mov`, `clip-final.mov`, `scan (1).png`) those markers
/// would drop real files, so a library keeps them.
pub fn conflict_copy(name: &str) -> bool {
    let s = name.to_ascii_lowercase();
    const PAREN: [&str; 3] = [
        "conflicted copy",
        "case conflict",
        "selective sync conflict",
    ];
    let paren = s.match_indices('(').any(|(i, _)| {
        s[i..]
            .find(')')
            .is_some_and(|j| PAREN.iter().any(|m| s[i + 1..i + j].contains(m)))
    });
    let digits_after = |marker: &str| {
        s.match_indices(marker).any(|(i, m)| {
            s.as_bytes()
                .get(i + m.len())
                .is_some_and(u8::is_ascii_digit)
        })
    };
    paren
        || digits_after(".sync-conflict-")
        || digits_after("_conflict-")
        || s.contains("[conflict]")
}
