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
///
/// The definitions are schemars' deserialize contract (what the engine accepts: a field with a
/// serde default is optional, so TypeScript builds a `Tx` without it), except that a field the
/// wire omits when absent (`skip_serializing_if`) is never `null` there: [`omitted_not_null`]
/// takes the `null` out of its schema (`field?: T`, not `field?: T | null`). An `Option` field
/// without `skip_serializing_if` is written as `null` and keeps it. `extra` runs twice, once per
/// contract.
pub fn json_schema_with(extra: impl Fn(&mut SchemaGenerator)) -> Value {
    let mut defs = definitions(SchemaSettings::draft2020_12(), &extra);
    let written = definitions(SchemaSettings::draft2020_12().for_serialize(), &extra);
    for (name, v) in defs.iter_mut() {
        if let Some(w) = written.get(name) {
            omitted_not_null(v, w);
        }
        ref_siblings_to_all_of(v);
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "FinCore",
        "$defs": defs,
    })
}

fn definitions(
    settings: SchemaSettings,
    extra: &dyn Fn(&mut SchemaGenerator),
) -> Map<String, Value> {
    let mut generator = SchemaGenerator::new(settings);
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
    generator.take_definitions(true)
}

/// Walks `read` (a deserialize-contract schema) beside `written` (the same schema under the
/// serialize contract) and takes `null` out of each property `written` does not require, which
/// is then optional in `read` too (a skipped field Rust requires on input is still one the wire
/// omits).
/// Under the serialize contract schemars makes a field optional exactly when it has
/// `skip_serializing_if` (a serde default alone does not: the field is always written), so
/// such a property is one the wire omits rather than writes as `null`. Only the `null` schemars
/// adds for an `Option` is removed (see [`strip_null`]). Where the two contracts differ in shape
/// (a variant or field skipped one way only) the walk leaves that part alone.
/// (An `Option<Option<T>>` that skips `None` would lose its inner `null` too: schemars gives it
/// the schema of `Option<T>`. The contract has none.)
fn omitted_not_null(read: &mut Value, written: &Value) {
    match (read, written) {
        (Value::Object(r), Value::Object(w)) => {
            let required: Vec<&str> = w
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut omitted: Vec<String> = Vec::new();
            for (k, x) in r.iter_mut() {
                let Some(wx) = w.get(k) else { continue };
                if k == "required" {
                    continue;
                }
                if k != "properties" {
                    omitted_not_null(x, wx);
                    continue;
                }
                let (Value::Object(props), Value::Object(wprops)) = (x, wx) else {
                    continue;
                };
                for (name, p) in props.iter_mut() {
                    let Some(wp) = wprops.get(name) else { continue };
                    omitted_not_null(p, wp);
                    if !required.contains(&name.as_str()) {
                        strip_null(p);
                        omitted.push(name.clone());
                    }
                }
            }
            if let Some(Value::Array(req)) = r.get_mut("required") {
                req.retain(|n| !n.as_str().is_some_and(|n| omitted.iter().any(|o| o == n)));
                if req.is_empty() {
                    r.remove("required");
                }
            }
        }
        (Value::Array(r), Value::Array(w)) if r.len() == w.len() => {
            for (x, wx) in r.iter_mut().zip(w) {
                omitted_not_null(x, wx);
            }
        }
        _ => {}
    }
}

/// Undoes the `null` schemars' `Option<T>` adds to `T`'s schema: `anyOf [S, {type: null}]`
/// becomes `S` (with the field's own keywords beside it), `"null"` leaves a `type` list, and
/// `null` an `enum` (a lone value left is the `const` it was).
fn strip_null(p: &mut Value) {
    let Value::Object(map) = p else { return };
    let is_null = |m: &Value| {
        m.as_object()
            .is_some_and(|o| o.len() == 1 && o.get("type") == Some(&json!("null")))
    };
    if let Some(Value::Array(any)) = map.get("anyOf")
        && any.len() == 2
        && let Some(i) = any.iter().position(is_null)
    {
        let Some(Value::Array(mut any)) = map.remove("anyOf") else {
            return;
        };
        if let Value::Object(other) = any.swap_remove(1 - i) {
            for (k, v) in other {
                map.entry(k).or_insert(v);
            }
        }
        return;
    }
    if let Some(Value::Array(types)) = map.get_mut("type") {
        types.retain(|t| t != "null");
        if types.len() == 1 {
            let t = types.remove(0);
            map.insert("type".into(), t);
        }
    }
    if let Some(Value::Array(values)) = map.get_mut("enum")
        && values.contains(&Value::Null)
    {
        values.retain(|v| !v.is_null());
        if values.len() == 1 {
            let c = values.remove(0);
            map.remove("enum");
            map.insert("const".into(), c);
        }
    }
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
