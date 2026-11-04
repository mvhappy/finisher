//! One JSON Schema document for the stable contract (spec §4 rule 5).

use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::error::CoreError;
use crate::model::Project;
use crate::tx::{Applied, CommittedTx, Diff, Op, Tx};

/// The document: `$defs` holds `Project`, `Tx`, `Op`, `Diff`, `CoreError`, `CommittedTx`
/// (plus everything they reference). Output is deterministic (sorted keys, no timestamps).
pub fn json_schema() -> Value {
    let mut generator = SchemaGenerator::new(SchemaSettings::draft2020_12());
    generator.subschema_for::<Project>();
    generator.subschema_for::<Tx>();
    generator.subschema_for::<Op>();
    generator.subschema_for::<Diff>();
    generator.subschema_for::<CoreError>();
    generator.subschema_for::<CommittedTx>();
    generator.subschema_for::<Applied>();
    let defs: Map<String, Value> = generator.take_definitions(true);
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "FinCore",
        "$defs": defs,
    })
}
