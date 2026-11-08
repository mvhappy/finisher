//! Decoding `JobHost`'s JSON arguments: the shapes are `fin_jobhost::wire`'s (generated into
//! `@finisher/core-types`); an argument that does not decode is `INVALID_REQUEST {field}`.

use fin_core::CoreError;
use fin_jobhost::HostEvent;
use fin_jobhost::KillReason;
use fin_jobhost::wire::{JobHostEvent, JobHostKillReason, WireError};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Decodes `json` as `T`, or `INVALID_REQUEST {field, error}`.
pub(crate) fn decode<T: DeserializeOwned>(field: &str, json: &str) -> Result<T, CoreError> {
    serde_json::from_str(json).map_err(|e| invalid(field, e.to_string()))
}

/// Decodes a value already parsed as `T`, or `INVALID_REQUEST {field, error}`.
pub(crate) fn decode_value<T: DeserializeOwned>(field: &str, v: Value) -> Result<T, CoreError> {
    serde_json::from_value(v).map_err(|e| invalid(field, e.to_string()))
}

pub(crate) fn invalid(field: &str, error: String) -> CoreError {
    CoreError::new("INVALID_REQUEST", format!("{field}: {error}"))
        .with_details(json!({ "field": field, "error": error }))
}

pub(crate) fn wire(e: WireError) -> CoreError {
    invalid(e.field, e.error)
}

pub(crate) fn kill_reason(name: &str) -> Result<KillReason, CoreError> {
    decode_value::<JobHostKillReason>("reason", Value::String(name.into())).map(Into::into)
}

/// A `JobHostEvent` as JSON.
pub(crate) fn event_json(e: HostEvent) -> String {
    serde_json::to_string(&JobHostEvent::from(e)).unwrap_or_else(|e| {
        // Unreachable: every part serializes. Still an event, so the order stays visible.
        json!({ "type": "stderr", "proc": 0, "line": format!("fin-node: an event did not encode: {e}") })
            .to_string()
    })
}
