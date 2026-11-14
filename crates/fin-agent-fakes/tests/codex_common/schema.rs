//! A small JSON Schema (draft-07 subset) validator over the committed Codex schema bundle, so
//! every line `fake-codex` writes is checked against `vendor/codex/<pin>/schema/` without a new
//! dependency. Supported: `$ref` (JSON pointers into the bundle), `type`, `enum`, `const`,
//! `properties`, `required`, `additionalProperties`, `items`, `oneOf`, `anyOf`, `allOf`, `not`,
//! `minimum`, `maximum` (`format` and the annotation keywords are ignored). Anything else in a
//! schema is an error, so a re-pin that uses a new keyword fails loudly instead of passing.

use std::path::PathBuf;

use serde_json::Value;

pub const PIN: &str = "0.167.0";

const KNOWN: &[&str] = &[
    "$ref",
    "$schema",
    "type",
    "enum",
    "const",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "oneOf",
    "anyOf",
    "allOf",
    "not",
    "minimum",
    "maximum",
    "minLength",
    "format",
    "default",
    "description",
    "title",
    "definitions",
    "deprecated",
    "writeOnly",
    "readOnly",
];

pub struct Schemas {
    root: Value,
    /// `method` of a client request -> the name of its response definition.
    responses: Vec<(String, String)>,
}

