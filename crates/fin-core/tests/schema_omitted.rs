//! The contract schema's rule for absent fields (`fin_core::json_schema_with`): a field the wire
//! omits when absent (`skip_serializing_if`) is optional and never `null`; an `Option` written
//! as `null` stays nullable; a serde default keeps a field optional (the input side).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Serialize, Deserialize, JsonSchema)]
#[allow(dead_code)]
struct Inner {
    n: u32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[allow(dead_code)]
enum Mode {
    A,
    B,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
struct Probe {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted: Option<String>,
    /// A doc beside the type.
    #[serde(skip_serializing_if = "Option::is_none")]
    omitted_ref: Option<Inner>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted_enum: Option<Mode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    omitted_list: Option<Vec<u32>>,
    #[serde(default)]
    written_null: Option<String>,
    written_null_ref: Option<Inner>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    list: Vec<u32>,
    #[serde(default)]
    defaulted: u32,
    plain: u32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[allow(dead_code)]
enum Tagged {
    One {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        omitted: Option<u32>,
        written: Option<u32>,
    },
}

fn defs() -> Value {
    let schema = fin_core::json_schema_with(|g| {
        g.subschema_for::<Probe>();
        g.subschema_for::<Tagged>();
    });
    schema["$defs"].clone()
}

fn required(s: &Value) -> Vec<&str> {
    s["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

#[test]
fn omitted_fields_are_optional_and_not_null() {
    let defs = defs();
    let probe = &defs["Probe"];
    let props = &probe["properties"];
    assert_eq!(props["omitted"], json!({ "type": "string" }));
    assert_eq!(
        props["omittedRef"],
        json!({ "allOf": [{ "$ref": "#/$defs/Inner" }], "description": "A doc beside the type." })
    );
    assert_eq!(props["omittedEnum"], json!({ "$ref": "#/$defs/Mode" }));
    assert_eq!(props["omittedList"]["type"], "array");
    for name in ["omitted", "omittedRef", "omittedEnum", "omittedList"] {
        assert!(!required(probe).contains(&name), "{name} is optional");
    }
}

#[test]
fn options_written_as_null_stay_nullable() {
    let defs = defs();
    let probe = &defs["Probe"];
    let props = &probe["properties"];
    assert_eq!(props["writtenNull"]["type"], json!(["string", "null"]));
    assert_eq!(
        props["writtenNullRef"]["anyOf"],
        json!([{ "$ref": "#/$defs/Inner" }, { "type": "null" }])
    );
    // The deserialize contract decides `required`: an `Option` or a serde default may be left out.
    assert_eq!(required(probe), ["plain"]);
    assert_eq!(props["list"]["type"], "array");
    assert_eq!(props["defaulted"]["type"], "integer");
}

#[test]
fn tagged_variants_follow_the_same_rule() {
    let defs = defs();
    let one = &defs["Tagged"]["oneOf"][0];
    assert_eq!(one["properties"]["omitted"]["type"], "integer");
    assert_eq!(
        one["properties"]["written"]["type"],
        json!(["integer", "null"])
    );
    assert_eq!(required(one), ["kind"]);
}
