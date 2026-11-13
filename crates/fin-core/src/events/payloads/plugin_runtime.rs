//! The plugin runtime's kinds (sub-project 8 spec §11; sp8b plan Task 15, Decision 22):
//! `plugin.stats`, `plugin.exit` and `plugin.command`, emitted by `finisherd` through
//! `fin-node` (the raw plugin id is hashed there, so the journal's key never reaches
//! TypeScript). No plugin process writes the journal.
//!
//! None holds free text: the plugin is a [`PluginRef`], a command the journal's keyed hash
//! of `<plugin id>/<command>`, method families come from `PLUGIN_METHOD_FAMILIES`, error codes
//! are [`Code`]s, sizes are log₂ buckets. The command's arguments are training content
//! (`toolArgs`), dropped unless training is on.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::jobs::{ExitReason, ProcessRole, WorkerExit};
use super::plugin::PluginRef;
use super::{Body, Counts};
use crate::CoreError;
use crate::events::approx::approx_struct;
use crate::events::class::{Code, Content, Hashed, IdRef, Name};
use crate::events::kinds::{AnyPayload, decode_payload, require_kind};
use crate::events::{names, names_jobs};

/// `plugin.stats`: one plugin's host traffic over one window (a minute), sent when the
/// plugin did anything in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginStats {
    // The plugin (a bundled name or the journal's keyed hash).
    pub plugin: PluginRef,
    /// The window's length in seconds.
    pub window_s: u32,
    /// Per method family (`PLUGIN_METHOD_FAMILIES`), each at most once.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub families: Vec<PluginMethodStats>,
    /// Its jobs that ended `PLUGIN_TIMEOUT`.
    pub timeouts: u32,
    /// Its jobs that ended `PLUGIN_CRASHED`.
    pub crashes: u32,
    /// `log` calls dropped by the rate limit.
    pub log_dropped: u32,
    /// stderr lines dropped by the byte limit.
    pub stderr_dropped: u32,
    // Its brokered requests (absent when it made none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PluginEgressStats")]
    pub egress: Option<PluginEgressStats>,
}
approx_struct!(PluginStats {
    plugin,
    window_s,
    families,
    timeouts,
    crashes,
    log_dropped,
    stderr_dropped,
    egress
});

/// One method family's calls in a `plugin.stats` window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginMethodStats {
    // `PLUGIN_METHOD_FAMILIES`.
    #[serde(deserialize_with = "names::plugin_method_family")]
    pub family: Name,
    pub calls: u32,
    /// Error code → count.
    #[serde(default, skip_serializing_if = "Counts::is_empty")]
    pub errors: Counts<Code>,
    /// Calls refused for reach (`PROJECT_NOT_FOUND`, `BRANCH_NOT_ALLOWED`, a library).
    pub reach_denials: u32,
    pub p50_micros: u64,
    pub p95_micros: u64,
    /// Params bytes over the window, as a log₂ bucket.
    pub bytes_in_log2: u8,
    /// Reply bytes over the window, as a log₂ bucket.
    pub bytes_out_log2: u8,
}
approx_struct!(PluginMethodStats {
    family,
    calls,
    errors,
    reach_denials,
    p50_micros,
    p95_micros,
    bytes_in_log2,
    bytes_out_log2
});

/// The egress broker's counts for one plugin in a `plugin.stats` window (Task 8's counters,
/// differenced per window). Refusals by reason; no origin, URL or body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginEgressStats {
    pub requests: u32,
    pub completed: u32,
    /// `EGRESS_DENIED`: an origin it was not granted.
    pub denied: u32,
    pub not_https: u32,
    pub private_address: u32,
    pub secret_missing: u32,
    pub rate_limited: u32,
    pub too_large: u32,
    pub timeouts: u32,
    pub failed: u32,
    pub redirects_followed: u32,
    pub redirects_returned: u32,
    pub bytes_in_log2: u8,
    pub bytes_out_log2: u8,
}
approx_struct!(PluginEgressStats {
    requests,
    completed,
    denied,
    not_https,
    private_address,
    secret_missing,
    rate_limited,
    too_large,
    timeouts,
    failed,
    redirects_followed,
    redirects_returned,
    bytes_in_log2,
    bytes_out_log2
});

/// Why a plugin process ended (spec §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginExitReason {
    Ok,
    Crash,
    Timeout,
    Killed,
    Oom,
    Protocol,
    Sandbox,
    Idle,
}

