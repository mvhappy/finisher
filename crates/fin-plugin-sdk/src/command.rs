//! What a plugin implements ([`Plugin`]), the call it gets ([`CommandCall`]) and the mapping
//! between the protocol's [`PluginError`] and the envelope's `JobError`.

use fin_jobs::JobError;
use fin_plugin_api::protocol::errors::codes;
use fin_plugin_api::protocol::{CommandOut, CommandRun, PluginError};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::host::Host;

/// A plugin: one value per process, made with `Default` before `hello`, serving every command
/// job the process gets (a warm process serves many, one at a time).
pub trait Plugin: Default {
    /// Runs the manifest command `call.name`. A name the plugin does not implement answers
    /// [`CommandCall::unknown`].
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError>;
}

/// A command run: the host has checked `args` against the command's schema and pinned `rev` to
/// the head of `branch` ([`CommandRun`]).
#[derive(Debug, Clone, PartialEq)]
pub struct CommandCall {
    /// The command's name in the manifest (not the `plugin:<id>/` registry name).
    pub name: String,
    pub args: Value,
    pub project: String,
    pub branch: String,
    pub rev: u64,
}

impl From<CommandRun> for CommandCall {
    fn from(r: CommandRun) -> CommandCall {
        CommandCall {
            name: r.command,
            args: r.args,
            project: r.project,
            branch: r.branch,
            rev: r.rev,
        }
    }
}

impl CommandCall {
    /// The arguments as `T`. They passed the manifest schema already, so a failure here is the
    /// plugin's own type disagreeing with its schema: `PLUGIN_PROTOCOL {reason: args, message}`.
    pub fn args<T: DeserializeOwned>(&self) -> Result<T, PluginError> {
        serde_json::from_value(self.args.clone()).map_err(|e| {
            let message = format!("the arguments of {:?} do not decode: {e}", self.name);
            protocol("args", message.clone()).with("message", message)
        })
    }

    /// The answer to a command this plugin does not implement (its manifest names one its code
    /// lacks, a plugin bug the caller can see): `PLUGIN_PROTOCOL {reason: unknownCommand,
    /// command}`.
    pub fn unknown(&self) -> PluginError {
        protocol(
            "unknownCommand",
            format!("the plugin does not implement the command {:?}", self.name),
        )
        .with("command", self.name.clone())
    }
}

/// `JOB_CANCELLED`: what a command returns once [`Host::cancelled`] is set (host calls return it
/// by themselves after a cancel).
pub fn cancelled() -> PluginError {
    PluginError::new("JOB_CANCELLED", "the command was cancelled")
}

/// `PLUGIN_PROTOCOL {reason, …}`: a job or reply this SDK cannot use.
pub(crate) fn protocol(reason: &str, message: impl Into<String>) -> PluginError {
    PluginError::new(codes::PLUGIN_PROTOCOL, message).with("reason", reason)
}

/// A host call's `JobError` as a [`PluginError`]: the same `code` and `message`; `details` an
/// object (a non-object is kept under `details.value`) with a string `details.fix` lifted into
/// `fix`.
pub fn error_from_job(e: JobError) -> PluginError {
    let mut details = match e.details {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        other => Map::from_iter([("value".to_owned(), other)]),
    };
    let fix = match details.remove("fix") {
        Some(Value::String(f)) => Some(f),
        Some(other) => {
            details.insert("fix".into(), other);
            None
        }
        None => None,
    };
    PluginError {
        code: e.code,
        message: e.message,
        details,
        fix,
    }
}

/// A command's [`PluginError`] as the job's `JobError` (`retry: never`: a command is never
/// retried, Decision 7), its `fix` as `details.fix`.
pub fn error_to_job(e: PluginError) -> JobError {
    let mut details = e.details;
    if let Some(fix) = e.fix {
        details.insert("fix".into(), Value::String(fix));
    }
    JobError::new(e.code, e.message).with_details(Value::Object(details))
}