impl Schemas {
    pub fn load() -> Schemas {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../vendor/codex/{PIN}/schema/codex_app_server_protocol.schemas.json"
        ));
        let root: Value =
            serde_json::from_str(&std::fs::read_to_string(&p).expect("schema bundle"))
                .expect("schema bundle is JSON");
        let mut responses = vec![];
        for v in root["definitions"]["ClientRequest"]["oneOf"]
            .as_array()
            .unwrap()
        {
            let method = v["properties"]["method"]["enum"][0].as_str().unwrap();
            let params = v["properties"]["params"]["$ref"].as_str().unwrap_or("");
            // `Params` -> `Response`; a method without params is named from the method itself
            // (`configRequirements/read` -> `ConfigRequirementsReadResponse`).
            let name = match params
                .rsplit('/')
                .next()
                .and_then(|n| n.strip_suffix("Params"))
            {
                Some(n) => n.to_string(),
                None => method
                    .split(['/', '_'])
                    .map(|w| w[..1].to_uppercase() + &w[1..])
                    .collect(),
            };
            responses.push((method.to_string(), format!("{name}Response")));
        }
        Schemas { root, responses }
    }

    fn named(&self, name: &str) -> Option<&Value> {
        let d = &self.root["definitions"];
        d["v2"].get(name).or_else(|| d.get(name))
    }

    /// Validates one line the fake wrote. `request` is the method of the request a response
    /// answers (the test knows it); notifications and server requests carry their own.
    pub fn check_line(&self, line: &Value, request: Option<&str>) -> Result<(), String> {
        let has = |k: &str| line.get(k).is_some();
        let (what, schema): (String, &Value) = if has("method") && has("id") {
            (
                "ServerRequest".into(),
                &self.root["definitions"]["ServerRequest"],
            )
        } else if has("method") {
            (
                "ServerNotification".into(),
                &self.root["definitions"]["ServerNotification"],
            )
        } else if has("error") {
            (
                "JSONRPCError".into(),
                &self.root["definitions"]["JSONRPCError"],
            )
        } else if has("result") {
            let m = request.ok_or("a response to a request the test did not record")?;
            let name = self
                .responses
                .iter()
                .find(|(x, _)| x == m)
                .map(|(_, n)| n.clone())
                .ok_or(format!("no response definition for {m}"))?;
            let s = self.named(&name).ok_or(format!("no definition {name}"))?;
            return self
                .check(s, &line["result"], &name)
                .map_err(|e| format!("{name}: {e}"));
        } else {
            return Err("neither a request, a notification nor a response".into());
        };
        self.check(schema, line, &what)
            .map_err(|e| format!("{what}: {e}"))
    }

    pub fn check(&self, schema: &Value, v: &Value, at: &str) -> Result<(), String> {
        let o = match schema {
            Value::Bool(true) => return Ok(()),
            Value::Bool(false) => return Err(format!("{at}: nothing is allowed here")),
            Value::Object(o) => o,
            _ => return Err(format!("{at}: schema is not an object")),
        };
        for k in o.keys() {
            if !KNOWN.contains(&k.as_str()) {
                return Err(format!("{at}: the validator does not know the keyword {k}"));
            }
        }
        if let Some(r) = o.get("$ref") {
            let r = r.as_str().ok_or("$ref is not a string")?;
            let target = self
                .root
                .pointer(r.strip_prefix('#').ok_or("only local $refs")?)
                .ok_or(format!("{at}: unresolved {r}"))?;
            self.check(target, v, at)?;
        }
        if let Some(t) = o.get("type") {
            let names: Vec<&str> = match t {
                Value::String(s) => vec![s.as_str()],
                Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
                _ => vec![],
            };
            if !names.iter().any(|n| type_is(n, v)) {
                return Err(format!("{at}: expected {names:?}, got {}", short(v)));
            }
        }
        if let Some(e) = o.get("enum")
            && !e.as_array().is_some_and(|a| a.contains(v))
        {
            return Err(format!("{at}: {} is not one of {e}", short(v)));
        }
        if let Some(c) = o.get("const")
            && c != v
        {
            return Err(format!("{at}: expected {c}, got {}", short(v)));
        }
        if let (Some(m), Some(s)) = (o.get("minLength").and_then(Value::as_u64), v.as_str())
            && (s.chars().count() as u64) < m
        {
            return Err(format!("{at}: shorter than minLength {m}"));
        }
        if let (Some(m), Some(n)) = (o.get("minimum").and_then(Value::as_f64), v.as_f64())
            && n < m
        {
            return Err(format!("{at}: {n} < minimum {m}"));
        }
        if let (Some(m), Some(n)) = (o.get("maximum").and_then(Value::as_f64), v.as_f64())
            && n > m
        {
            return Err(format!("{at}: {n} > maximum {m}"));
        }
        if let Value::Object(m) = v {
            for r in o
                .get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let k = r.as_str().unwrap_or("");
                if !m.contains_key(k) {
                    return Err(format!("{at}: missing required `{k}`"));
                }
            }
            let props = o.get("properties").and_then(Value::as_object);
            for (k, val) in m {
                match props.and_then(|p| p.get(k)) {
                    Some(s) => self.check(s, val, &format!("{at}.{k}"))?,
                    None => match o.get("additionalProperties") {
                        Some(Value::Bool(false)) => return Err(format!("{at}: unexpected `{k}`")),
                        Some(s) => self.check(s, val, &format!("{at}.{k}"))?,
                        None => {}
                    },
                }
            }
        }
        if let (Value::Array(a), Some(s)) = (v, o.get("items")) {
            for (i, x) in a.iter().enumerate() {
                self.check(s, x, &format!("{at}[{i}]"))?;
            }
        }
        for key in ["allOf"] {
            for s in o.get(key).and_then(Value::as_array).into_iter().flatten() {
                self.check(s, v, at)?;
            }
        }
        if let Some(any) = o.get("anyOf").and_then(Value::as_array) {
            let errs: Vec<String> = any
                .iter()
                .filter_map(|s| self.check(s, v, at).err())
                .collect();
            if errs.len() == any.len() {
                return Err(format!(
                    "{at}: no anyOf branch matches ({})",
                    errs.join(" | ")
                ));
            }
        }
        if let Some(one) = o.get("oneOf").and_then(Value::as_array) {
            let errs: Vec<String> = one
                .iter()
                .filter_map(|s| self.check(s, v, at).err())
                .collect();
            match one.len() - errs.len() {
                1 => {}
                0 => {
                    return Err(format!(
                        "{at}: no oneOf branch matches ({})",
                        errs.join(" | ")
                    ));
                }
                _ => return Err(format!("{at}: several oneOf branches match {}", short(v))),
            }
        }
        if let Some(n) = o.get("not")
            && self.check(n, v, at).is_ok()
        {
            return Err(format!("{at}: matches `not`"));
        }
        Ok(())
    }
}

fn type_is(name: &str, v: &Value) -> bool {
    match name {
        "string" => v.is_string(),
        "boolean" => v.is_boolean(),
        "null" => v.is_null(),
        "array" => v.is_array(),
        "object" => v.is_object(),
        "number" => v.is_number(),
        "integer" => v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0),
        _ => false,
    }
}

fn short(v: &Value) -> String {
    let s: String = v.to_string().chars().take(120).collect();
    s
}
