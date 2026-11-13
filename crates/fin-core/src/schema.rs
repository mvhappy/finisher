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

/// Rewrites every `{"$ref": r, <keywords>}` into `{"allOf": [{"$ref": r}, {<keywords>}]}`,
/// keeping `description` and `default` beside `allOf` (and leaving out the second member when
/// a `description` is all there is; a `$ref` with only a `default` beside, or a `default` and a
/// `description`, is left alone). schemars writes an internally tagged newtype variant
/// (`Op::ItemInsert(..)`) as its payload's `$ref` beside the tag's `properties`;
/// json-schema-to-typescript merges such siblings into the referenced schema shallowly, so the
/// tag's `properties` would replace the payload's. A field's doc beside its `$ref` (a field of a
/// named type) would make it copy the referenced type under a new name (`FontFile1`, or a
/// `Rational` field typed `string`); inside `allOf` the field keeps the type's name.
fn ref_siblings_to_all_of(v: &mut Value) {
    match v {
        Value::Object(map) => {
            for x in map.values_mut() {
                ref_siblings_to_all_of(x);
            }
            let other = map
                .keys()
                .any(|k| !matches!(k.as_str(), "$ref" | "description" | "default"));
            // With a `default` beside `allOf` json-schema-to-typescript intersects the type with
            // the default's (`AssetKind & string`): a lone `$ref` with a default stays as it is.
            let doc_only = map.contains_key("description") && !map.contains_key("default");
            if map.contains_key("$ref") && (other || doc_only) {
                let reference = map.remove("$ref").unwrap_or_default();
                let kept: Vec<(String, Value)> = ["description", "default"]
                    .into_iter()
                    .filter_map(|k| map.remove(k).map(|v| (k.to_string(), v)))
                    .collect();
                let rest = std::mem::take(map);
                let mut all_of = vec![json!({ "$ref": reference })];
                if !rest.is_empty() {
                    all_of.push(Value::Object(rest));
                }
                map.insert("allOf".into(), Value::Array(all_of));
                map.extend(kept);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(ref_siblings_to_all_of),
        _ => {}
    }
}