impl PluginExitReason {
    /// The plugin reason of a process exit: `done` and `recycled` are `ok`, `idleExpired`
    /// `idle`, `hung` and `stalled` `timeout`, `cancelled`, `killed` and `abandoned` `killed`,
    /// `crashed` and `spawnFailed` `crash`. `sandbox` is 7C's (no exit reason maps to it yet).
    pub fn of(r: ExitReason) -> PluginExitReason {
        match r {
            ExitReason::Done | ExitReason::Recycled => PluginExitReason::Ok,
            ExitReason::IdleExpired => PluginExitReason::Idle,
            ExitReason::Hung | ExitReason::Stalled => PluginExitReason::Timeout,
            ExitReason::Cancelled | ExitReason::Killed | ExitReason::Abandoned => {
                PluginExitReason::Killed
            }
            ExitReason::Crashed | ExitReason::SpawnFailed => PluginExitReason::Crash,
            ExitReason::Oom => PluginExitReason::Oom,
            ExitReason::ProtocolError => PluginExitReason::Protocol,
        }
    }
}

/// `plugin.exit`: a plugin process ended (built from the same exit as its `worker.exit`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginExit {
    // The plugin (a bundled name or the journal's keyed hash).
    pub plugin: PluginRef,
    pub reason: PluginExitReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub exit_code: Option<i64>,
    // The terminating signal (`SIGNALS`; Unix only).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "names_jobs::signal_opt"
    )]
    #[schemars(with = "Name")]
    pub signal: Option<Name>,
    pub jobs_served: u32,
    pub uptime_s: u64,
}
approx_struct!(PluginExit {
    plugin,
    reason,
    exit_code,
    signal,
    jobs_served,
    uptime_s
});

impl PluginExit {
    /// The `plugin.exit` of a `worker.exit` of a plugin's own process (`role: worker`); a
    /// helper (a codec of a plugin job) is not the plugin's process.
    pub fn of(w: &WorkerExit) -> Option<PluginExit> {
        if w.role != ProcessRole::Worker {
            return None;
        }
        Some(PluginExit {
            plugin: w.plugin.clone()?,
            reason: PluginExitReason::of(w.reason),
            exit_code: w.exit_code,
            signal: w.signal,
            jobs_served: w.jobs_served,
            uptime_s: w.lifetime_s,
        })
    }
}

/// How a plugin command ended (spec §11; `failed`: the process crashed, broke the protocol or
/// the host failed, a deviation: the spec's list has no outcome for it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum PluginCommandOutcome {
    /// Its transaction was committed as is (or, with `dryRun`, validated).
    Accepted,
    /// Committed after a rebase.
    Rebased,
    Conflict,
    /// Refused by a check (permission, reach, a disabled plugin).
    Refused,
    /// Its arguments or its returned ops were invalid (`PLUGIN_COMMAND_ARGS`,
    /// `PLUGIN_TX_INVALID`).
    Invalid,
    Timeout,
    /// It returned data, not a transaction.
    Data,
    Failed,
}

/// Why `args` is absent although training is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ArgsOmitted {
    // Over `MAX_COMMAND_ARGS_BYTES` as JSON.
    TooLarge,
}

/// The largest command arguments kept inline (as JSON bytes).
pub const MAX_COMMAND_ARGS_BYTES: usize = 16 * 1024;

/// `plugin.command`: one run of a plugin command (`plugin.command.run`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PluginCommand {
    // The plugin (a bundled name or the journal's keyed hash).
    pub plugin: PluginRef,
    // The keyed hash of `<plugin id>/<command>`.
    pub command: Hashed,
    pub outcome: PluginCommandOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Code")]
    pub code: Option<Code>,
    /// Op kind → count of the ops it returned (`OP_KINDS`).
    #[serde(
        default,
        skip_serializing_if = "Counts::is_empty",
        deserialize_with = "names::op_counts"
    )]
    pub ops: Counts<Name>,
    // The committed transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "IdRef")]
    pub tx_id: Option<IdRef>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dry_run: bool,
    pub micros: u64,
    /// Training only: the arguments as validated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Content<Body>", extend("x-fin-content" = "toolArgs"))]
    pub args: Option<Content<Body>>,
    // Why `args` is absent although training is on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ArgsOmitted")]
    pub args_omitted: Option<ArgsOmitted>,
}
approx_struct!(PluginCommand {
    plugin,
    command,
    outcome,
    code,
    ops,
    tx_id,
    dry_run,
    micros,
    args,
    args_omitted
});

