//! The plugin runtime's journal entries (sp8b plan Task 15, Decision 22): `finisherd` hands
//! the raw plugin id to [`JournalHandle::record_plugin`], which names the plugin by its
//! `PluginRef` here (a bundled name, else the journal's keyed hash; built by fin-core's
//! `plugin_payloads`, tested in `fin-core/tests/events_plugin.rs`), so the key never reaches
//! TypeScript and the raw id never reaches the journal.
//!
//! - `worker.exit` `kv` 2 with `plugin`, and `plugin.exit` built from the same exit;
//! - `plugin.stats` `kv` 1;
//! - `plugin.command` `kv` 1: `command` (the raw name) becomes the keyed hash of
//!   `<plugin id>/<command>`, `txId` an `IdRef`, and `args` (raw JSON) is kept as training
//!   content only while training is on and its JSON fits `MAX_COMMAND_ARGS_BYTES`.
//!
//! Off, nothing is built or hashed.

use fin_core::events::Source;
use fin_core::events::payloads::{PluginKeys, plugin_payloads};
use napi_derive::napi;
use serde_json::Value;

use super::{JournalHandle, event_invalid, parse};
use crate::reject;

#[napi]
impl JournalHandle {
    /// Records a plugin runtime entry whose plugin is named by its raw id (`@publisher/name`,
    /// no version): `worker.exit` v2 (and its `plugin.exit`), `plugin.exit`, `plugin.stats`
    /// or `plugin.command` v1, `payload_json` without `plugin`. The id becomes a bundled name
    /// or the journal's keyed hash here. Source `finisherd`. Throws `EVENT_INVALID`,
    /// `EVENT_KIND_UNKNOWN`; off, it builds nothing.
    #[napi(catch_unwind)]
    pub fn record_plugin(
        &self,
        kind: String,
        kv: u32,
        plugin_id: String,
        payload_json: String,
    ) -> napi::Result<()> {
        let Some(purposes) = self.journal.gate() else {
            return Ok(());
        };
        let kv = u16::try_from(kv).map_err(|_| event_invalid("kv is out of range"))?;
        let payload: Value = parse("plugin payload", &payload_json)?;
        let j = &self.journal;
        let keys = PluginKeys {
            hash: &|s| j.hash(s),
            id_ref: &|s| j.id_ref(s),
            training: purposes.has_training(),
        };
        for p in plugin_payloads(&kind, kv, &plugin_id, payload, &keys).map_err(reject)? {
            j.record(Source::Finisherd, None, p);
        }
        Ok(())
    }
}
