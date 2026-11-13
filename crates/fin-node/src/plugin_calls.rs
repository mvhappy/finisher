//! The plugin host's call helpers (sp8b Task 7): each host call's params decode with the
//! `fin-plugin-api` type the generated schema comes from, and `project.view` renders the same
//! screenplay as `fin view` (fin-core's `view`), offline (no font file states). Task 9 adds a
//! command's argument check and the check of what a command returned.

// napi registers the export; a `cargo test` build has no caller.
#![cfg_attr(test, allow(dead_code))]

use fin_core::{CoreError, MemIds, Project, SCHEMA_VERSION, Tx, Via, apply_tx};
use fin_plugin_api::protocol::calls::{decode_params, reserved_method};
use fin_plugin_api::protocol::{CommandOut, HostMethod};
use fin_plugin_api::schema_subset::{CompiledArgs, SchemaSubset};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{encode, guarded, reject};

/// `decodePluginCall`: `{ok: true, params}` (canonical), `{ok: false, reserved: <batch>}` for a
/// later batch's method, `{ok: false, unknown: true}` for any other name, or
/// `{ok: false, error}` when the params do not decode (the host answers `PLUGIN_PROTOCOL`).
#[napi(catch_unwind)]
pub fn decode_plugin_call(method: String, params_json: String) -> Result<String> {
    guarded(
        || {
            let out = match HostMethod::from_name(&method) {
                None => match reserved_method(&method) {
                    Some(batch) => json!({ "ok": false, "reserved": batch }),
                    None => json!({ "ok": false, "unknown": true }),
                },
                Some(m) => match serde_json::from_str::<Value>(&params_json) {
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                    Ok(params) => match decode_params(m, &params) {
                        Ok(params) => json!({ "ok": true, "params": params }),
                        Err(error) => json!({ "ok": false, "error": error }),
                    },
                },
            };
            Ok(out.to_string())
        },
        || {},
    )
}

/// `pluginProjectView`: `{lines, elided, tokens}` of `projectJson` (a snapshot's project).
#[napi(catch_unwind)]
pub fn plugin_project_view(
    project_json: String,
    range: Option<String>,
    budget: Option<f64>,
) -> Result<String> {
    guarded(
        || {
            let project: Project = serde_json::from_str(&project_json).map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("the snapshot's project does not decode: {e}"),
                ))
            })?;
            let budget = match budget {
                None => None,
                Some(b) if b.is_finite() && b >= 0.0 && b.fract() == 0.0 => Some(b as u64),
                Some(_) => {
                    return Err(reject(
                        CoreError::new("INVALID_REQUEST", "budget: a whole number of tokens")
                            .with_details(json!({ "field": "budget" })),
                    ));
                }
            };
            let v =
                fin_core::view::screenplay(&project, range.as_deref(), budget).map_err(reject)?;
            encode(&v)
        },
        || {},
    )
}

/// A command's argument schema compiled once per installed version (sp8b Task 9): the host
/// checks `args` with it before a `plugin.command` job is created.
#[napi]
pub struct PluginCommandArgs {
    compiled: CompiledArgs,
}

#[napi]
impl PluginCommandArgs {
    /// Compiles `schema_json` (a manifest's `args`); `PLUGIN_MANIFEST_INVALID {at, problem}`
    /// when it is outside the subset (the manifest validator refused that at install).
    #[napi(constructor)]
    pub fn new(schema_json: String) -> Result<PluginCommandArgs> {
        let value: Value = serde_json::from_str(&schema_json).map_err(|e| {
            reject(CoreError::new(
                "PLUGIN_MANIFEST_INVALID",
                format!("a command's args schema is not JSON: {e}"),
            ))
        })?;
        let compiled = CompiledArgs::new(&SchemaSubset::new(value)).map_err(|e| {
            reject(
                CoreError::new("PLUGIN_MANIFEST_INVALID", e.message.clone())
                    .with_details(json!({ "at": e.at, "problem": e.problem })),
            )
        })?;
        Ok(PluginCommandArgs { compiled })
    }

    /// `{ok: true}` or `{ok: false, problems: [ArgProblem]}` (sorted by path).
    #[napi(catch_unwind)]
    pub fn validate(&self, args_json: String) -> Result<String> {
        let args: Value = serde_json::from_str(&args_json).map_err(|e| {
            reject(CoreError::new(
                "INVALID_REQUEST",
                format!("args are not JSON: {e}"),
            ))
        })?;
        let out = match self.compiled.validate(&args) {
            Ok(()) => json!({ "ok": true }),
            Err(problems) => json!({ "ok": false, "problems": problems }),
        };
        Ok(out.to_string())
    }
}

/// The fields of the transaction the host builds around a command's ops.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandTxFields {
    tx_id: String,
    author: String,
    branch: String,
    base_rev: u64,
    via: Via,
}

/// `pluginCommandOutcome`: decodes a `plugin.command` job's result (`CommandOut`) and, for a
/// transaction, builds it on `txJson`'s fields (`via` the host's) and applies it to the
/// project the plugin saw (`projectJson`, at `baseRev`): `{ok: true, kind: "data", value}`,
/// `{ok: true, kind: "tx", tx, bindings, diff}`, `{ok: false, protocol: <error>}` when the
/// result does not decode, or `{ok: false, invalid: <CoreError>}` when fin-core refuses it.
#[napi(catch_unwind)]
pub fn plugin_command_outcome(
    project_json: String,
    out_json: String,
    tx_json: String,
) -> Result<String> {
    guarded(
        || {
            let out: CommandOut = match serde_json::from_str(&out_json) {
                Ok(o) => o,
                Err(e) => return Ok(json!({ "ok": false, "protocol": e.to_string() }).to_string()),
            };
            let ops = match out {
                CommandOut::Data { value } => {
                    return Ok(json!({ "ok": true, "kind": "data", "value": value }).to_string());
                }
                CommandOut::Tx { ops } => ops,
            };
            let f: CommandTxFields = serde_json::from_str(&tx_json).map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("the command's tx fields do not decode: {e}"),
                ))
            })?;
            let project: Project = serde_json::from_str(&project_json).map_err(|e| {
                reject(CoreError::new(
                    "INTERNAL",
                    format!("the snapshot's project does not decode: {e}"),
                ))
            })?;
            let tx = Tx {
                tx_id: f.tx_id,
                author: f.author,
                branch: f.branch,
                base_rev: f.base_rev,
                schema_version: SCHEMA_VERSION,
                ops,
                args_hash: None,
                via: Some(Box::new(f.via)),
            };
            let out = match apply_tx(&project, &tx, &mut MemIds::after(&project)) {
                Ok(a) => {
                    json!({ "ok": true, "kind": "tx", "tx": tx, "bindings": a.bindings, "diff": a.diff })
                }
                Err(e) => json!({ "ok": false, "invalid": e }),
            };
            Ok(out.to_string())
        },
        || {},
    )
}
