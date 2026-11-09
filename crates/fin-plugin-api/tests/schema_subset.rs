//! The command argument schema subset (8B plan Decision 13): exactly the listed keywords,
//! `additionalProperties: false` at every object, and a linear-time `pattern`.

use std::time::{Duration, Instant};

use fin_plugin_api::schema_subset::{SchemaSubset, validate_args};
use serde_json::{Value, json};

fn schema(v: Value) -> SchemaSubset {
    serde_json::from_value(v).unwrap()
}

/// The top-level object schema with one property `x` of schema `prop`.
fn one(prop: Value) -> SchemaSubset {
    schema(json!({
        "type": "object",
        "properties": { "x": prop },
        "additionalProperties": false
    }))
}

/// The problems' `(path, reason)`, or none.
fn problems(s: &SchemaSubset, args: Value) -> Vec<(String, String)> {
    match validate_args(s, &args) {
        Ok(()) => Vec::new(),
        Err(p) => p.into_iter().map(|p| (p.path, p.reason)).collect(),
    }
}

fn reasons(s: &SchemaSubset, args: Value) -> Vec<String> {
    problems(s, args).into_iter().map(|(_, r)| r).collect()
}

#[test]
fn validate_args_table() {
    let none: &[&str] = &[];
    // (property schema, value of `x`, expected reasons)
    let table: Vec<(Value, Value, &[&str])> = vec![
        (json!({ "type": "string" }), json!("a"), none),
        (json!({ "type": "string" }), json!(1), &["type"]),
        (json!({ "type": "number" }), json!(1.5), none),
        (json!({ "type": "number" }), json!(2), none),
        (json!({ "type": "number" }), json!("1"), &["type"]),
        (json!({ "type": "integer" }), json!(2), none),
        (json!({ "type": "integer" }), json!(2.0), none),
        (json!({ "type": "integer" }), json!(2.5), &["type"]),
        (json!({ "type": "boolean" }), json!(true), none),
        (json!({ "type": "boolean" }), json!(null), &["type"]),
        (
            json!({ "type": "string", "enum": ["a", "b"] }),
            json!("b"),
            none,
        ),
        (
            json!({ "type": "string", "enum": ["a", "b"] }),
            json!("c"),
            &["enum"],
        ),
        (json!({ "type": "integer", "enum": [1, 2] }), json!(2), none),
        (
            json!({ "type": "string", "const": "on" }),
            json!("on"),
            none,
        ),
        (
            json!({ "type": "string", "const": "on" }),
            json!("off"),
            &["const"],
        ),
        (
            json!({ "type": "number", "minimum": 0, "maximum": 1 }),
            json!(0),
            none,
        ),
        (
            json!({ "type": "number", "minimum": 0, "maximum": 1 }),
            json!(1),
            none,
        ),
        (
            json!({ "type": "number", "minimum": 0 }),
            json!(-0.1),
            &["minimum"],
        ),
        (
            json!({ "type": "number", "maximum": 1 }),
            json!(1.1),
            &["maximum"],
        ),
        (
            json!({ "type": "integer", "minimum": -40, "maximum": 0 }),
            json!(-41),
            &["minimum"],
        ),
        (
            json!({ "type": "string", "minLength": 2 }),
            json!("é"),
            &["minLength"],
        ),
        (
            json!({ "type": "string", "minLength": 2 }),
            json!("éé"),
            none,
        ),
        (
            json!({ "type": "string", "maxLength": 2 }),
            json!("abc"),
            &["maxLength"],
        ),
        (
            json!({ "type": "string", "pattern": "^[a-z]+$" }),
            json!("abc"),
            none,
        ),
        (
            json!({ "type": "string", "pattern": "^[a-z]+$" }),
            json!("ab1"),
            &["pattern"],
        ),
        // `pattern` is unanchored, as JSON Schema says.
        (
            json!({ "type": "string", "pattern": "[0-9]" }),
            json!("ab1c"),
            none,
        ),
        (
            json!({ "type": "array", "items": { "type": "string" } }),
            json!(["a", "b"]),
            none,
        ),
        (
            json!({ "type": "array", "items": { "type": "string" } }),
            json!(["a", 1]),
            &["type"],
        ),
        (
            json!({ "type": "array", "items": { "type": "string" }, "minItems": 1 }),
            json!([]),
            &["minItems"],
        ),
        (
            json!({ "type": "array", "items": { "type": "string" }, "maxItems": 1 }),
            json!(["a", "b"]),
            &["maxItems"],
        ),
        (
            json!({ "type": "array", "items": { "type": "string" } }),
            json!("a"),
            &["type"],
        ),
        (
            json!({ "type": "object", "properties": { "y": { "type": "integer" } }, "required": ["y"], "additionalProperties": false }),
            json!({ "y": 1 }),
            none,
        ),
        (
            json!({ "type": "object", "properties": { "y": { "type": "integer" } }, "required": ["y"], "additionalProperties": false }),
            json!({}),
            &["required"],
        ),
        (
            json!({ "type": "object", "properties": { "y": { "type": "integer" } }, "additionalProperties": false }),
            json!({ "z": 1 }),
            &["additionalProperties"],
        ),
        (
            json!({ "type": "string", "description": "arg.x" }),
            json!("a"),
            none,
        ),
    ];
    for (prop, x, want) in table {
        let s = one(prop.clone());
        let got = reasons(&s, json!({ "x": x }));
        assert_eq!(got, want, "{prop} against {x}");
    }
}

