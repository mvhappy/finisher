//! The params registry type (Decision 27): each kind's params are a Rust type, registered by
//! its family's crate as a [`KindParams`] and validated through fin-node's
//! `validateJobParams`. [`validate_as`] and [`schema_as`] make the two functions of a type.

use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// A kind's params at one version.
#[derive(Clone, Copy)]
pub struct KindParams {
    pub kind: &'static str,
    pub version: u32,
    /// The params as the kind will read them (defaults filled), or the errors, each naming
    /// the field (`params.ms: …`) when it can.
    pub validate: fn(&Value) -> Result<Value, Vec<String>>,
    /// The params' JSON Schema.
    pub schema: fn() -> Schema,
}

impl std::fmt::Debug for KindParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KindParams({} v{})", self.kind, self.version)
    }
}

/// `validate` for a params type `T`: decode, then encode again (so defaults are filled). A
/// failure names the top-level field it comes from where serde's message does not.
pub fn validate_as<T: DeserializeOwned + Serialize>(params: &Value) -> Result<Value, Vec<String>> {
    match T::deserialize(params) {
        Ok(t) => serde_json::to_value(t).map_err(|e| vec![e.to_string()]),
        Err(e) => Err(vec![name_field::<T>(params, &e.to_string())]),
    }
}

/// Finds the top-level field behind a decode error: the params are decoded again from a text
/// with one field per line, and the error's line names the field.
fn name_field<T: DeserializeOwned>(params: &Value, message: &str) -> String {
    let Value::Object(map) = params else {
        return format!("params: {message}");
    };
    let keys: Vec<&String> = map.keys().collect();
    let mut text = String::from("{\n");
    for (i, (k, v)) in map.iter().enumerate() {
        text.push_str(&Value::String(k.clone()).to_string());
        text.push(':');
        text.push_str(&v.to_string());
        text.push_str(if i + 1 < keys.len() { ",\n" } else { "\n" });
    }
    text.push('}');
    match serde_json::from_str::<T>(&text) {
        Err(e) if e.line() >= 2 && e.line() - 2 < keys.len() => {
            format!("params.{}: {message}", keys[e.line() - 2])
        }
        _ => format!("params: {message}"),
    }
}

/// `schema` for a params type `T` (draft 2020-12).
pub fn schema_as<T: JsonSchema>() -> Schema {
    SchemaGenerator::new(SchemaSettings::draft2020_12()).into_root_schema_for::<T>()
}

/// The entry for `kind` at `version` in `tables`.
pub fn find<'t>(tables: &[&'t [KindParams]], kind: &str, version: u32) -> Option<&'t KindParams> {
    tables
        .iter()
        .flat_map(|t| t.iter())
        .find(|p| p.kind == kind && p.version == version)
}
