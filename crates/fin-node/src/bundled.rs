//! The bundled plugins this binary was built with (8A plan Task 10, Decision 24): the
//! compiled-in `BUNDLED_PLUGINS` list `build.rs` generated from `resources/plugins/bundled/`,
//! and the check of a resources folder against it.
//!
//! The list detects corruption and partial updates of the resources. Before sub-project 11's
//! code signing nothing else protects them: a user (or malware with the user's rights) who
//! rewrites both the resources and this binary is not detected (Decision 25), which is why
//! `finisherd` reports `bundledIntegrity: hashesOnly`.

use std::path::Path;

use fin_registry::package::tree_hash;
use serde::Serialize;

include!(concat!(env!("OUT_DIR"), "/bundled_plugins.rs"));

/// One bundled plugin as compiled in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Entry {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) tree_hash: String,
}

/// The compiled-in list, in id order.
pub(crate) fn compiled() -> Vec<Entry> {
    BUNDLED_PLUGINS
        .iter()
        .map(|(id, version, tree_hash)| Entry {
            id: (*id).into(),
            version: (*version).into(),
            tree_hash: (*tree_hash).into(),
        })
        .collect()
}

impl Entry {
    /// The folder under the resources root: the id without its `@finisher/`.
    pub(crate) fn folder(&self) -> &str {
        self.id.rsplit('/').next().unwrap_or(&self.id)
    }
}

/// How one bundled tree compared with the compiled-in hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Verified {
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) tree_hash: String,
    pub(crate) ok: bool,
    /// `PLUGIN_BUNDLED_MISMATCH` when `ok` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) code: Option<&'static str>,
    /// `missing` (no such folder), `unreadable` (a symlink, a special file, an I/O error) or
    /// `treeHash` (readable, different).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'static str>,
}

/// Re-hashes every entry's tree under `root` (`root/<folder>`).
pub(crate) fn verify(root: &Path, entries: &[Entry]) -> Vec<Verified> {
    entries
        .iter()
        .map(|e| {
            let dir = root.join(e.folder());
            let reason = if !dir.is_dir() {
                Some("missing")
            } else {
                match tree_hash(&dir) {
                    Ok(h) if h == e.tree_hash => None,
                    Ok(_) => Some("treeHash"),
                    Err(_) => Some("unreadable"),
                }
            };
            Verified {
                id: e.id.clone(),
                version: e.version.clone(),
                tree_hash: e.tree_hash.clone(),
                ok: reason.is_none(),
                code: reason.map(|_| "PLUGIN_BUNDLED_MISMATCH"),
                reason,
            }
        })
        .collect()
}
