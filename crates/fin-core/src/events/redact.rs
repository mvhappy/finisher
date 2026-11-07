//! Redaction (plan D, Decisions 10 and 12): the content paths of a kind, computed from its
//! JSON Schema (`x-fin-class: content`), so a new content field needs no second list; their
//! removal unless training is on; and the blobs an entry on disk references (`x-fin-blob`
//! inside content).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::{Map, Value};

use super::Purposes;
use super::class::{BlobRef, CONTENT, ContentKind};
use super::kinds::{KindSpec, kinds};

/// One step of a path into a payload.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Seg {
    /// An object member.
    Key(String),
    /// Every element of an array.
    Items,
    /// Every value of a map.
    Values,
}

/// A content field: where it is (always ending at an object member, which redaction
/// removes) and the kind its schema names (`x-fin-content`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentPath {
    pub path: Vec<Seg>,
    pub kind: Option<ContentKind>,
}

impl fmt::Display for ContentPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt_path(&self.path, f)
    }
}

fn fmt_path(path: &[Seg], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    for (i, seg) in path.iter().enumerate() {
        match seg {
            Seg::Key(k) if i == 0 => f.write_str(k)?,
            Seg::Key(k) => write!(f, ".{k}")?,
            Seg::Items => f.write_str("[]")?,
            Seg::Values => f.write_str("{}")?,
        }
    }
    Ok(())
}

/// What a kind's schema says about content.
#[derive(Debug, Default)]
struct Paths {
    content: Vec<ContentPath>,
    blobs: Vec<Vec<Seg>>,
}

struct Walk<'a> {
    defs: Option<&'a Map<String, Value>>,
    out: Paths,
    stack: Vec<&'a str>,
}

impl<'a> Walk<'a> {
    fn node(
        &mut self,
        node: &'a Value,
        path: &mut Vec<Seg>,
        in_content: bool,
        field: Option<&'a str>,
    ) {
        let Some(obj) = node.as_object() else {
            return;
        };
        let field = obj.get("x-fin-content").and_then(Value::as_str).or(field);
        let mut in_content = in_content;
        if !in_content && obj.get("x-fin-class").and_then(Value::as_str) == Some(CONTENT) {
            let keep = path
                .iter()
                .rposition(|s| matches!(s, Seg::Key(_)))
                .map_or(0, |i| i + 1);
            self.out.content.push(ContentPath {
                path: path[..keep].to_vec(),
                kind: field.and_then(ContentKind::parse),
            });
            in_content = true;
        }
        if in_content && obj.get("x-fin-blob").and_then(Value::as_bool) == Some(true) {
            self.out.blobs.push(path.clone());
            return;
        }
        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            let name = r.rsplit('/').next().unwrap_or(r);
            // Our schemas are generated with their `$defs`: an unresolved `$ref` is a bug, and
            // skipping it could miss a content field.
            let def = self
                .defs
                .and_then(|d| d.get(name))
                .unwrap_or_else(|| panic!("unresolved $ref {r} in an event schema"));
            if !self.stack.contains(&name) {
                self.stack.push(name);
                self.node(def, path, in_content, field);
                self.stack.pop();
            }
        }
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (k, v) in props {
                path.push(Seg::Key(k.clone()));
                self.node(v, path, in_content, None);
                path.pop();
            }
        }
        for (key, seg) in [("items", Seg::Items), ("additionalProperties", Seg::Values)] {
            if let Some(v) = obj.get(key).filter(|v| v.is_object()) {
                path.push(seg);
                self.node(v, path, in_content, field);
                path.pop();
            }
        }
        for key in ["prefixItems", "anyOf", "oneOf", "allOf"] {
            for v in obj.get(key).and_then(Value::as_array).into_iter().flatten() {
                let items = key == "prefixItems";
                if items {
                    path.push(Seg::Items);
                }
                self.node(v, path, in_content, field);
                if items {
                    path.pop();
                }
            }
        }
    }
}

fn paths_of(schema: &Value) -> Paths {
    let mut walk = Walk {
        defs: schema.get("$defs").and_then(Value::as_object),
        out: Paths::default(),
        stack: Vec::new(),
    };
    walk.node(schema, &mut Vec::new(), false, None);
    let mut out = walk.out;
    out.content.sort();
    out.content.dedup();
    out.blobs.sort();
    out.blobs.dedup();
    out
}

fn cache() -> &'static BTreeMap<(&'static str, u16), Paths> {
    static CACHE: OnceLock<BTreeMap<(&'static str, u16), Paths>> = OnceLock::new();
    CACHE.get_or_init(|| {
        kinds()
            .iter()
            .map(|k| ((k.name, k.version), paths_of(&(k.schema)())))
            .collect()
    })
}

fn with_paths<R>(kind: &KindSpec, f: impl FnOnce(&Paths) -> R) -> R {
    match cache().get(&(kind.name, kind.version)) {
        Some(p) => f(p),
        None => f(&paths_of(&(kind.schema)())),
    }
}

/// The content fields of `kind`'s payload, from its schema (computed once per kind).
pub fn content_paths(kind: &KindSpec) -> Vec<ContentPath> {
    with_paths(kind, |p| p.content.clone())
}

/// Removes every content field of `payload` unless `purposes` has training. A training-only
/// kind keeps its metadata part, like any other kind.
pub fn redact(kind: &KindSpec, payload: &mut Value, purposes: Purposes) {
    if purposes.has_training() {
        return;
    }
    with_paths(kind, |p| {
        for c in &p.content {
            remove_at(payload, &c.path);
        }
    });
}

fn remove_at(v: &mut Value, path: &[Seg]) {
    match path {
        [] => {}
        [Seg::Key(k)] => {
            if let Some(obj) = v.as_object_mut() {
                obj.remove(k);
            }
        }
        [Seg::Key(k), rest @ ..] => {
            if let Some(x) = v.get_mut(k.as_str()) {
                remove_at(x, rest);
            }
        }
        [Seg::Items, rest @ ..] => {
            for x in v.as_array_mut().into_iter().flatten() {
                remove_at(x, rest);
            }
        }
        [Seg::Values, rest @ ..] => {
            for x in v.as_object_mut().into_iter().flat_map(|m| m.values_mut()) {
                remove_at(x, rest);
            }
        }
    }
}

fn collect_at<'a>(v: &'a Value, path: &[Seg], out: &mut Vec<&'a Value>) {
    match path {
        [] => out.push(v),
        [Seg::Key(k), rest @ ..] => {
            if let Some(x) = v.get(k.as_str()) {
                collect_at(x, rest, out);
            }
        }
        [Seg::Items, rest @ ..] => {
            for x in v.as_array().into_iter().flatten() {
                collect_at(x, rest, out);
            }
        }
        [Seg::Values, rest @ ..] => {
            for x in v.as_object().into_iter().flat_map(|m| m.values()) {
                collect_at(x, rest, out);
            }
        }
    }
}

/// The blobs a serialised `payload` of `kind` references (a line read from disk): every
/// [`BlobRef`] at a blob path of its schema that decodes.
pub fn blob_refs(kind: &KindSpec, payload: &Value) -> Vec<BlobRef> {
    with_paths(kind, |p| {
        let mut found = Vec::new();
        for path in &p.blobs {
            collect_at(payload, path, &mut found);
        }
        found
            .into_iter()
            .filter_map(|v| BlobRef::deserialize(v).ok())
            .collect()
    })
}
