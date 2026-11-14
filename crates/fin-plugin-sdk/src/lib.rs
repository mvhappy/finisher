//! The Rust SDK for Finisher code plugins (sub-project 8 spec §3, §7.1; plan 8B Task 12).
//!
//! A plugin process is a worker of 5C's `plugin` family: it speaks the `fin-jobs` envelope on
//! stdin and stdout and nothing else (plan 8B Decision 2). This crate implements the plugin side
//! of that envelope over `fin_jobs::worker` (Decision 21): `hello` with `v` 1, the SDK's
//! [`MINOR`] and no `envelope` hash; the job loop; `ready` after each job when started with a
//! trailing `--warm` (a cold process exits 0 after its job); `alive`; cancel; and a host message
//! of an unknown type (a newer host) dropped, never fatal. A job is a `plugin.command` v1 run
//! (params a [`CommandRun`](fin_plugin_api::protocol::CommandRun), result a [`CommandOut`]) or,
//! for a plugin with [`Plugin::HOOKS`], a `plugin.hook` v1 run (params a
//! [`HookRun`](fin_plugin_api::protocol::HookRun), result a [`HookReply`]; 7F Decision 20); its
//! `start.extra` is the [`PluginContext`].
//!
//! Typed host calls (`project.snapshot` with its pages joined, `project.view`, `net.fetch` with
//! its body read through `net.read`, `log`) are [`Host`]'s methods. Plugins propose
//! transactions of fin-core ops; the host validates and commits them under the invoker.
//!
//! ```no_run
//! use fin_plugin_sdk::prelude::*;
//!
//! #[derive(Default)]
//! struct Tools;
//!
//! impl Plugin for Tools {
//!     fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError> {
//!         match call.name.as_str() {
//!             "count-timelines" => {
//!                 let project = host.snapshot()?;
//!                 Ok(CommandOut::Data { value: json!(project.timelines.len()) })
//!             }
//!             _ => Err(call.unknown()),
//!         }
//!     }
//! }
//!
//! fin_plugin_sdk::main!(Tools);
//! ```
//!
//! **Errors.** A failed host call arrives as a `fin-jobs` `JobError` and is handed to the
//! command as a [`PluginError`] with the same `code`, `message` and `details`; a command's
//! `PluginError` goes back as the job's error the same way. The envelope's error has no `fix`,
//! so a `fix` travels as `details.fix` (a string) and is lifted back out on the other side
//! ([`command::error_from_job`], [`command::error_to_job`]).

pub mod command;
#[cfg(feature = "files")]
pub mod files;
pub mod hook;
pub mod host;
mod macros;
pub mod prelude;
pub mod runtime;

pub use command::{CommandCall, Plugin};
pub use fin_plugin_api::protocol::calls::{Header, HttpMethod, HttpRequest, LogLevel as Level};
pub use fin_plugin_api::protocol::errors::codes;
pub use fin_plugin_api::protocol::{
    CommandOut, HookEvent, HookReply, HookRequest, PluginContext, PluginError,
};
pub use hook::HookCall;
pub use host::{Host, HttpResponse, http_get};

/// Re-exported so a plugin builds ops and reads snapshots without its own dependency.
pub use fin_core;
pub use fin_plugin_api;
pub use serde_json;

/// The envelope minor this SDK speaks, sent in `hello.minor` (5C Decision 3: a host older than
/// this answers or drops what it does not know by the minor rule). It is the envelope's own
/// minor: the SDK knows no message the tree's runtime does not.
pub const MINOR: u32 = fin_jobs::MINOR;
