//! The plugin host's call helpers (sp8b Task 7): each host call's params decode with the
//! `fin-plugin-api` type the generated schema comes from, and `project.view` renders the same
//! screenplay as `fin view` (fin-core's `view`), offline (no font file states).

use fin_core::{CoreError, Project};
use fin_plugin_api::protocol::HostMethod;
use fin_plugin_api::protocol::calls::{decode_params, reserved_method};
use napi::bindgen_prelude::*;
use napi_derive::napi;
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
