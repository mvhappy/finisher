//! The JSON Schema subset of command arguments (8B plan Decision 13), checked by our own pure
//! code: the manifest validator checks a command's `args` schema at install, and the host
//! checks a run's arguments with [`validate_args`] before the job is created.
//!
//! The subset is exactly: `type` (`object` at the top; `string`, `number`, `integer`,
//! `boolean`, `array`, `object` below), `properties`, `required`, `additionalProperties`
//! (`false`, required at every object), `enum`, `const`, `minimum`/`maximum`,
//! `minLength`/`maxLength`, `pattern`, `items` (required at every array),
//! `minItems`/`maxItems` and `description` (a message key). A keyword outside the subset, or
//! one that does not apply to the node's type, refuses the schema: there is no `$ref`, no
//! format, no composition, no default. `pattern` is compiled by `regex-automata`'s meta
//! engine (not the `regex` facade the plan names: it is not in the tree) without its lazy
//! DFA, so matching time is linear in the input and its memory fixed at compile time (no
//! backreferences, no lookaround). Shorthand classes and word boundaries (`\d \w \s \b`, their
//! negations, `\p`) are Unicode there, not ECMA-262, so they are refused: patterns spell
//! ASCII classes out (`[0-9]`). Each schema has a compile budget: at most
//! [`MAX_PATTERNS`] patterns, [`PATTERN_NFA_BYTES`] of automaton per pattern and
//! [`MAX_COMPILED_BYTES`] in all. A host compiles a schema once ([`CompiledArgs::new`]) and
//! reuses it for every run.
//! **Deviation from spec §18.1:** no `jsonschema` crate.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use regex_automata::meta::Regex;
use regex_automata::util::syntax;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};

mod args;

/// The deepest nesting of schema nodes accepted.
pub const MAX_DEPTH: usize = 16;
/// The longest `pattern` accepted, in bytes.
pub const MAX_PATTERN_BYTES: usize = 1024;
/// The most `pattern`s in one schema.
pub const MAX_PATTERNS: usize = 64;
/// The compiled automaton size cap of one `pattern`, in bytes.
pub const PATTERN_NFA_BYTES: usize = 64 * 1024;
/// The compiled size cap of all of one schema's patterns together, in bytes.
pub const MAX_COMPILED_BYTES: usize = 2 * 1024 * 1024;

const KEYWORDS: &[&str] = &[
    "type",
    "description",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "minLength",
    "maxLength",
    "pattern",
    "minimum",
    "maximum",
    "items",
    "minItems",
    "maxItems",
];
const COMMON: &[&str] = &["type", "description", "enum", "const"];

/// A command's argument schema, kept as written (it serialises unchanged). Parsing accepts
/// any JSON value so the validator can report a schema outside the subset with its manifest
/// path; [`SchemaSubset::check`] is that check.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaSubset(Value);

impl JsonSchema for SchemaSubset {
    fn schema_name() -> Cow<'static, str> {
        "SchemaSubset".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "A command's argument schema: the JSON Schema subset of 8B Decision 13 (an object schema with `additionalProperties: false` at every object).",
            "type": "object",
            "additionalProperties": true,
        })
    }
}

/// Why a schema is outside the subset: `at` is a JSON pointer into the schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SchemaError {
    pub at: String,
    /// `keyword`, `type`, `topType`, `additionalProperties`, `properties`, `required`,
    /// `items`, `bound`, `enum`, `const`, `pattern`, `description`, `depth` or `budget`.
    pub problem: String,
    pub message: String,
}

/// One argument that does not match: `path` is a JSON pointer into the arguments, `reason`
/// the failing keyword (`type`, `required`, `additionalProperties`, `enum`, `const`,
/// `minimum`, `maximum`, `minLength`, `maxLength`, `pattern`, `minItems`, `maxItems`), or
/// `schema` when the schema itself is outside the subset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgProblem {
    pub path: String,
    pub reason: String,
    pub message: String,
}

