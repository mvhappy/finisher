//! The config keys the fake knows, typed as 0.167.0 types them (7E Task 4 review: each message
//! below was seen on the real binary). A wrongly typed value stops the start whether or not
//! `--strict-config` is given; a key the CLI does not know stops it only under
//! `--strict-config`. Keys outside this table fall back to the shape of `data/config.json`
//! (known keys, no type check).

use serde_json::Value;

#[derive(Clone, Copy)]
pub enum Ty {
    Any,
    Str,
    Bool,
    Usize,
    Enum(&'static [&'static str]),
    /// `AskForApproval`: a mode name, or a `{granular = {...}}` table (its fields unchecked).
    Approval,
    StrList,
    StrMap,
    /// A struct: its name (for "expected struct X"), its fields, whether other keys are taken.
    Struct(&'static str, &'static [(&'static str, Ty)], bool),
    /// `mcp_servers`: a map of `RawMcpServerConfig`.
    Servers,
    /// `features`: a map of known feature names to booleans.
    Features,
}

use Ty::*;

const APPROVAL: &[&str] = &["untrusted", "on-failure", "on-request", "granular", "never"];

const ROOT: &[(&str, Ty)] = &[
    ("model", Str),
    ("review_model", Str),
    ("model_provider", Str),
    ("approval_policy", Approval),
    (
        "approvals_reviewer",
        Enum(&["user", "auto_review", "guardian_subagent"]),
    ),
    (
        "sandbox_mode",
        Enum(&["read-only", "workspace-write", "danger-full-access"]),
    ),
    (
        "web_search",
        Enum(&["disabled", "cached", "indexed", "live"]),
    ),
    (
        "cli_auth_credentials_store",
        Enum(&["file", "keyring", "auto", "ephemeral"]),
    ),
    ("project_doc_max_bytes", Usize),
    ("project_doc_fallback_filenames", StrList),
    ("project_root_markers", StrList),
    ("check_for_update_on_startup", Bool),
    (
        "analytics",
        Struct("AnalyticsConfigToml", &[("enabled", Bool)], false),
    ),
    (
        "history",
        Struct(
            "History",
            &[
                ("persistence", Enum(&["save-all", "none"])),
                ("max_bytes", Usize),
            ],
            false,
        ),
    ),
    (
        "shell_environment_policy",
        Struct(
            "ShellEnvironmentPolicyTomlRaw",
            &[
                ("inherit", Enum(&["core", "all", "none"])),
                ("set", StrMap),
                ("ignore_default_excludes", Bool),
                ("exclude", StrList),
                ("include_only", StrList),
                ("filters", Any),
                ("experimental_use_profile", Bool),
            ],
            false,
        ),
    ),
    (
        "skills",
        Struct(
            "SkillsConfig",
            &[
                (
                    "bundled",
                    Struct("BundledSkillsConfig", &[("enabled", Bool)], false),
                ),
                ("include_instructions", Bool),
                ("config", Any),
            ],
            false,
        ),
    ),
    ("tools", Struct("ToolsToml", &[("web_search", Any)], false)),
    // UNOBSERVED(7E Task 0 Step 3): role tables under `agents` (`AgentRoleToml`) are taken as
    // they are; 0.167.0 types them.
    ("agents", Struct("AgentsToml", &[("enabled", Bool)], true)),
    ("mcp_servers", Servers),
    ("features", Features),
];

/// Features whose value is a table with `enabled` (their `--disable` origin key names it).
pub const TABLE_FEATURES: &[&str] = &["sleep_tool"];

fn field(fields: &'static [(&'static str, Ty)], k: &str) -> Option<Ty> {
    fields.iter().find(|(n, _)| *n == k).map(|(_, t)| *t)
}

/// `--strict-config`: the key's prefix up to its first unknown segment, `None` when known.
pub fn unknown_field(
    path: &[&str],
    base: &Value,
    known_feature: impl Fn(&str) -> bool,
) -> Option<String> {
    let prefix = |i: usize| Some(path[..=i].join("."));
    let Some(top) = field(ROOT, path[0]) else {
        // Outside the typed table: the shape of `config/read`'s answer, where a null, a scalar
        // or an empty table is open.
        let mut node = base;
        for (i, seg) in path.iter().enumerate() {
            match node {
                Value::Object(m) if m.is_empty() => return None,
                Value::Object(m) => match m.get(*seg) {
                    Some(n) => node = n,
                    None => return prefix(i),
                },
                _ => return None,
            }
        }
        return None;
    };
    let mut ty = top;
    for (i, seg) in path.iter().enumerate().skip(1) {
        match ty {
            Struct(_, fields, open) => match field(fields, seg) {
                Some(t) => ty = t,
                None if open => return None,
                None => return prefix(i),
            },
            Features if known_feature(seg) => return None,
            Features => return prefix(i),
            // A server's own keys are judged by its transport, not by name (0.167.0 answers
            // `mcp_servers.x.bogus` with "invalid transport", strict or not).
            _ => return None,
        }
    }
    None
}

