//! A plugin command's command line mapped onto its argument schema (sp8b Task 10, spec §3.8;
//! the schema is the restricted subset of plan 8B Decision 13). `fin`'s conventions:
//! - `--name value` (or `--name=value`) for a `string`, an `integer` or a `number` property,
//!   parsed by the property's type (`--count 3` is `3`, never `"3"`);
//! - `--flag` for a `boolean` (`true`; `--flag=false` for `false`);
//! - an `array` repeats its flag, one item per occurrence (`--tag a --tag b`), each parsed by
//!   the `items` type;
//! - `--args-json '<object>'` for anything else (objects, arrays of objects), merged with the
//!   flags; a key given both ways is refused.
//!
//! A property is spelled as written (`--sleepMs`) or in kebab case (`--sleep-ms`). A property
//! whose flag would be one of fin's own (`RESERVED`: `--project`, `--branch`, `--dry-run`, …),
//! or the same flag as an earlier property's, has no flag: it goes in `--args-json`, and its
//! usage line says so. Only the
//! command line is checked here (`USAGE`, exit 2): the server checks the arguments against the
//! schema itself (`PLUGIN_COMMAND_ARGS`, exit 3).

use fin_core::CoreError;
use serde_json::{Map, Value, json};

use crate::output::usage;

/// The flag that carries a JSON object of arguments.
pub const ARGS_JSON: &str = "args-json";

/// fin's own flags on a plugin command's line (`plugin_cmd::split`) and `--args-json` itself.
pub const RESERVED: &[&str] = &[
    "project", "branch", "dry-run", "idem-key", "timeline", "help", ARGS_JSON,
];

/// `sleepMs` → `sleep-ms`.
pub fn kebab(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '_' {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    out
}

/// A property's type: `type`, else the JSON type of its `const` or first `enum` value, else
/// `string`.
fn type_of(prop: &Value) -> &'static str {
    let named = prop.get("type").and_then(Value::as_str);
    let sample = prop
        .get("const")
        .or_else(|| prop.get("enum").and_then(|e| e.get(0)));
    match (named, sample) {
        (Some("string"), _) => "string",
        (Some("integer"), _) => "integer",
        (Some("number"), _) => "number",
        (Some("boolean"), _) => "boolean",
        (Some("array"), _) => "array",
        (Some(_), _) => "object",
        (None, Some(Value::Bool(_))) => "boolean",
        (None, Some(Value::Number(n))) if n.is_i64() || n.is_u64() => "integer",
        (None, Some(Value::Number(_))) => "number",
        (None, Some(Value::Object(_) | Value::Array(_))) => "object",
        (None, _) => "string",
    }
}

fn properties(schema: &Value) -> Vec<(&str, &Value)> {
    schema
        .get("properties")
        .and_then(Value::as_object)
        .map(|m| m.iter().map(|(k, v)| (k.as_str(), v)).collect())
        .unwrap_or_default()
}

fn required(schema: &Value) -> Vec<&str> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

/// The properties that have a flag (in key order), and those reached only through
/// `--args-json`: a reserved flag, or one an earlier property already has.
fn split_props(schema: &Value) -> (Vec<(&str, &Value)>, Vec<&str>) {
    let mut flagged: Vec<(&str, &Value)> = Vec::new();
    let mut json_only = Vec::new();
    for (name, prop) in properties(schema) {
        let flag = kebab(name);
        if RESERVED.contains(&flag.as_str()) || flagged.iter().any(|(k, _)| kebab(k) == flag) {
            json_only.push(name);
        } else {
            flagged.push((name, prop));
        }
    }
    (flagged, json_only)
}

/// The property a flag names: its own spelling first, then the kebab one.
fn find<'a>(props: &[(&'a str, &'a Value)], flag: &str) -> Option<(&'a str, &'a Value)> {
    props
        .iter()
        .find(|(k, _)| *k == flag)
        .or_else(|| props.iter().find(|(k, _)| kebab(k) == flag))
        .copied()
}

/// What a command's arguments look like on the command line, from its schema
/// (`--op <command|look> [--count <integer>] [--dry]`).
pub fn usage_args(schema: &Value) -> String {
    let req = required(schema);
    let mut parts: Vec<(bool, String)> = Vec::new();
    let (flagged, mut json_keys) = split_props(schema);
    for (name, prop) in flagged {
        let flag = format!("--{}", kebab(name));
        let value = |t: &str, p: &Value| match p.get("enum").and_then(Value::as_array) {
            Some(e) => format!(
                "<{}>",
                e.iter()
                    .map(|v| v.as_str().map_or_else(|| v.to_string(), str::to_string))
                    .collect::<Vec<_>>()
                    .join("|")
            ),
            None => format!("<{t}>"),
        };
        let part = match type_of(prop) {
            "boolean" => flag,
            "array" => {
                let items = prop.get("items").unwrap_or(&Value::Null);
                match type_of(items) {
                    "object" | "array" => {
                        json_keys.push(name);
                        continue;
                    }
                    t => format!("{flag} {}…", value(t, items)),
                }
            }
            "object" => {
                json_keys.push(name);
                continue;
            }
            t => format!("{flag} {}", value(t, prop)),
        };
        let needed = req.contains(&name);
        parts.push((!needed, if needed { part } else { format!("[{part}]") }));
    }
    // Required flags first, then the rest in key order.
    parts.sort_by_key(|(optional, _)| *optional);
    let mut parts: Vec<String> = parts.into_iter().map(|(_, p)| p).collect();
    if !json_keys.is_empty() {
        json_keys.sort_unstable();
        let keys: Vec<String> = json_keys.iter().map(|k| format!("\"{k}\": …")).collect();
        parts.push(format!("[--{ARGS_JSON} '{{{}}}']", keys.join(", ")));
    }
    parts.join(" ")
}

