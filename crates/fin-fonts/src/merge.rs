//! Merging plugin packs' font catalogs into the server's (8A Decision 22): new entry ids
//! only, the merged catalog validated whole.

use std::collections::BTreeSet;

use fin_core::CoreError;
use serde_json::json;

use crate::catalog::FontCatalog;

impl FontCatalog {
    /// This catalog with the entries of each of `extra` appended, in order.
    ///
    /// - An entry id this catalog or an earlier `extra` already has is refused: nothing a
    ///   pack ships replaces or shadows another entry (`FONT_CATALOG_INVALID`, rule
    ///   `catalogIdTaken`, `details {rule, path, id}` with `path` into `extra`).
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
}