/// serde's words for a value it did not expect.
fn unexpected(v: &Value) -> String {
    match v {
        Value::String(s) => format!("string {s:?}"),
        Value::Bool(b) => format!("boolean `{b}`"),
        Value::Number(n) if n.is_f64() => format!("floating point `{n}`"),
        Value::Number(n) => format!("integer `{n}`"),
        Value::Array(_) => "sequence".into(),
        Value::Object(_) => "map".into(),
        Value::Null => "unit value".into(),
    }
}

/// A type error: serde's message and the dotted path it names.
pub type TypeError = (String, String);

fn invalid(v: &Value, want: &str, path: &str) -> Result<(), TypeError> {
    Err((
        format!("invalid type: {}, expected {want}", unexpected(v)),
        path.into(),
    ))
}

fn variants(list: &[&str]) -> String {
    let q: Vec<String> = list.iter().map(|v| format!("`{v}`")).collect();
    match q.len() {
        1 => q[0].clone(),
        2 => format!("{} or {}", q[0], q[1]),
        _ => format!("one of {}", q.join(", ")),
    }
}

pub fn unknown_variant(v: &str, list: &[&str]) -> String {
    format!("unknown variant `{v}`, expected {}", variants(list))
}

fn sub(path: &str, k: &str) -> String {
    if path.is_empty() {
        k.into()
    } else {
        format!("{path}.{k}")
    }
}

fn check(ty: Ty, v: &Value, path: &str) -> Result<(), TypeError> {
    match (ty, v) {
        (Any, _) | (Str, Value::String(_)) | (Bool, Value::Bool(_)) => Ok(()),
        (Str, _) => invalid(v, "a string", path),
        (Bool, _) => invalid(v, "a boolean", path),
        (Usize, Value::Number(n)) if n.is_u64() => Ok(()),
        (Usize, Value::Number(n)) if n.is_i64() => Err((
            format!("invalid value: integer `{n}`, expected usize"),
            path.into(),
        )),
        (Usize, _) => invalid(v, "usize", path),
        (Approval, Value::Object(_)) => Ok(()),
        (Approval, _) => check(Enum(APPROVAL), v, path),
        (Enum(list), Value::String(s)) if list.contains(&s.as_str()) => Ok(()),
        (Enum(list), Value::String(s)) => Err((unknown_variant(s, list), path.into())),
        // An externally tagged enum read from a table takes its key as the variant.
        (Enum(list), Value::Object(m)) => match m.keys().next() {
            Some(k) if !list.contains(&k.as_str()) => Err((unknown_variant(k, list), path.into())),
            _ => Ok(()),
        },
        // UNOBSERVED(7E Task 0 Step 3): the message for a number or boolean given to an enum
        // (0.167.0 said "invalid type: unit variant, expected string only" for `web_search=1`).
        (Enum(_), _) => invalid(v, "a string", path),
        (StrList, Value::Array(a)) => a.iter().try_for_each(|x| check(Str, x, path)),
        (StrList, _) => invalid(v, "a sequence", path),
        (StrMap, Value::Object(m)) => m.iter().try_for_each(|(k, x)| check(Str, x, &sub(path, k))),
        (StrMap, _) => invalid(v, "a map", path),
        (Struct(_, fields, _), Value::Object(m)) => {
            m.iter().try_for_each(|(k, x)| match field(fields, k) {
                Some(t) => check(t, x, &sub(path, k)),
                None => Ok(()),
            })
        }
        (Struct(name, _, _), _) => invalid(v, &format!("struct {name}"), path),
        (Servers, Value::Object(m)) => m.iter().try_for_each(|(k, x)| server(x, &sub(path, k))),
        (Servers, _) => invalid(v, "a map", path),
        (Features, Value::Object(m)) => m.iter().try_for_each(|(k, x)| match x {
            Value::Object(t) if TABLE_FEATURES.contains(&k.as_str()) => match t.get("enabled") {
                Some(e) => check(Bool, e, &sub(&sub(path, k), "enabled")),
                None => Ok(()),
            },
            // serde names the map, not the entry (as 0.167.0 prints it).
            _ => check(Bool, x, path),
        }),
        (Features, _) => invalid(v, "struct FeaturesToml", path),
    }
}

fn server(v: &Value, path: &str) -> Result<(), TypeError> {
    let Value::Object(m) = v else {
        return invalid(v, "struct RawMcpServerConfig", path);
    };
    if !m.contains_key("command") && !m.contains_key("url") {
        return Err(("invalid transport".into(), path.into()));
    }
    for (k, t) in [
        ("command", Str),
        ("url", Str),
        ("args", StrList),
        ("env", StrMap),
        ("enabled", Bool),
    ] {
        if let Some(x) = m.get(k) {
            check(t, x, &sub(path, k))?;
        }
    }
    Ok(())
}

/// Types the known keys of a layer (the merged `-c` values, or a `config.toml`).
pub fn check_layer(layer: &Value) -> Result<(), TypeError> {
    let Value::Object(m) = layer else {
        return Ok(());
    };
    m.iter().try_for_each(|(k, v)| match field(ROOT, k) {
        Some(t) => check(t, v, k),
        None => Ok(()),
    })
}
