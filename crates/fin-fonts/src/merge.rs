//! Merging plugin packs' font catalogs into the server's (8A Decision 22): new entry ids
//! only, the merged catalog validated whole.

use std::collections::BTreeSet;

use fin_core::CoreError;
use serde_json::json;

use crate::catalog::FontCatalog;

/// Every entry and face family of `c`, case-folded.
fn families_of(c: &FontCatalog) -> BTreeSet<String> {
    let faces = c.entries.iter().flat_map(|e| {
        e.styles
            .iter()
            .flat_map(|s| s.file.faces.iter().map(|f| f.family.as_str()))
    });
    c.entries
        .iter()
        .map(|e| e.family.as_str())
        .chain(faces)
        .map(str::to_lowercase)
        .collect()
}

impl FontCatalog {
    /// The first family of this catalog (entry, then its faces, in order) that `taken` holds,
    /// with its path below the catalog's root.
    fn family_taken(&self, taken: &BTreeSet<String>) -> Option<(String, String)> {
        for (i, e) in self.entries.iter().enumerate() {
            if taken.contains(&e.family.to_lowercase()) {
                return Some((format!("/entries/{i}/family"), e.family.clone()));
            }
            for (j, s) in e.styles.iter().enumerate() {
                for (l, f) in s.file.faces.iter().enumerate() {
                    if taken.contains(&f.family.to_lowercase()) {
                        let path = format!("/entries/{i}/styles/{j}/file/faces/{l}/family");
                        return Some((path, f.family.clone()));
                    }
                }
            }
        }
        None
    }

    /// This catalog with the entries of each of `extra` appended, in order.
    ///
    /// - An entry id this catalog or an earlier `extra` already has is refused: nothing a
    ///   pack ships replaces or shadows another entry (`FONT_CATALOG_INVALID`, rule
    ///   `catalogIdTaken`, `details {rule, path, id}` with `path` into `extra`).
    /// - A family this catalog or an earlier `extra` already has is refused too, whether it
    ///   names a pack entry or one of its faces and whether the other side names an entry or a
    ///   face (compared without case): `find_face` matches face families before entry
    ///   families, so a pack face called `Inter` would win new pins for `Inter` over the
    ///   built-in `Inter Variable` (`catalogIdTaken`, `details {rule, path, id, family}`;
    ///   review H1 of Tasks 10 and 12). Two entries of one pack may share a family.
    /// - A pack's `fallback` ids go after this catalog's order (a script this catalog's
    ///   fallbacks cover keeps its font; one they do not may find a pack's), each once.
    /// - `catalog_version` stays this catalog's: pins name the version they were planned at,
    ///   and a pack's catalog does not move it.
    /// - The result passes [`FontCatalog::validate`] again, so a file or face the packs
    ///   list twice between them (the same bytes under two ids) is refused as `unique`.
    ///
    /// Every pack's catalog is validated on its own first, by whoever parsed it
    /// ([`FontCatalog::from_json`]): its licences, https and immutable source URLs, source
    /// hosts, sizes and SHA-256 pins are the built-in catalog's rules.
    pub fn merged(&self, extra: &[FontCatalog]) -> Result<FontCatalog, CoreError> {
        let mut out = self.clone();
        let mut ids: BTreeSet<String> = out.entries.iter().map(|e| e.id.clone()).collect();
        let mut families = families_of(&out);
        for (k, pack) in extra.iter().enumerate() {
            for (i, entry) in pack.entries.iter().enumerate() {
                if !ids.insert(entry.id.clone()) {
                    let path = format!("/extra/{k}/entries/{i}/id");
                    return Err(CoreError::new(
                        "FONT_CATALOG_INVALID",
                        format!("{path}: the catalog already has an entry {}", entry.id),
                    )
                    .with_details(json!({
                        "rule": "catalogIdTaken", "path": path, "id": entry.id,
                    })));
                }
            }
            if let Some((rel, family)) = pack.family_taken(&families) {
                let path = format!("/extra/{k}{rel}");
                return Err(CoreError::new(
                    "FONT_CATALOG_INVALID",
                    format!("{path}: the catalog already has the family {family}"),
                )
                .with_details(json!({
                    "rule": "catalogIdTaken", "path": path, "id": family, "family": family,
                })));
            }
            families.extend(families_of(pack));
            out.entries.extend(pack.entries.iter().cloned());
            for id in &pack.fallback {
                if !out.fallback.contains(id) {
                    out.fallback.push(id.clone());
                }
            }
        }
        out.validate()?;
        Ok(out)
    }

    /// The first of `extra` that does not merge into this catalog after the ones before it,
    /// and the [`FontCatalog::merged`] error it gets (its `details.path` reads `/extra/0/…`):
    /// the rule a pack's catalogs are held to when the pack is planned or validated (8A Task
    /// 12c), so a pack that would be left out at sync is refused before it installs.
    pub fn first_unmergeable(&self, extra: &[FontCatalog]) -> Option<(usize, CoreError)> {
        let mut catalog = self.clone();
        for (index, one) in extra.iter().enumerate() {
            match catalog.merged(std::slice::from_ref(one)) {
                Ok(merged) => catalog = merged,
                Err(e) => return Some((index, e)),
            }
        }
        None
    }
}
