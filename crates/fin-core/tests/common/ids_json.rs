//! `ids_cover_every_id_in_json` (plan 4C Decision 24): every id-shaped string an op carries
//! must be listed by `Op::ids()`.
#![allow(dead_code)]

use fin_core::Op;
use serde_json::Value;

/// Fields whose strings are free text, never ids.
const FREE_TEXT: &[&str] = &["label", "name", "text", "path", "fingerprint", "color"];

/// An id: a `$placeholder`, or one to three lowercase letters followed by digits (`c1`, `tl2`).
fn id_shaped(s: &str) -> bool {
    if s.starts_with('$') {
        return s.len() > 1;
    }
    let letters = s.bytes().take_while(u8::is_ascii_lowercase).count();
    (1..=3).contains(&letters)
        && s.len() > letters
        && s.bytes().skip(letters).all(|b| b.is_ascii_digit())
}

fn walk(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if id_shaped(s) => out.push(s.clone()),
        Value::Array(items) => items.iter().for_each(|v| walk(v, out)),
        Value::Object(map) => {
            for (key, v) in map {
                if !FREE_TEXT.contains(&key.as_str()) {
                    walk(v, out);
                }
            }
        }
        _ => {}
    }
}

/// Checks `json` (an op's wire form) against `op.ids()`. `Err` names the id-shaped strings the
/// op does not list.
pub fn ids_cover_json(op: &Op, json: &Value) -> Result<(), Vec<String>> {
    let mut found = Vec::new();
    // The `op` tag is a name, not an id.
    match json {
        Value::Object(map) => {
            for (key, v) in map.iter().filter(|(k, _)| k.as_str() != "op") {
                if !FREE_TEXT.contains(&key.as_str()) {
                    walk(v, &mut found);
                }
            }
        }
        other => walk(other, &mut found),
    }
    let listed = op.ids();
    let missing: Vec<String> = found
        .into_iter()
        .filter(|id| !listed.contains(&id.as_str()))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Panics unless every id in `op`'s JSON is listed by `op.ids()`.
pub fn ids_cover_every_id_in_json(op: &Op) {
    let json = serde_json::to_value(op).unwrap();
    if let Err(missing) = ids_cover_json(op, &json) {
        panic!("{} does not list ids {missing:?}", op.name());
    }
}