fn bad(command: &str, message: String, details: Value, fix: &str) -> CoreError {
    let mut d = json!({ "command": command });
    if let (Some(m), Some(more)) = (d.as_object_mut(), details.as_object()) {
        m.extend(more.clone());
    }
    usage(message, d, fix)
}

/// One scalar from its text, by type.
fn scalar(t: &str, text: &str) -> Result<Value, String> {
    match t {
        "integer" => text
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| format!("{text:?} is not an integer")),
        "number" => text
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| format!("{text:?} is not a number")),
        "boolean" => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(format!("{text:?} is not true or false")),
        },
        _ => Ok(Value::String(text.to_string())),
    }
}

/// `tokens` (after `fin @<id> <command>`, fin's own flags removed) as the command's `args`.
/// `command` names it in errors (`plugin:<id>/<cmd>`).
pub fn map_args(command: &str, schema: &Value, tokens: &[String]) -> Result<Value, CoreError> {
    let (props, _) = split_props(schema);
    let fix = "run fin help @<id> for the command's flags; objects go in --args-json '{…}'";
    let accepted = || {
        let mut f: Vec<String> = props
            .iter()
            .map(|(k, _)| format!("--{}", kebab(k)))
            .collect();
        f.push(format!("--{ARGS_JSON}"));
        f
    };
    let mut out = Map::new();
    let mut from_json: Option<Map<String, Value>> = None;
    let mut i = 0;
    while i < tokens.len() {
        let token = &tokens[i];
        i += 1;
        let Some(body) = token.strip_prefix("--").filter(|b| !b.is_empty()) else {
            return Err(bad(
                command,
                format!("{token:?}: plugin command arguments are --name value"),
                json!({ "arg": token, "accepted": accepted() }),
                fix,
            ));
        };
        let (flag, inline) = match body.split_once('=') {
            Some((f, v)) => (f, Some(v.to_string())),
            None => (body, None),
        };
        // The next token as a value, unless it is a flag itself.
        let mut take = |what: &str| -> Result<String, CoreError> {
            if let Some(v) = &inline {
                return Ok(v.clone());
            }
            match tokens.get(i) {
                Some(v) if !v.starts_with("--") => {
                    i += 1;
                    Ok(v.clone())
                }
                _ => Err(bad(
                    command,
                    format!("--{flag} needs {what}"),
                    json!({ "arg": format!("--{flag}") }),
                    fix,
                )),
            }
        };
        if flag == ARGS_JSON {
            let text = take("a JSON object")?;
            let parsed = serde_json::from_str::<Value>(&text).ok();
            let Some(Value::Object(obj)) = parsed else {
                return Err(bad(
                    command,
                    format!("--{ARGS_JSON} takes a JSON object"),
                    json!({ "arg": format!("--{ARGS_JSON}") }),
                    fix,
                ));
            };
            if from_json.replace(obj).is_some() {
                return Err(bad(
                    command,
                    format!("--{ARGS_JSON} is given twice"),
                    json!({ "arg": format!("--{ARGS_JSON}") }),
                    fix,
                ));
            }
            continue;
        }
        let Some((name, prop)) = find(&props, flag) else {
            return Err(bad(
                command,
                format!("--{flag} is not an argument of {command}"),
                json!({ "arg": format!("--{flag}"), "accepted": accepted() }),
                fix,
            ));
        };
        let value = match type_of(prop) {
            "boolean" => match &inline {
                None => Ok(Value::Bool(true)),
                Some(v) => scalar("boolean", v),
            }
            .map_err(|why| (why, "true or false"))
            .map(Some),
            "array" => {
                let items = prop.get("items").unwrap_or(&Value::Null);
                let t = type_of(items);
                if matches!(t, "object" | "array") {
                    Err((format!("--{flag} holds objects"), "--args-json"))
                } else {
                    match scalar(t, &take(&format!("a {t} value"))?) {
                        Ok(item) => {
                            let list = out.entry(name).or_insert_with(|| json!([]));
                            if let Some(a) = list.as_array_mut() {
                                a.push(item);
                            }
                            Ok(None)
                        }
                        Err(why) => Err((why, t)),
                    }
                }
            }
            "object" => Err((format!("--{flag} is an object"), "--args-json")),
            t => scalar(t, &take(&format!("a {t} value"))?)
                .map_err(|why| (why, t))
                .map(Some),
        };
        match value {
            Ok(None) => {}
            Ok(Some(v)) => {
                if out.insert(name.to_string(), v).is_some() {
                    return Err(bad(
                        command,
                        format!("--{flag} is given twice"),
                        json!({ "arg": format!("--{flag}") }),
                        fix,
                    ));
                }
            }
            Err((why, expected)) => {
                return Err(bad(
                    command,
                    format!("--{flag}: {why}"),
                    json!({ "arg": format!("--{flag}"), "expected": expected }),
                    fix,
                ));
            }
        }
    }
    if let Some(base) = from_json {
        for (k, v) in base {
            if out.contains_key(&k) {
                return Err(bad(
                    command,
                    format!("{k:?} is given both as a flag and in --{ARGS_JSON}"),
                    json!({ "arg": k }),
                    fix,
                ));
            }
            out.insert(k, v);
        }
    }
    Ok(Value::Object(out))
}

#[cfg(test)]
#[path = "plugin_args_tests.rs"]
mod tests;
