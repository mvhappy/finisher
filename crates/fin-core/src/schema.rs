//! One JSON Schema document for the stable contract (spec §4 rule 5).

use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

use crate::error::CoreError;
use crate::eval::{AudioPlan, FramePlan};
use crate::model::Project;
use crate::tx::{Applied, CommittedTx, Diff, Op, Tx};

/// The document: `$defs` holds `Project`, `Tx`, `Op`, `Diff`, `CoreError`, `CommittedTx`
/// (plus everything they reference). Output is deterministic (sorted keys, no timestamps).
pub fn json_schema() -> Value {
    json_schema_with(|_| {})
}

/// [`json_schema`] plus the types `extra` adds to the generator (`subschema_for`), in the same
/// `$defs`: the crates above `fin-core` (store, probe) add the envelope types they send to
/// TypeScript, sharing these definitions.
pub fn json_schema_with(extra: impl FnOnce(&mut SchemaGenerator)) -> Value {
    let mut generator = SchemaGenerator::new(SchemaSettings::draft2020_12());
    generator.subschema_for::<Project>();
    generator.subschema_for::<Tx>();
    generator.subschema_for::<Op>();
    generator.subschema_for::<Diff>();
    generator.subschema_for::<CoreError>();
    generator.subschema_for::<CommittedTx>();
    generator.subschema_for::<Applied>();
    generator.subschema_for::<crate::params::ParamSpec>();
    generator.subschema_for::<FramePlan>();
    generator.subschema_for::<AudioPlan>();
    extra(&mut generator);
    let mut defs: Map<String, Value> = generator.take_definitions(true);
    for v in defs.values_mut() {
        ref_siblings_to_all_of(v);
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "FinCore",
        "$defs": defs,
    })
}

/// Rewrites every `{"$ref": r, <keywords>}` with sibling keywords other than `description`
/// and `default` into `{"allOf": [{"$ref": r}, {<keywords>}]}` (keeping those two). schemars writes an
/// internally tagged newtype variant (`Op::ItemInsert(..)`) as its payload's `$ref` beside the
/// tag's `properties`; json-schema-to-typescript merges such siblings into the referenced
/// schema shallowly, so the tag's `properties` would replace the payload's.
fn ref_siblings_to_all_of(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for x in map.values_mut() {
                ref_siblings_to_all_of(x);
            }
            let has_ref = map.contains_key("$ref");
            let siblings = map
                .keys()
                .any(|k| !matches!(k.as_str(), "$ref" | "description" | "default"));
            if has_ref && siblings {
                let reference = map.remove("$ref").unwrap_or_default();
                let kept: Vec<(String, Value)> = ["description", "default"]
                    .into_iter()
                    .filter_map(|k| map.remove(k).map(|v| (k.to_string(), v)))
                    .collect();
                let rest = std::mem::take(map);
                map.insert(
                    "allOf".into(),
                    json!([{ "$ref": reference }, Value::Object(rest)]),
                );
                map.extend(kept);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(ref_siblings_to_all_of),
        _ => {}
    }
}