impl SchemaSubset {
    pub fn new(schema: Value) -> SchemaSubset {
        SchemaSubset(schema)
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    /// Whether the schema is inside the subset; the first problem otherwise. Compiles it:
    /// call it once (install) and keep [`CompiledArgs`] where arguments are checked.
    pub fn check(&self) -> Result<(), SchemaError> {
        CompiledArgs::new(self).map(|_| ())
    }

    /// The `description`s that are well-formed message keys, as (pointer into the schema,
    /// key), without compiling anything; a malformed one is [`SchemaSubset::check`]'s.
    pub fn descriptions(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        descriptions(&self.0, "", 0, &mut out);
        out
    }
}

/// Checks `args` against `schema`, reporting every problem, sorted by path.
/// Compiles `schema` each call: a host keeps a [`CompiledArgs`] instead.
pub fn validate_args(schema: &SchemaSubset, args: &Value) -> Result<(), Vec<ArgProblem>> {
    match CompiledArgs::new(schema) {
        Ok(c) => c.validate(args),
        Err(e) => Err(vec![ArgProblem {
            path: String::new(),
            reason: "schema".into(),
            message: format!(
                "the argument schema is outside the subset at {:?}: {}",
                e.at, e.message
            ),
        }]),
    }
}

/// A checked and compiled schema, reusable for every run of its command.
#[derive(Debug, Clone)]
pub struct CompiledArgs {
    root: Node,
}

/// What a schema's patterns have used of their budget.
#[derive(Default)]
struct Budget {
    patterns: usize,
    bytes: usize,
}

impl CompiledArgs {
    pub fn new(schema: &SchemaSubset) -> Result<CompiledArgs, SchemaError> {
        let root = node(&schema.0, "", 0, &mut Budget::default())?;
        if !matches!(root.kind, Kind::Object { .. }) {
            return Err(err(
                "/type",
                "topType",
                "the top of an argument schema is an object",
            ));
        }
        Ok(CompiledArgs { root })
    }

