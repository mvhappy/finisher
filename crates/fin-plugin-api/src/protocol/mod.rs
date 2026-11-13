//! The plugin protocol's types (spec §3.4 as 8B builds it, plan Decision 2): a plugin process
//! is a worker of 5C's `plugin` family and speaks the `fin-jobs` envelope and nothing else.
//! These are the typed payloads inside that envelope:
//! - [`PluginContext`] ([`context`]): the job's `start.extra` (host API, plugin, grants,
//!   directories, locale, limits), in place of spec §3.4's `initialize`;
//! - [`CommandRun`] and [`CommandOut`] ([`command`]): the params and result of a
//!   `plugin.command` v1 job (spec §3.6's `command.run`);
//! - [`hook`]: the hook events, [`HookRequest`] and [`HookReply`] (7F), shared by command hooks
//!   and a `plugin.hook` v1 job's params ([`HookRun`]) and result;
//! - [`calls`]: the plugin → host calls (`call {id, method, params}` → `reply`) of host API 1.1;
//! - [`JobRights`] ([`rights`]): a plugin job's invoker ∩ grants (Decision 8);
//! - the client RPCs of commands ([`rpc`]): `plugin.commands`, `plugin.command.run`;
//! - [`PluginError`] ([`errors`]): the `{code, message, details, fix?}` a failed call, job or
//!   command answers.
//!
//! Host → plugin payloads ([`PluginContext`] and its parts, [`CommandRun`], the replies in
//! [`calls`]) ignore unknown fields, so a newer host can add one; plugin → host payloads (call
//! params, [`CommandOut`], `log`) refuse them.
//!
//! Every line of the envelope is at most [`MAX_MESSAGE_BYTES`] (5C's cap; **deviation from
//! spec §3.3**'s 16 MiB), so larger payloads are paged in [`PAGE_BYTES`] pages (Decision 5).

pub mod calls;
pub mod command;
pub mod context;
pub mod errors;
pub mod hook;
pub mod rights;
pub mod rpc;

pub use calls::HostMethod;
pub use command::{CommandOut, CommandRun};
pub use context::{CallLimits, PluginContext, PluginDirs, PluginRefFull};
pub use errors::PluginError;
pub use hook::{
    HookClass, HookEvent, HookOut, HookProject, HookReply, HookRequest, HookRun, HookSession,
};
pub use rights::{JobRights, PluginScope, Reach, ReachAll, RightsBranch, RightsPrincipal};
pub use rpc::{
    CommandOutcome, PluginCommandInfo, PluginCommandList, PluginCommandRunRequest,
    PluginCommandRunResult,
};

/// The job kind of a command run (Decision 7).
pub const COMMAND_KIND: &str = "plugin.command";
/// The version of [`COMMAND_KIND`]'s params and result.
pub const COMMAND_KIND_VERSION: u32 = 1;
/// A command's timeout when its manifest names none, in seconds (spec §3.3).
pub const DEFAULT_COMMAND_TIMEOUT_S: u16 = 10;
/// The longest timeout a manifest may name for a command, in seconds.
pub const MAX_COMMAND_TIMEOUT_S: u16 = 60;
/// The largest envelope line, in bytes (5C's cap).
pub const MAX_MESSAGE_BYTES: u32 = 1 << 20;
/// The largest page of a paged reply (`project.snapshot`, `project.view`, `net.read`), in
/// bytes before base64: 756 KiB (774,144 bytes), whose 1,032,192 base64 characters leave room
/// for the reply's JSON in a [`MAX_MESSAGE_BYTES`] line.
pub const PAGE_BYTES: u32 = 756 * 1024;
