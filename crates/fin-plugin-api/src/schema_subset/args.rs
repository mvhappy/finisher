//! Checking arguments against a compiled schema: every problem is reported.

use std::cmp::Ordering;

use serde_json::{Number, Value};

use super::{ArgProblem, Kind, Node, escape, sorted_keys};

/// `n` exactly, when it is an integer of `i64` or `u64` range.
fn as_int(n: &Number) -> Option<i128> {
    n.as_i64()
        .map(i128::from)
        .or_else(|| n.as_u64().map(i128::from))
}

/// Numbers by value: exactly when both are integers, as `f64` otherwise.
pub(super) fn num_cmp(a: &Number, b: &Number) -> Ordering {
    match (as_int(a), as_int(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => {
            let (x, y) = (
                a.as_f64().unwrap_or(f64::NAN),
                b.as_f64().unwrap_or(f64::NAN),
            );
            x.partial_cmp(&y).unwrap_or(Ordering::Equal)
        }
    }
}

/// Equality of JSON values with numbers compared by value (`2` equals `2.0`).
pub(super) fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => num_cmp(x, y).is_eq(),
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(p, q)| json_eq(p, q))
        }
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len()
                && x.iter()
                    .all(|(k, p)| y.get(k).is_some_and(|q| json_eq(p, q)))
        }
        _ => a == b,
    }
}

fn is_integer(v: &Value) -> bool {
    v.is_i64()
        || v.is_u64()
        || v.as_f64()
            .is_some_and(|f| f.is_finite() && f.fract() == 0.0)
}

impl Kind {
    /// Whether `v` has this node's JSON type.
    pub(super) fn admits(&self, v: &Value) -> bool {
        match self {
            Kind::Object { .. } => v.is_object(),
            Kind::String { .. } => v.is_string(),
            Kind::Number { integer, .. } => v.is_number() && (!integer || is_integer(v)),
            Kind::Boolean => v.is_boolean(),
            Kind::Array { .. } => v.is_array(),
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Kind::Object { .. } => "an object",
            Kind::String { .. } => "a string",
            Kind::Number { integer: true, .. } => "an integer",
            Kind::Number { .. } => "a number",
            Kind::Boolean => "a boolean",
            Kind::Array { .. } => "an array",
        }
    }
}

fn push(out: &mut Vec<ArgProblem>, path: &str, reason: &str, message: String) {
    out.push(ArgProblem {
        path: path.into(),
        reason: reason.into(),
        message,
    });
}

/// `n` against optional bounds, as `(reason, message)` problems.
fn bounds<T: PartialOrd + std::fmt::Display + Copy>(
    n: T,
    min: Option<T>,
    max: Option<T>,
    names: (&str, &str),
    unit: &str,
) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if let Some(m) = min
        && n < m
    {
        v.push((names.0.to_string(), format!("at least {m}{unit}")));
    }
    if let Some(m) = max
        && n > m
    {
        v.push((names.1.to_string(), format!("at most {m}{unit}")));
    }
    v
}

impl Node {
    pub(super) fn validate(&self, v: &Value, path: &str, out: &mut Vec<ArgProblem>) {
        if !self.kind.admits(v) {
            push(out, path, "type", format!("expected {}", self.kind.name()));
            return;
        }
        if let Some(values) = &self.one_of
            && !values.iter().any(|e| json_eq(e, v))
        {
            push(
                out,
                path,
                "enum",
                format!("{v} is not one of the listed values"),
            );
        }
        if let Some(c) = &self.konst
            && !json_eq(c, v)
        {
            push(out, path, "const", format!("expected {c}"));
        }
        let found = match (&self.kind, v) {
            (
                Kind::Object {
                    properties,
                    required,
                },
                Value::Object(o),
            ) => {
                for name in required.iter().filter(|n| !o.contains_key(*n)) {
                    push(
                        out,
                        path,
                        "required",
                        format!("missing required argument {name:?}"),
                    );
                }
                for k in sorted_keys(o) {
                    let here = format!("{path}/{}", escape(k));
                    match properties.get(k) {
                        Some(child) => child.validate(&o[k], &here, out),
                        None => push(
                            out,
                            &here,
                            "additionalProperties",
                            format!("unknown argument {k:?}"),
                        ),
                    }
                }
                Vec::new()
            }
            (Kind::String { min, max, pattern }, Value::String(s)) => {
                let n = s.chars().count() as u64;
                let mut found = bounds(n, *min, *max, ("minLength", "maxLength"), " characters");
                if let Some((src, re)) = pattern
                    && !re.is_match(s.as_str())
                {
                    found.push(("pattern".into(), format!("does not match {src:?}")));
                }
                found
            }
            (Kind::Number { min, max, .. }, Value::Number(n)) => {
                let mut found = Vec::new();
                if let Some(m) = min
                    && num_cmp(n, m).is_lt()
                {
                    found.push(("minimum".to_string(), format!("at least {m}")));
                }
                if let Some(m) = max
                    && num_cmp(n, m).is_gt()
                {
                    found.push(("maximum".to_string(), format!("at most {m}")));
                }
                found
            }
            (Kind::Array { items, min, max }, Value::Array(a)) => {
                for (i, item) in a.iter().enumerate() {
                    items.validate(item, &format!("{path}/{i}"), out);
                }
                bounds(
                    a.len() as u64,
                    *min,
                    *max,
                    ("minItems", "maxItems"),
                    " items",
                )
            }
            _ => Vec::new(),
        };
        for (reason, message) in found {
            push(out, path, &reason, message);
        }
    }
}