    pub fn validate(&self, args: &Value) -> Result<(), Vec<ArgProblem>> {
        let mut out = Vec::new();
        self.root.validate(args, "", &mut out);
        out.sort_by(|a, b| a.path.cmp(&b.path));
        if out.is_empty() { Ok(()) } else { Err(out) }
    }
}

#[derive(Debug, Clone)]
struct Node {
    kind: Kind,
    one_of: Option<Vec<Value>>,
    konst: Option<Value>,
}

#[derive(Debug, Clone)]
enum Kind {
    Object {
        properties: BTreeMap<String, Node>,
        required: Vec<String>,
    },
    String {
        min: Option<u64>,
        max: Option<u64>,
        pattern: Option<(String, Regex)>,
    },
    Number {
        integer: bool,
        min: Option<Number>,
        max: Option<Number>,
    },
    Boolean,
    Array {
        items: Box<Node>,
        min: Option<u64>,
        max: Option<u64>,
    },
}

fn err(at: &str, problem: &str, message: impl Into<String>) -> SchemaError {
    SchemaError {
        at: at.into(),
        problem: problem.into(),
        message: message.into(),
    }
}

/// RFC 6901 escaping of one reference token.
fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn sorted_keys(o: &Map<String, Value>) -> Vec<&String> {
    let mut keys: Vec<&String> = o.keys().collect();
    keys.sort();
    keys
}

fn node(v: &Value, at: &str, depth: usize, b: &mut Budget) -> Result<Node, SchemaError> {
    let Value::Object(o) = v else {
        return Err(err(at, "type", "a schema node is an object"));
    };
    if depth >= MAX_DEPTH {
        return Err(err(
            at,
            "depth",
            format!("schemas nest at most {MAX_DEPTH} deep"),
        ));
    }
    let keys = sorted_keys(o);
    if let Some(k) = keys.iter().find(|k| !KEYWORDS.contains(&k.as_str())) {
        return Err(err(
            &format!("{at}/{}", escape(k)),
            "keyword",
            format!("`{k}` is not in the argument schema subset"),
        ));
    }
    let Some(t) = o.get("type") else {
        return Err(err(at, "type", "every schema node names its type"));
    };
    let ty = t.as_str().unwrap_or("");
    let own: &[&str] = match ty {
        "object" => &["properties", "required", "additionalProperties"],
        "string" => &["minLength", "maxLength", "pattern"],
        "number" | "integer" => &["minimum", "maximum"],
        "boolean" => &[],
        "array" => &["items", "minItems", "maxItems"],
        _ => {
            return Err(err(
                &format!("{at}/type"),
                "type",
                format!("type {t} is not one of object, string, number, integer, boolean, array"),
            ));
        }
    };
    if let Some(k) = keys
        .iter()
        .find(|k| !COMMON.contains(&k.as_str()) && !own.contains(&k.as_str()))
    {
        return Err(err(
            &format!("{at}/{}", escape(k)),
            "keyword",
            format!("`{k}` does not apply to type {ty}"),
        ));
    }
    if let Some(d) = o.get("description")
        && !d.as_str().is_some_and(crate::validate::is_message_key)
    {
        return Err(err(
            &format!("{at}/description"),
            "description",
            "a description is a message key",
        ));
    }
    let kind = match ty {
        "object" => object(o, at, depth, b)?,
        "string" => {
            let (min, max) = lengths(o, at, "minLength", "maxLength")?;
            let pattern = match o.get("pattern") {
                None => None,
                Some(p) => Some(pattern(p, &format!("{at}/pattern"), b)?),
            };
            Kind::String { min, max, pattern }
        }
        "number" | "integer" => {
            let num = |k: &str| match o.get(k) {
                None => Ok(None),
                Some(Value::Number(n)) => Ok(Some(n.clone())),
                Some(_) => Err(err(
                    &format!("{at}/{k}"),
                    "bound",
                    format!("{k} is a number"),
                )),
            };
            let (min, max) = (num("minimum")?, num("maximum")?);
            if let (Some(a), Some(b)) = (&min, &max)
                && args::num_cmp(a, b).is_gt()
            {
                return Err(err(at, "bound", "minimum is above maximum"));
            }
            Kind::Number {
                integer: ty == "integer",
                min,
                max,
            }
        }
        "boolean" => Kind::Boolean,
        _ => {
            let (min, max) = lengths(o, at, "minItems", "maxItems")?;
            let Some(items) = o.get("items") else {
                return Err(err(at, "items", "an array schema names its items"));
            };
            let items = Box::new(node(items, &format!("{at}/items"), depth + 1, b)?);
            Kind::Array { items, min, max }
        }
    };
    let mut n = Node {
        kind,
        one_of: None,
        konst: None,
    };
    if let Some(e) = o.get("enum") {
        let Value::Array(values) = e else {
            return Err(err(&format!("{at}/enum"), "enum", "enum is an array"));
        };
        if values.is_empty() {
            return Err(err(
                &format!("{at}/enum"),
                "enum",
                "enum lists at least one value",
            ));
        }
        for (i, v) in values.iter().enumerate() {
            if !n.kind.admits(v) {
                return Err(err(
                    &format!("{at}/enum/{i}"),
                    "enum",
                    format!("{v} is not of type {ty}"),
                ));
            }
        }
        n.one_of = Some(values.clone());
    }
    if let Some(c) = o.get("const") {
        if !n.kind.admits(c) {
            return Err(err(
                &format!("{at}/const"),
                "const",
                format!("{c} is not of type {ty}"),
            ));
        }
        if let Some(values) = &n.one_of
            && !values.iter().any(|v| args::json_eq(v, c))
        {
            return Err(err(
                &format!("{at}/const"),
                "const",
                format!("{c} is not among the enum's values"),
            ));
        }
        n.konst = Some(c.clone());
    }
    Ok(n)
}

fn object(
    o: &Map<String, Value>,
    at: &str,
    depth: usize,
    b: &mut Budget,
) -> Result<Kind, SchemaError> {
    match o.get("additionalProperties") {
        None => {
            return Err(err(
                at,
                "additionalProperties",
                "every object schema says additionalProperties: false",
            ));
        }
        Some(Value::Bool(false)) => {}
        Some(_) => {
            return Err(err(
                &format!("{at}/additionalProperties"),
                "additionalProperties",
                "additionalProperties is false",
            ));
        }
    }
    let mut properties = BTreeMap::new();
    match o.get("properties") {
        None => {}
        Some(Value::Object(p)) => {
            for k in sorted_keys(p) {
                let child = node(
                    &p[k],
                    &format!("{at}/properties/{}", escape(k)),
                    depth + 1,
                    b,
                )?;
                properties.insert(k.clone(), child);
            }
        }
        Some(_) => {
            return Err(err(
                &format!("{at}/properties"),
                "properties",
                "properties is an object",
            ));
        }
    }
    let mut required = Vec::new();
    match o.get("required") {
        None => {}
        Some(Value::Array(names)) => {
            let mut seen = BTreeSet::new();
            for (i, name) in names.iter().enumerate() {
                let here = format!("{at}/required/{i}");
                let Some(name) = name.as_str() else {
                    return Err(err(&here, "required", "required lists property names"));
                };
                if !properties.contains_key(name) {
                    return Err(err(
                        &here,
                        "required",
                        format!("{name:?} is not a property"),
                    ));
                }
                if !seen.insert(name) {
                    return Err(err(&here, "required", format!("{name:?} is listed twice")));
                }
                required.push(name.to_string());
            }
        }
        Some(_) => {
            return Err(err(
                &format!("{at}/required"),
                "required",
                "required is an array",
            ));
        }
    }
    Ok(Kind::Object {
        properties,
        required,
    })
}

/// A pair of non-negative integer bounds, the first not above the second.
fn lengths(
    o: &Map<String, Value>,
    at: &str,
    lo: &str,
    hi: &str,
) -> Result<(Option<u64>, Option<u64>), SchemaError> {
    let get = |k: &str| match o.get(k) {
        None => Ok(None),
        Some(v) => v.as_u64().map(Some).ok_or_else(|| {
            err(
                &format!("{at}/{k}"),
                "bound",
                format!("{k} is a non-negative integer"),
            )
        }),
    };
    let (min, max) = (get(lo)?, get(hi)?);
    if let (Some(a), Some(b)) = (min, max)
        && a > b
    {
        return Err(err(at, "bound", format!("{lo} is above {hi}")));
    }
    Ok((min, max))
}

/// A shorthand class or assertion that is Unicode in regex-automata but ASCII in ECMA-262,
/// or a Unicode property: the first one in `p`, outside an escaped backslash.
fn unicode_shorthand(p: &str) -> Option<char> {
    let mut chars = p.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(e @ ('d' | 'D' | 'w' | 'W' | 's' | 'S' | 'b' | 'B' | 'p' | 'P')) =
                chars.next()
        {
            return Some(e);
        }
    }
    None
}

