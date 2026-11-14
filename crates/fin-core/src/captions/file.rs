//! A caption style as a file (8A Task 12a): the JSON a content pack ships for
//! `contributes.captionStyles[].file`.
//!
//! The file is a [`CaptionStyle`] without its `id` (the project gives a copy one when
//! `style.add` makes it), so the generated model type is the schema. It is checked on three
//! levels, all derived from the model rather than written beside it:
//!
//! 1. the shape: it deserialises as a `CaptionStyle` (with an empty `id`), and no key
//!    anywhere in it is one the model would drop (a typo never silently loses a setting);
//! 2. every style path of the parameter registry holds a value of its shape and in its range
//!    (what `style.add`'s `set` would enforce: colours, ranges, enum names, safe zones, the
//!    rationals' canonical form);
//! 3. the name is not empty.

use serde_json::Value;

use crate::model::CaptionStyle;
use crate::params::{Scope, get_style, specs, validate};

/// Parses and validates a style file; the style's `id` is empty. The error is a message for
/// the manifest check that reports it.
pub fn parse_style_file(bytes: &[u8]) -> Result<CaptionStyle, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
    let input: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let Some(object) = input.as_object() else {
        return Err("a caption style is a JSON object".into());
    };
    if object.contains_key("id") {
        return Err("a style file has no \"id\": the project names the copy it makes".into());
    }
    let mut keyed = input.clone();
    keyed["id"] = Value::String(String::new());
    let style: CaptionStyle =
        serde_json::from_value(keyed).map_err(|e| format!("not a caption style: {e}"))?;
    // What the model keeps, written back: a key of the file it lacks is one the model does
    // not know (`null` for an absent optional field is fine).
    let kept = serde_json::to_value(&style).map_err(|e| e.to_string())?;
    if let Some(path) = dropped_key(&input, &kept, "") {
        return Err(format!("{path} is not a caption style field"));
    }
    if style.name.is_empty() {
        return Err("name is empty".into());
    }
    for spec in specs().iter().filter(|s| s.scope == Scope::Style) {
        let value = get_style(&style, spec.path).map_err(|e| e.message)?;
        if let Some(v) = value {
            validate(spec, &v).map_err(|e| e.message)?;
        }
    }
    Ok(style)
}

/// The first path of `input` (objects only) that `kept` does not have.
fn dropped_key(input: &Value, kept: &Value, at: &str) -> Option<String> {
    let (Value::Object(i), Value::Object(k)) = (input, kept) else {
        return None;
    };
    for (key, v) in i {
        let path = if at.is_empty() {
            key.clone()
        } else {
            format!("{at}.{key}")
        };
        match k.get(key) {
            Some(inner) => {
                if let Some(found) = dropped_key(v, inner, &path) {
                    return Some(found);
                }
            }
            None if v.is_null() => {}
            None => return Some(path),
        }
    }
    None
}
