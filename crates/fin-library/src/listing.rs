//! One directory listing as scan entries (pure): names and keys, placeholders and stubs,
//! links, and every exclusion rule, in one place so every walk mode (Task 4) reports a
//! listing the same way.
//!
//! Per entry, in raw-name order (the order stubs claim their targets in); the entries are
//! then **sorted by their reported name**:
//! 1. a Windows reparse point with a name-surrogate tag becomes `symlink` (Decision 8):
//!    consumers then need one rule, "a symlink is skipped and never followed";
//! 2. its placeholder ([`crate::placeholder::classify`]); a stub that stands for `<name>` is
//!    reported **as `<name>`** with the stub kind, so the row survives hydration; a stub
//!    whose target is listed, or that a stub earlier in raw order already claimed, keeps its
//!    own name and is `STUB_SHADOWED`;
//! 3. `name_nfc`, `name_lossy`, `rel_key` of the reported name;
//! 4. `excluded`: shadowed stub, then `AUTOMOUNT`, then `PROJECT_INTERNAL` (the directory
//!    holds `finisher.project.json`; only a `sources` directory is kept), then
//!    [`Excludes::check`].

use std::collections::HashSet;

use crate::exclude::{self, Excludes, code};
use crate::names;
use crate::placeholder;
use crate::raw::RawRel;
use crate::reparse;
use crate::source::RawEntry;
use crate::types::{EntryType, ScanEntry};

/// The scan entries of the listing of `dir`, sorted by reported name (a stub's target, else
/// its raw name; reported names are unique within a listing).
pub fn entries(dir: &RawRel, mut raw: Vec<RawEntry>, excludes: &Excludes) -> Vec<ScanEntry> {
    raw.sort_by(|a, b| a.name.cmp(&b.name));
    let case_sensitive = excludes.case_sensitive();
    let dir_key = names::dir_key(dir, case_sensitive);
    let listed: HashSet<&[u8]> = raw.iter().map(|e| e.name.as_slice()).collect();
    let project = listed.contains(exclude::PROJECT_FILE);
    let mut claimed: HashSet<Vec<u8>> = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for e in &raw {
        let entry_type = match e.reparse_tag {
            Some(t) if reparse::is_name_surrogate(t) => EntryType::Symlink,
            _ => e.entry_type,
        };
        let mut placeholder = placeholder::classify(e, &listed);
        let mut shadowed = placeholder::shadowed(e, &listed);
        let mut name = e.name.clone();
        if placeholder.is_some()
            && placeholder::from_attrs(e.attrs).is_none()
            && let Some(stub) = placeholder::stub(&e.name)
        {
            if claimed.insert(stub.target.clone()) {
                name = stub.target;
            } else {
                placeholder = None;
                shadowed = true;
            }
        }
        let n = names::nfc(&name);
        let rel_key = names::rel_key(&dir_key, &n.nfc, case_sensitive);
        let is_dir = entry_type == EntryType::Dir;
        let excluded = if shadowed {
            Some(code::STUB_SHADOWED)
        } else if e.automount {
            Some(code::AUTOMOUNT)
        } else if project && !(is_dir && name == exclude::PROJECT_SOURCES) {
            Some(code::PROJECT_INTERNAL)
        } else {
            excludes.check(&rel_key, &n.nfc, is_dir)
        };
        out.push(ScanEntry {
            dir: dir.clone(),
            name,
            name_nfc: n.nfc,
            name_lossy: n.lossy,
            rel_key,
            entry_type,
            size: e.size,
            mtime_ns: e.mtime_ns,
            file_id: e.file_id.clone(),
            dev: e.dev.clone(),
            mount_root: e.mount_root,
            automount: e.automount,
            reparse_tag: e.reparse_tag,
            placeholder,
            excluded: excluded.map(str::to_owned),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}
