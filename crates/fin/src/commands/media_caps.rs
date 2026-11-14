//! `fin media caps` (plan 6A Task 13): what the server's media worker can do on this machine,
//! from `media.capabilities`: `{codecs: {video: [{codec, decode, encode, backends}]}, gpu:
//! {backend, deviceType, name, vendor, device, software} | null, worker: {version,
//! renderVersion}}`. `decode` and `encode` are `hardware`, `software` or `none`; `gpu: null`
//! means no adapter (not even a software one) can be opened, so nothing renders. The server asks
//! a warm worker once and keeps the answer for its run.

use clap::Subcommand;
use fin_core::CoreError;
use serde_json::{Value, json};

use super::Globals;
use super::media_probe::{self, ProbeArgs};
use super::media_proxy::{self, ProxyArgs, StatusArgs};
use super::media_thumbs::{self, ThumbsArgs};
use crate::backend::Backend;
use crate::output::unavailable;

#[derive(Debug, Clone, Subcommand)]
pub enum MediaCmd {
    /// Codecs, GPU and worker versions of this machine's media worker.
    Caps,
    /// Decode an asset's first frame and read its in-band HDR metadata (6B-1).
    Probe(ProbeArgs),
    /// Make (or find) a 540 or 1080 proxy of a video asset (6B-1).
    Proxy(ProxyArgs),
    /// The state of an asset's proxies, per tier (6B-1).
    Status(StatusArgs),
    /// Thumbnails at source times, or the filmstrip, of a video asset (6B-1).
    Thumbs(ThumbsArgs),
}

pub fn run(backend: &dyn Backend, globals: &Globals, cmd: &MediaCmd) -> Result<Value, CoreError> {
    match cmd {
        MediaCmd::Probe(a) => media_probe::run(backend, globals, a),
        MediaCmd::Proxy(a) => media_proxy::proxy(backend, globals, a),
        MediaCmd::Status(a) => media_proxy::status(backend, globals, a),
        MediaCmd::Thumbs(a) => media_thumbs::run(backend, globals, a),
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
