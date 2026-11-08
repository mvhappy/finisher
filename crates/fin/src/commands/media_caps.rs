//! `fin media caps` (plan 6A Task 13): what the server's media worker can do on this machine,
//! from `media.capabilities`: `{codecs: {video: [{codec, decode, encode, backends}]}, gpu:
//! {backend, deviceType, name, vendor, device, software} | null, worker: {version,
//! renderVersion}}`. `decode` and `encode` are `hardware`, `software` or `none`; `gpu: null`
//! means no adapter (not even a software one) can be opened, so nothing renders. The server asks
//! a warm worker once and keeps the answer for its run.

use clap::Subcommand;
use fin_core::CoreError;
use serde_json::{Value, json};

use crate::backend::Backend;
use crate::output::unavailable;

#[derive(Debug, Clone, Subcommand)]
pub enum MediaCmd {
    /// Codecs, GPU and worker versions of this machine's media worker.
    Caps,
}

pub fn run(backend: &dyn Backend, cmd: &MediaCmd) -> Result<Value, CoreError> {
    match cmd {
        MediaCmd::Caps => {
            if backend.dry_run_only() {
                return Err(unavailable(
                    json!({ "mode": "offline", "operation": "media.capabilities" }),
                )
                .with_fix("fin media caps asks the server: start finisherd and run it again"));
            }
            backend.server_call("media.capabilities", json!({}))
        }
    }
}