fn pattern(p: &Value, at: &str, b: &mut Budget) -> Result<(String, Regex), SchemaError> {
    let Some(s) = p.as_str() else {
        return Err(err(at, "pattern", "pattern is a string"));
    };
    if s.len() > MAX_PATTERN_BYTES {
        return Err(err(
            at,
            "pattern",
            format!("a pattern is at most {MAX_PATTERN_BYTES} bytes"),
        ));
    }
    if let Some(e) = unicode_shorthand(s) {
        return Err(err(
            at,
            "pattern",
            format!(
                "\\{e} is Unicode here, not ECMA-262: spell the class out in ASCII, such as [0-9], [A-Za-z0-9_] or [ \\t\\n\\r]"
            ),
        ));
    }
    if b.patterns >= MAX_PATTERNS {
        return Err(err(
            at,
            "budget",
            format!("a schema has at most {MAX_PATTERNS} patterns"),
        ));
    }
    b.patterns += 1;
    let re = Regex::builder()
        .syntax(syntax::Config::new().nest_limit(64))
        .configure(
            Regex::config()
                .nfa_size_limit(Some(PATTERN_NFA_BYTES))
                .onepass_size_limit(Some(0))
                .hybrid(false),
        )
        .build(s)
        .map_err(|e| {
            err(
                at,
                "pattern",
                format!("{s:?} is not a supported pattern: {e}"),
            )
        })?;
    b.bytes += re.memory_usage();
    if b.bytes > MAX_COMPILED_BYTES {
        return Err(err(
            at,
            "budget",
            format!("a schema's patterns compile to at most {MAX_COMPILED_BYTES} bytes in all"),
        ));
    }
    Ok((s.to_string(), re))
}

fn descriptions(v: &Value, at: &str, depth: usize, out: &mut Vec<(String, String)>) {
    let Value::Object(o) = v else { return };
    if depth >= MAX_DEPTH {
        return;
    }
    if let Some(Value::String(d)) = o.get("description")
        && crate::validate::is_message_key(d)
    {
        out.push((format!("{at}/description"), d.clone()));
    }
    if let Some(Value::Object(p)) = o.get("properties") {
        for k in sorted_keys(p) {
            descriptions(
                &p[k],
                &format!("{at}/properties/{}", escape(k)),
                depth + 1,
                out,
            );
        }
    }
    if let Some(items) = o.get("items") {
        descriptions(items, &format!("{at}/items"), depth + 1, out);
    }
}
