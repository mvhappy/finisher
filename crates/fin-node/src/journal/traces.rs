//! The agent-trace store (sp7 §12.1; plan 7A Task 10): a second `fin_journal::Journal` of
//! the same code in `<server-data>/agent-traces`, training only, with its own caps and
//! whole-session eviction, and the calls content emitters need (`training`,
//! `recordsProject`, `attachBlob`).

use std::path::PathBuf;
use std::sync::Arc;

use fin_core::events::names::MEDIA_TYPES;
use fin_core::events::payloads::Body;
use fin_journal::{EnvOverrides, JournalOptions};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use super::{JournalHandle, for_key, key_of, options, privacy_value, warning_of};
use crate::encode;

#[napi]
impl JournalHandle {
    /// Opens the agent-trace store of `server_data` (`<server-data>/agent-traces`; sp7
    /// §12.1): a second journal of the same code that records training only (nothing, and no
    /// directory, unless training is on), with its own caps (`privacy.tracesMaxMiB`, default
    /// 2048; `privacy.tracesRetentionDays`, default 90) and whole-session eviction. It follows
    /// the project policies of the diagnostics journal open on the same server data (open
    /// that first), keys ids and hashes with its install key (so the stores join; blob
    /// addresses stay the store's own), and engines never find it.
    #[napi(factory, catch_unwind)]
    pub fn open_traces(
        server_data: String,
        privacy_json: Option<String>,
        hostname: String,
        test_options_json: Option<String>,
    ) -> Result<JournalHandle> {
        let dir = PathBuf::from(&server_data);
        let key = key_of(&dir);
        let leader = for_key(&key);
        let mut opts = options(
            JournalOptions::agent_traces(),
            &dir,
            hostname,
            test_options_json.as_deref(),
        )?;
        // Ids and hashes keyed as the diagnostics journal keys them, so the stores join.
        opts.key = leader.shared_keyer();
        let (privacy, unparsable) = privacy_value(privacy_json.as_deref());
        let (journal, w) = fin_journal::Journal::open_with_settings(
            &dir.join("agent-traces"),
            privacy.as_ref(),
            EnvOverrides::from_env(),
            opts,
        );
        journal.follow_projects(&leader);
        Ok(JournalHandle {
            journal,
            key,
            warning: warning_of(w, unparsable),
            registered: false,
        })
    }

    /// Whether training is in force (content may be recorded): one atomic load.
    #[napi(getter, catch_unwind)]
    pub fn training(&self) -> bool {
        self.journal.gate().is_some_and(|p| p.has_training())
    }

    /// Whether an entry of `project_id` would be recorded now (`default`; not `never`, not
    /// unknown). Checked before content and blobs are built.
    #[napi(catch_unwind)]
    pub fn records_project(&self, project_id: String) -> bool {
        self.journal.records_project(&project_id)
    }

    /// Hashes `bytes` with the install key and queues them as a blob, which the writer puts
    /// on disk before the first entry that references it. Returns the `Body` JSON
    /// (`{"blob": {hash, bytes, mediaType}}`) to put in a content field, or `null` when
    /// training is off, `bytes` is over 4 MiB, or `media_type` is not in `MEDIA_TYPES`.
    #[napi(catch_unwind)]
    pub fn attach_blob(&self, bytes: Buffer, media_type: String) -> Result<Option<String>> {
        let Some(media) = MEDIA_TYPES.get(&media_type) else {
            return Ok(None);
        };
        match self.journal.attach_blob(Arc::from(bytes.as_ref()), media) {
            None => Ok(None),
            Some(r) => encode(&Body::Blob(r)).map(Some),
        }
    }
}
