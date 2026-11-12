//! What the fake host serves and sends: the fixture snapshot (`project.snapshot`,
//! `project.view`) and each command's sample arguments, made from its schema.

use std::path::Path;

use fin_core::model::{Marker, new_project};
use fin_core::{AudioParams, Item, ItemBody, Project, Rational};
use fin_plugin_api::schema_subset::CompiledArgs;
use serde_json::{Map, Value, json};

use crate::HarnessError;

/// A project at a revision, as the host holds a command's snapshot.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub project: Project,
    /// The project's canonical JSON (what `project.snapshot` pages).
    pub bytes: Vec<u8>,
    pub rev: u64,
    /// `builtin` or `file`.
    pub source: &'static str,
}

fn t(n: i64) -> Rational {
    Rational::from_int(n)
}

impl Snapshot {
    fn of(project: Project, rev: u64, source: &'static str) -> Snapshot {
        let bytes = serde_json::to_vec(&project).expect("a project serialises");
        Snapshot {
            project,
            bytes,
            rev,
            source,
        }
    }

    /// The built-in fixture at rev 1: a 25 fps project whose main timeline has five one-second
    /// items on `V1` and one on `A1` (gain −90 dB), span markers labelled `silence` over 1–3 s
    /// and 3.5–9.5 s, a point marker and a `chapter` span. Commands that look for silence find
    /// some, so their transactions are not empty.
    pub fn builtin() -> Snapshot {
        let mut p = new_project("p1".into(), "Fixture".into(), t(25)).expect("a project");
        let tl = &mut p.timelines[0];
        for i in 0..5 {
            let item = Item::new(format!("adj{}", i + 1), t(i), t(1), ItemBody::Adjustment {});
            tl.tracks[0].items.push(item);
        }
        let mut quiet = Item::new("adj6", t(2), t(1), ItemBody::Adjustment {});
        quiet.common.audio = Some(AudioParams {
            gain: -90.0,
            ..AudioParams::default()
        });
        tl.tracks[1].items.push(quiet);
        let marker = |id: &str, at: Rational, duration: Option<Rational>, label: &str| Marker {
            id: id.into(),
            at,
            duration,
            label: label.into(),
            color: None,
        };
        tl.markers = vec![
            marker("m1", t(0), None, "silence"),
            marker("m2", t(1), Some(t(2)), "silence"),
            marker(
                "m3",
                Rational::new(7, 2).expect("7/2"),
                Some(t(6)),
                "silence",
            ),
            marker("m4", t(0), Some(t(10)), "chapter"),
        ];
        Snapshot::of(p, 1, "builtin")
    }

    /// A fixture file: a fin-core project as JSON, or `{rev, project}`.
    pub fn load(path: &Path) -> Result<Snapshot, HarnessError> {
        let invalid = |message: String| {
            HarnessError::new("FIXTURE_INVALID", message).with("path", path.display().to_string())
        };
        let bytes = std::fs::read(path).map_err(|e| {
            HarnessError::new("IO_ERROR", format!("read {}: {e}", path.display()))
                .with("op", "read")
                .with("path", path.display().to_string())
        })?;
        let v: Value =
            serde_json::from_slice(&bytes).map_err(|e| invalid(format!("not JSON: {e}")))?;
        let (rev, project) = match (v.get("rev").and_then(Value::as_u64), v.get("project")) {
            (Some(rev), Some(project)) => (rev, project.clone()),
            _ => (1, v),
        };
        let project: Project = serde_json::from_value(project)
            .map_err(|e| invalid(format!("not a fin-core project: {e}")))?;
        if project.schema_version != fin_core::SCHEMA_VERSION {
            return Err(invalid(format!(
                "the project is schema version {}, this host's is {}",
                project.schema_version,
                fin_core::SCHEMA_VERSION
            )));
        }
        Ok(Snapshot::of(project, rev, "file"))
    }
}

/// Arguments outside every schema of the subset (each object says `additionalProperties:
/// false`): the host must refuse them before a job exists.
pub fn outside_args() -> Value {
    json!({ "$finPluginTestUnexpected": true })
}

/// Arguments that pass `schema`, made from it (required properties only, the first `enum`
/// value, `const`, a number inside its bounds, a string from a pattern's literal prefix), or
/// `None` when none of the candidates passes.
pub fn sample_args(schema: &CompiledArgs, raw: &Value) -> Option<Value> {
    let mut candidates = vec![sample(raw, false)];
    candidates.push(sample(raw, true));
    candidates.push(json!({}));
    candidates.into_iter().find(|c| schema.validate(c).is_ok())
}