/// The kinds [`plugin_payloads`] builds, with their payload versions.
const KINDS: [(&str, u16); 4] = [
    ("worker.exit", 2),
    ("plugin.exit", 1),
    ("plugin.stats", 1),
    ("plugin.command", 1),
];

/// What [`plugin_payloads`] keys with: the journal's keyed hash and `IdRef` (each `None`
/// while the journal is off), and whether training is on.
pub struct PluginKeys<'a> {
    pub hash: &'a dyn Fn(&str) -> Option<Hashed>,
    pub id_ref: &'a dyn Fn(&str) -> Option<IdRef>,
    pub training: bool,
}

fn invalid(kind: &str, m: impl Into<String>) -> CoreError {
    CoreError::new("EVENT_INVALID", m).with_details(json!({ "kind": kind }))
}

/// The payloads `fin-node`'s `recordPlugin` records for a raw plugin id (`@publisher/name`):
/// `kind`'s, and for a `worker.exit` its `plugin.exit`. Empty when the keys are unavailable
/// (the journal is off). `payload` is the kind's payload without `plugin` (a `plugin` sent is
/// replaced). For `plugin.command`, `command` is the raw name (keyed with the plugin id),
/// `txId` a raw id and `args` raw JSON, kept as `toolArgs` content only with training on and
/// within [`MAX_COMMAND_ARGS_BYTES`] (else `argsOmitted: tooLarge`).
pub fn plugin_payloads(
    kind: &str,
    kv: u16,
    plugin_id: &str,
    payload: Value,
    keys: &PluginKeys,
) -> Result<Vec<AnyPayload>, CoreError> {
    if !KINDS.contains(&(kind, kv)) {
        return Err(invalid(
            kind,
            format!(
                "recordPlugin takes worker.exit v2, plugin.exit, plugin.stats and plugin.command v1, not {kind} v{kv}"
            ),
        ));
    }
    let spec = require_kind(kind, kv)?;
    let Value::Object(mut obj) = payload else {
        return Err(invalid(kind, "the payload is not an object"));
    };
    let Some(plugin) = PluginRef::of(plugin_id, keys.hash) else {
        return Ok(Vec::new());
    };
    obj.insert(
        "plugin".into(),
        serde_json::to_value(&plugin).map_err(|e| invalid(kind, e.to_string()))?,
    );
    if kind == "plugin.command" && !command_fields(&mut obj, plugin_id, keys)? {
        return Ok(Vec::new());
    }
    let p = decode_payload(spec, &Value::Object(obj))?;
    let exit = match &p {
        AnyPayload::WorkerExit(w) => PluginExit::of(w).map(AnyPayload::from),
        _ => None,
    };
    Ok(std::iter::once(p).chain(exit).collect())
}

/// `plugin.command`'s keyed fields; false when the keys are unavailable.
fn command_fields(
    obj: &mut Map<String, Value>,
    plugin_id: &str,
    keys: &PluginKeys,
) -> Result<bool, CoreError> {
    let Some(Value::String(command)) = obj.remove("command") else {
        return Err(invalid("plugin.command", "command is the command's name"));
    };
    let Some(h) = (keys.hash)(&format!("{plugin_id}/{command}")) else {
        return Ok(false);
    };
    obj.insert("command".into(), json!(h.as_str()));
    match obj.remove("txId") {
        None | Some(Value::Null) => {}
        Some(Value::String(tx)) => match (keys.id_ref)(&tx) {
            Some(r) => {
                obj.insert("txId".into(), json!(r.as_str()));
            }
            None => return Ok(false),
        },
        Some(_) => return Err(invalid("plugin.command", "txId is a string")),
    }
    // The sender's `argsOmitted: tooLarge` (it left oversized arguments out) is kept with
    // training only; any other value is dropped.
    let omitted = obj.remove("argsOmitted");
    if keys.training && omitted == Some(json!("tooLarge")) {
        obj.insert("argsOmitted".into(), json!("tooLarge"));
    }
    match obj.remove("args") {
        Some(args) if keys.training && !args.is_null() => {
            let fits = serde_json::to_vec(&args).is_ok_and(|b| b.len() <= MAX_COMMAND_ARGS_BYTES);
            if fits {
                obj.insert("args".into(), json!({ "inline": args }));
            } else {
                obj.insert("argsOmitted".into(), json!("tooLarge"));
            }
        }
        _ => {}
    }
    Ok(true)
}