#[test]
fn problems_carry_paths_and_every_problem_is_reported() {
    let s = schema(json!({
        "type": "object",
        "properties": {
            "name": { "type": "string", "minLength": 1 },
            "tags": { "type": "array", "items": { "type": "string", "maxLength": 3 } },
            "opts": {
                "type": "object",
                "properties": { "fast": { "type": "boolean" } },
                "required": ["fast"],
                "additionalProperties": false
            }
        },
        "required": ["name"],
        "additionalProperties": false
    }));
    assert_eq!(problems(&s, json!({ "name": "a" })), []);
    let got = problems(
        &s,
        json!({ "tags": ["ok", "toolong"], "opts": { "slow": 1 }, "extra/key~": 0 }),
    );
    let want: Vec<(String, String)> = [
        ("", "required"),
        ("/extra~1key~0", "additionalProperties"),
        ("/opts", "required"),
        ("/opts/slow", "additionalProperties"),
        ("/tags/1", "maxLength"),
    ]
    .iter()
    .map(|(p, r)| (p.to_string(), r.to_string()))
    .collect();
    assert_eq!(got, want);
    // Arguments that are not an object.
    assert_eq!(reasons(&s, json!([1])), ["type"]);
    let p = validate_args(&s, &json!({})).unwrap_err();
    assert!(p[0].message.contains("name"), "{}", p[0].message);
}

/// The schema check's `(at, problem)`, or `None` when the schema is in the subset.
fn refusal(v: Value) -> Option<(String, String)> {
    schema(v).check().err().map(|e| (e.at, e.problem))
}