/// One value for `schema`; `all` also fills optional properties.
fn sample(schema: &Value, all: bool) -> Value {
    if let Some(c) = schema.get("const") {
        return c.clone();
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|e| e.first())
    {
        return first.clone();
    }
    let num = |key: &str| schema.get(key).and_then(Value::as_f64);
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => {
            let required: Vec<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|r| r.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let mut out = Map::new();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                for (k, s) in props {
                    if all || required.contains(&k.as_str()) {
                        out.insert(k.clone(), sample(s, all));
                    }
                }
            }
            Value::Object(out)
        }
        Some("array") => {
            let n = schema.get("minItems").and_then(Value::as_u64).unwrap_or(0);
            let item = schema.get("items").cloned().unwrap_or(Value::Null);
            Value::Array((0..n).map(|_| sample(&item, all)).collect())
        }
        Some(kind @ ("number" | "integer")) => {
            let (lo, hi) = (num("minimum"), num("maximum"));
            let mut x = 0.0_f64;
            if let Some(lo) = lo {
                x = x.max(lo);
            }
            if let Some(hi) = hi {
                x = x.min(hi);
            }
            if kind == "integer" {
                json!(x.ceil() as i64)
            } else {
                json!(x)
            }
        }
        Some("boolean") => json!(false),
        Some("string") => {
            let mut s = schema
                .get("pattern")
                .and_then(Value::as_str)
                .map(literal_prefix)
                .unwrap_or_default();
            let min = schema.get("minLength").and_then(Value::as_u64).unwrap_or(0) as usize;
            while s.chars().count() < min {
                s.push('a');
            }
            json!(s)
        }
        _ => Value::Null,
    }
}

/// The literal text a pattern starts with after `^` (`^https://api\.example\.org/` →
/// `https://api.example.org/`), up to its first metacharacter.
pub fn literal_prefix(pattern: &str) -> String {
    let mut out = String::new();
    let mut chars = pattern
        .strip_prefix('^')
        .unwrap_or(pattern)
        .chars()
        .peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some(e) if e.is_ascii_punctuation() => out.push(e),
                _ => break,
            },
            '[' | ']' | '(' | ')' | '{' | '}' | '*' | '+' | '?' | '|' | '.' | '^' | '$' => {
                // A quantifier applies to the character before it, which may then be absent.
                if matches!(c, '*' | '?' | '{') {
                    out.pop();
                }
                break;
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fin_plugin_api::SchemaSubset;

    fn compiled(v: &Value) -> CompiledArgs {
        CompiledArgs::new(&SchemaSubset::new(v.clone())).unwrap()
    }

    #[test]
    fn literal_prefixes() {
        assert_eq!(
            literal_prefix("^https://api\\.example\\.org/"),
            "https://api.example.org/"
        );
        assert_eq!(literal_prefix("^/[A-Za-z0-9/_.-]*$"), "/");
        assert_eq!(literal_prefix("^ab*c"), "a");
        assert_eq!(literal_prefix("[a-z]+"), "");
    }

    #[test]
    fn samples_pass_their_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "pattern": "^https://api\\.example\\.org/", "maxLength": 2048 },
                "n": { "type": "integer", "minimum": 3, "maximum": 9 },
                "db": { "type": "number", "minimum": -120, "maximum": -10 },
                "mode": { "type": "string", "enum": ["ripple", "lift"] },
                "tags": { "type": "array", "items": { "type": "string", "minLength": 2 }, "minItems": 1 },
            },
            "required": ["url", "n", "db", "tags"],
            "additionalProperties": false
        });
        let got = sample_args(&compiled(&schema), &schema).unwrap();
        assert_eq!(
            got,
            json!({ "url": "https://api.example.org/", "n": 3, "db": -10.0, "tags": ["aa"] })
        );
        assert!(compiled(&schema).validate(&outside_args()).is_err());
    }

    #[test]
    fn builtin_fixture_round_trips() {
        let s = Snapshot::builtin();
        let back: Project = serde_json::from_slice(&s.bytes).unwrap();
        assert_eq!(back, s.project);
        assert_eq!(s.rev, 1);
    }
}
