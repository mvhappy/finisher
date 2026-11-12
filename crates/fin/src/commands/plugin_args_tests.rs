use serde_json::{Value, json};

use super::{kebab, map_args, usage_args};

fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "op": { "type": "string", "enum": ["a", "b"] },
            "sleepMs": { "type": "integer" },
            "gain": { "type": "number" },
            "loud": { "type": "boolean" },
            "tags": { "type": "array", "items": { "type": "string" } },
            "frames": { "type": "array", "items": { "type": "integer" } },
            "box": { "type": "object", "properties": {}, "additionalProperties": false },
            "branch": { "type": "string" },
            "dry_run": { "type": "boolean" },
            "a_b": { "type": "string" },
            "aB": { "type": "integer" },
        },
        "required": ["op"],
        "additionalProperties": false,
    })
}

fn map(tokens: &[&str]) -> Result<Value, String> {
    let t: Vec<String> = tokens.iter().map(|s| (*s).to_string()).collect();
    map_args("plugin:@a/b/c", &schema(), &t).map_err(|e| {
        assert_eq!(e.code, "USAGE", "{e:?}");
        e.message
    })
}

#[test]
fn kebab_spells_camel_case() {
    assert_eq!(kebab("sleepMs"), "sleep-ms");
    assert_eq!(kebab("op"), "op");
    assert_eq!(kebab("a_b"), "a-b");
}

#[test]
fn flags_map_by_type() {
    let ok = |t: &[&str], v: Value| assert_eq!(map(t).unwrap(), v, "{t:?}");
    ok(&[], json!({}));
    ok(&["--op", "a"], json!({ "op": "a" }));
    ok(&["--op=b"], json!({ "op": "b" }));
    ok(&["--sleep-ms", "5"], json!({ "sleepMs": 5 }));
    ok(&["--sleepMs", "-5"], json!({ "sleepMs": -5 }));
    ok(&["--gain", "1.5"], json!({ "gain": 1.5 }));
    ok(&["--loud"], json!({ "loud": true }));
    ok(&["--loud=false"], json!({ "loud": false }));
    ok(
        &["--tags", "x", "--tags", "y"],
        json!({ "tags": ["x", "y"] }),
    );
    ok(
        &["--frames", "1", "--frames", "2"],
        json!({ "frames": [1, 2] }),
    );
    ok(
        &["--args-json", r#"{"box":{}}"#, "--op", "a"],
        json!({ "box": {}, "op": "a" }),
    );
    // Two properties with one kebab flag: the first in key order has it (`aB` before `a_b`).
    ok(&["--a-b", "2"], json!({ "aB": 2 }));
    ok(
        &["--args-json", r#"{"branch":"b","a_b":"y"}"#],
        json!({ "branch": "b", "a_b": "y" }),
    );
    // The schema decides nothing else here: an enum miss is the server's to refuse.
    ok(&["--op", "zzz"], json!({ "op": "zzz" }));
}

#[test]
fn bad_command_lines_are_usage() {
    for t in [
        vec!["pos"],
        vec!["--nope", "1"],
        vec!["--op"],
        vec!["--op", "--loud"],
        vec!["--op", "a", "--op", "b"],
        vec!["--sleep-ms", "x"],
        vec!["--sleep-ms", "1.5"],
        vec!["--gain", "x"],
        vec!["--loud=maybe"],
        vec!["--frames", "q"],
        vec!["--box", "{}"],
        vec!["--args-json", "[1]"],
        vec!["--args-json", "{}", "--args-json", "{}"],
        vec!["--op", "a", "--args-json", r#"{"op":"b"}"#],
        vec!["--"],
        // fin's own flag names are never a property's: `branch` goes in --args-json.
        vec!["--branch", "x"],
        vec!["--dry-run"],
        // The second property of one kebab flag has none of its own.
        vec!["--a_b", "x"],
    ] {
        assert!(map(&t).is_err(), "{t:?} was accepted");
    }
}

#[test]
fn usage_line_from_schema() {
    assert_eq!(
        usage_args(&schema()),
        "--op <a|b> [--a-b <integer>] [--frames <integer>…] [--gain <number>] [--loud] \
         [--sleep-ms <integer>] [--tags <string>…] \
         [--args-json '{\"a_b\": …, \"box\": …, \"branch\": …, \"dry_run\": …}']"
    );
}
