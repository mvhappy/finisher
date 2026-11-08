//! `Registry`'s bundled-plugin calls (8A plan Task 10): the compiled-in list, and the check of
//! the app's resources folder against it. A bundled plugin is planned through `plan` with
//! `source: {kind: "bundled", root}`; the hash it must match is the one compiled into this
//! binary, looked up here by id, never one the caller passes. Same worker thread and promise
//! discipline as the other calls.

use std::path::PathBuf;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use super::{Registry, on_worker};
use crate::bundled::{self, Entry};
use crate::encode;

#[napi]
impl Registry {
    /// The bundled plugins compiled into this binary: `[{ id, version, treeHash }]` JSON.
    #[napi(catch_unwind)]
    pub fn bundled_list(&self) -> Result<String> {
        let list: Vec<Entry> = self.inner.bundled_entries();
        encode(&list)
    }

    /// Re-hashes each bundled plugin's tree under `root` and compares it with the compiled-in
    /// hash: `[{ id, version, treeHash, ok, code?, reason? }]` JSON (`code` is
    /// `PLUGIN_BUNDLED_MISMATCH` when `ok` is false). Nothing is installed or changed.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn bundled_verify<'e>(&self, env: &'e Env, root: String) -> Result<Object<'e>> {
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            encode(&bundled::verify(
                &PathBuf::from(root),
                &inner.bundled_entries(),
            ))
        })
    }
}

#[cfg(feature = "test-hooks")]
#[napi]
impl Registry {
    /// Test builds only: replaces the compiled-in list (`[{ id, version, treeHash }]` JSON)
    /// for this `Registry`, or restores it with `null`.
    #[napi(catch_unwind)]
    pub fn set_bundled_for_test(&self, list_json: Option<String>) -> Result<()> {
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Row {
            id: String,
            version: String,
            tree_hash: String,
        }
        let list = match list_json {
            None => None,
            Some(s) => Some(
                serde_json::from_str::<Vec<Row>>(&s)
                    .map_err(|e| super::bad_request("the bundled list", e))?
                    .into_iter()
                    .map(|r| Entry {
                        id: r.id,
                        version: r.version,
                        tree_hash: r.tree_hash,
                    })
                    .collect(),
            ),
        };
        *self
            .inner
            .bundled_override
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = list;
        Ok(())
    }
}