#[test]
fn schema_outside_subset_refused() {
    let obj = |props: Value| json!({ "type": "object", "properties": props, "additionalProperties": false });
    assert_eq!(refusal(obj(json!({}))), None);
    assert_eq!(
        refusal(json!({ "type": "object", "additionalProperties": false })),
        None
    );
    let cases: Vec<(Value, &str, &str)> = vec![
        (
            obj(json!({ "x": { "$ref": "#/defs/x" } })),
            "/properties/x/$ref",
            "keyword",
        ),
        (
            obj(json!({ "x": { "type": "string", "format": "email" } })),
            "/properties/x/format",
            "keyword",
        ),
        (
            json!({ "type": "object", "oneOf": [], "properties": {}, "additionalProperties": false }),
            "/oneOf",
            "keyword",
        ),
        (
            obj(json!({ "x": { "anyOf": [] } })),
            "/properties/x/anyOf",
            "keyword",
        ),
        (
            obj(json!({ "x": { "type": "string", "default": "a" } })),
            "/properties/x/default",
            "keyword",
        ),
        (
            json!({ "type": "object", "properties": {} }),
            "",
            "additionalProperties",
        ),
        (
            json!({ "type": "object", "properties": {}, "additionalProperties": true }),
            "/additionalProperties",
            "additionalProperties",
        ),
        (
            obj(json!({ "x": { "type": "object", "properties": {} } })),
            "/properties/x",
            "additionalProperties",
        ),
        (json!({ "type": "string" }), "/type", "topType"),
        (
            json!({ "properties": {}, "additionalProperties": false }),
            "",
            "type",
        ),
        (
            obj(json!({ "x": { "type": "null" } })),
            "/properties/x/type",
            "type",
        ),
        (
            obj(json!({ "x": { "type": ["string", "null"] } })),
            "/properties/x/type",
            "type",
        ),
        (
            obj(json!({ "x": { "type": "number", "pattern": "a" } })),
            "/properties/x/pattern",
            "keyword",
        ),
        (
            obj(json!({ "x": { "type": "string", "minimum": 1 } })),
            "/properties/x/minimum",
            "keyword",
        ),
        (
            obj(json!({ "x": { "type": "string", "pattern": "(" } })),
            "/properties/x/pattern",
            "pattern",
        ),
        (
            obj(json!({ "x": { "type": "string", "pattern": "(?=a)" } })),
            "/properties/x/pattern",
            "pattern",
        ),
        (
            obj(json!({ "x": { "type": "string", "pattern": "(a)\\1" } })),
            "/properties/x/pattern",
            "pattern",
        ),
        (
            obj(json!({ "x": { "type": "string", "minLength": -1 } })),
            "/properties/x/minLength",
            "bound",
        ),
        (
            obj(json!({ "x": { "type": "string", "minLength": 3, "maxLength": 2 } })),
            "/properties/x",
            "bound",
        ),
        (
            obj(json!({ "x": { "type": "number", "minimum": 3, "maximum": 2 } })),
            "/properties/x",
            "bound",
        ),
        (
            obj(json!({ "x": { "type": "number", "minimum": "1" } })),
            "/properties/x/minimum",
            "bound",
        ),
        (
            obj(json!({ "x": { "type": "array" } })),
            "/properties/x",
            "items",
        ),
        (
            obj(json!({ "x": { "type": "string", "enum": [] } })),
            "/properties/x/enum",
            "enum",
        ),
        (
            obj(json!({ "x": { "type": "string", "enum": [1] } })),
            "/properties/x/enum/0",
            "enum",
        ),
        (
            obj(json!({ "x": { "type": "string", "const": 1 } })),
            "/properties/x/const",
            "const",
        ),
        (
            obj(json!({ "x": { "type": "string", "description": 1 } })),
            "/properties/x/description",
            "description",
        ),
        (
            obj(json!({ "x": { "type": "string", "description": "Not a key" } })),
            "/properties/x/description",
            "description",
        ),
        (
            json!({ "type": "object", "properties": {}, "required": ["y"], "additionalProperties": false }),
            "/required/0",
            "required",
        ),
        (
            json!({ "type": "object", "properties": { "y": { "type": "string" } }, "required": ["y", "y"], "additionalProperties": false }),
            "/required/1",
            "required",
        ),
        (json!("object"), "", "type"),
    ];
    for (v, at, problem) in cases {
        assert_eq!(
            refusal(v.clone()),
            Some((at.to_string(), problem.to_string())),
            "{v}"
        );
    }
    // A schema outside the subset never validates arguments.
    let s = schema(json!({ "type": "object", "properties": {} }));
    assert_eq!(reasons(&s, json!({})), ["schema"]);
}

#[test]
fn nesting_is_bounded() {
    let mut v = json!({ "type": "string" });
    for _ in 0..40 {
        v = json!({ "type": "array", "items": v });
    }
    let v = json!({ "type": "object", "properties": { "x": v }, "additionalProperties": false });
    let (_, problem) = refusal(v).unwrap();
    assert_eq!(problem, "depth");
}

#[test]
fn catastrophic_regex_is_still_linear() {
    let s = one(json!({ "type": "string", "pattern": "(a+)+$" }));
    let input = format!("{}!", "a".repeat(100_000));
    let t = Instant::now();
    assert_eq!(reasons(&s, json!({ "x": input })), ["pattern"]);
    let nested = one(json!({ "type": "string", "pattern": "^(a|a?)+$" }));
    assert_eq!(
        reasons(&nested, json!({ "x": format!("{}b", "a".repeat(100_000)) })),
        ["pattern"]
    );
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "took {:?}",
        t.elapsed()
    );
}

#[test]
fn schema_round_trips_unchanged() {
    let v = json!({
        "type": "object",
        "properties": { "x": { "type": "string", "pattern": "^a", "description": "arg.x" } },
        "required": ["x"],
        "additionalProperties": false
    });
    let s = schema(v.clone());
    assert_eq!(serde_json::to_value(&s).unwrap(), v);
    assert_eq!(s.check(), Ok(()));
}
