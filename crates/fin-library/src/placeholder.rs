//! Placeholder rules (Decision 12), pure over [`RawEntry`] so every OS's bits are tested on
//! every host.
//!
//! - macOS: `SF_DATALESS` in `ATTR_CMN_FLAGS`, on files **and directories**;
//! - Windows, on any reparse tag (Decision 8): `RECALL_ON_OPEN`, `RECALL_ON_DATA_ACCESS`,
//!   `OFFLINE` and `UNPINNED` (online-only even with the other bits clear); `PINNED` with
//!   none of them is hydrated;
//! - every OS, by name, files only: `.<name>.icloud`, `<name>.nextcloud` and
//!   `<name>.owncloud` stand for `<name>` ([`stub`]); when `<name>` is listed too, the real
//!   file wins and the stub is **shadowed** ([`shadowed`]): no placeholder, never probed,
//!   excluded by [`crate::listing`].
//!
//! An attribute placeholder wins over a name: a dataless `x.nextcloud` is `dataless`.

use std::collections::HashSet;

use crate::source::{Attrs, RawEntry};
use crate::types::{EntryType, Placeholder};

/// macOS `<sys/stat.h>` `SF_DATALESS`: the item's data (a file's bytes, a directory's
/// listing) is not local.
pub const SF_DATALESS: u32 = 0x4000_0000;

/// Windows `FILE_ATTRIBUTE_*` bits the rules read.
pub mod win {
    pub const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    pub const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;
    pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0004_0000;
    pub const FILE_ATTRIBUTE_PINNED: u32 = 0x0008_0000;
    pub const FILE_ATTRIBUTE_UNPINNED: u32 = 0x0010_0000;
    pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
}

/// The placeholder an attribute word marks, if any. Several Windows bits: recall-on-open,
/// then recall-on-data, offline, unpinned.
pub fn from_attrs(attrs: Attrs) -> Option<Placeholder> {
    use win::*;
    match attrs {
        Attrs::None => None,
        Attrs::Macos { flags } => (flags & SF_DATALESS != 0).then_some(Placeholder::Dataless),
        Attrs::Windows { attributes: a } => [
            (FILE_ATTRIBUTE_RECALL_ON_OPEN, Placeholder::RecallOnOpen),
            (
                FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
                Placeholder::RecallOnData,
            ),
            (FILE_ATTRIBUTE_OFFLINE, Placeholder::Offline),
            (FILE_ATTRIBUTE_UNPINNED, Placeholder::Unpinned),
        ]
        .into_iter()
        .find_map(|(bit, p)| (a & bit != 0).then_some(p)),
    }
}

/// A stub file name and the name it stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stub {
    pub target: Vec<u8>,
    pub kind: Placeholder,
}

/// `.<name>.icloud` → `<name>` (iCloud), `<name>.nextcloud`, `<name>.owncloud` → `<name>`;
/// `None` for any other name or an empty, `.` or `..` target. Suffixes are exact (the clients
/// write them in lower case).
pub fn stub(name: &[u8]) -> Option<Stub> {
    let (target, kind) = if let Some(t) = name
        .strip_prefix(b".")
        .and_then(|n| n.strip_suffix(b".icloud"))
    {
        (t, Placeholder::IcloudStub)
    } else if let Some(t) = name.strip_suffix(b".nextcloud") {
        (t, Placeholder::Nextcloud)
    } else {
        (name.strip_suffix(b".owncloud")?, Placeholder::Owncloud)
    };
    (!target.is_empty() && target != b"." && target != b"..").then(|| Stub {
        target: target.to_vec(),
        kind,
    })
}

/// The entry's placeholder kind. `siblings` holds the raw names of the entry's directory
/// listing (the entry's own name may be among them): a stub whose target is listed is
/// shadowed and answers `None` (see [`shadowed`]).
pub fn classify(entry: &RawEntry, siblings: &HashSet<&[u8]>) -> Option<Placeholder> {
    if let Some(p) = from_attrs(entry.attrs) {
        return Some(p);
    }
    if entry.entry_type != EntryType::File {
        return None;
    }
    let s = stub(&entry.name)?;
    (!siblings.contains(s.target.as_slice())).then_some(s.kind)
}

/// A stub file (no attribute placeholder) whose real file is listed beside it.
pub fn shadowed(entry: &RawEntry, siblings: &HashSet<&[u8]>) -> bool {
    from_attrs(entry.attrs).is_none()
        && entry.entry_type == EntryType::File
        && stub(&entry.name).is_some_and(|s| siblings.contains(s.target.as_slice()))
}
