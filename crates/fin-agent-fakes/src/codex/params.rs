//! Request parameters checked as 0.167.0's serde types check them (7E Task 4 review: each
//! message below was seen on the real binary unless marked UNOBSERVED). A check returns the
//! error message of a `-32600` answer.

use serde_json::Value;

use super::config_check::unknown_variant;

pub const SANDBOX: &[&str] = &["read-only", "workspace-write", "danger-full-access"];
/// `AskForApproval` over the protocol (no `on-failure`, unlike the config key).
const APPROVAL: &[&str] = &["untrusted", "on-request", "granular", "never"];
const REVIEWER: &[&str] = &["user", "auto_review", "guardian_subagent"];
const MULTI_AGENT: &[&str] = &["none", "custom", "explicitRequestOnly", "proactive"];
const INPUT_TYPES: &[&str] = &[
    "text",
    "image",
    "localImage",
    "audio",
    "localAudio",
    "skill",
    "mention",
];

type Check = Result<(), String>;

fn bad(msg: String) -> Check {
    Err(format!("Invalid request: {msg}"))
}

fn unexpected(v: &Value) -> String {
    match v {
        Value::String(s) => format!("string {s:?}"),
        Value::Bool(b) => format!("boolean `{b}`"),
        Value::Number(n) if n.is_f64() => format!("floating point `{n}`"),
        Value::Number(n) => format!("integer `{n}`"),
        Value::Array(_) => "sequence".into(),
        Value::Object(_) => "map".into(),
        Value::Null => "null".into(),
    }
}

fn invalid(v: &Value, want: &str) -> Check {
    bad(format!("invalid type: {}, expected {want}", unexpected(v)))
}

/// A present, non-null field must have this JSON type.
fn opt_str(p: &Value, k: &str) -> Check {
    match &p[k] {
        Value::Null | Value::String(_) => Ok(()),
        v => invalid(v, "a string"),
    }
}

fn opt_bool(p: &Value, k: &str) -> Check {
    match &p[k] {
        Value::Null | Value::Bool(_) => Ok(()),
        v => invalid(v, "a boolean"),
    }
}

fn opt_enum(p: &Value, k: &str, list: &[&str], tables: bool) -> Check {
    match &p[k] {
        Value::Null => Ok(()),
        Value::String(s) if list.contains(&s.as_str()) => Ok(()),
        Value::String(s) => bad(unknown_variant(s, list)),
        Value::Object(_) if tables => Ok(()),
        v => invalid(v, "a string"),
    }
}

fn required<'a>(p: &'a Value, k: &str) -> Result<&'a Value, String> {
    match p.get(k) {
        Some(v) if !v.is_null() => Ok(v),
        _ => Err(format!("Invalid request: missing field `{k}`")),
    }
}

fn required_str<'a>(p: &'a Value, k: &str) -> Result<&'a str, String> {
    let v = required(p, k)?;
    v.as_str().ok_or_else(|| {
        format!(
            "Invalid request: invalid type: {}, expected a string",
            unexpected(v)
        )
    })
}

/// A thread id is a UUID: `urn:uuid:` may prefix it; anything else is the `uuid` crate's error.
pub fn uuid(id: &str) -> Check {
    let body = id.strip_prefix("urn:uuid:").unwrap_or(id);
    let skip = id.len() - body.len();
    if let Some((i, c)) = body
        .char_indices()
        .find(|(_, c)| !c.is_ascii_hexdigit() && *c != '-')
    {
        return Err(format!(
            "invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `{c}` at {}",
            skip + i + 1
        ));
    }
    // UNOBSERVED(7E Task 0 Step 3): the length and group messages (the `uuid` crate's words).
    let groups: Vec<usize> = body.split('-').map(str::len).collect();
    if groups.len() == 1 {
        if body.len() != 32 {
            return Err(format!(
                "invalid length: expected length 32 for simple format, found {}",
                body.len()
            ));
        }
        return Ok(());
    }
    if groups.len() != 5 {
        return Err(format!(
            "invalid group count: expected 5, found {}",
            groups.len()
        ));
    }
    for (g, (&got, want)) in groups.iter().zip([8, 4, 4, 4, 12]).enumerate() {
        if got != want {
            return Err(format!(
                "invalid group length in group {g}: expected {want}, found {got}"
            ));
        }
    }
    Ok(())
}

fn thread_id(p: &Value, what: &str) -> Result<String, String> {
    let id = required_str(p, "threadId")?;
    uuid(id).map_err(|e| format!("invalid {what} id: {e}"))?;
    Ok(id.to_string())
}

/// The settings `thread/start` and `thread/resume` share.
fn settings(p: &Value) -> Check {
    opt_str(p, "cwd")?;
    opt_str(p, "model")?;
    opt_enum(p, "sandbox", SANDBOX, false)?;
    opt_enum(p, "approvalPolicy", APPROVAL, true)?;
    opt_enum(p, "approvalsReviewer", REVIEWER, false)?;
    opt_str(p, "developerInstructions")?;
    opt_str(p, "baseInstructions")
}

/// `thread/start`; `experimental` is the client's `experimentalApi` capability.
pub fn thread_start(p: &Value, experimental: bool) -> Check {
    if p.is_null() {
        return bad("missing field `params`".into());
    }
    settings(p)?;
    opt_bool(p, "ephemeral")?;
    opt_str(p, "serviceName")?;
    opt_enum(p, "multiAgentMode", MULTI_AGENT, true)?;
    let tools = match &p["dynamicTools"] {
        Value::Null => vec![],
        Value::Array(a) => a.clone(),
        v => return invalid(v, "a sequence"),
    };
    for t in &tools {
        if !t.is_object() {
            return invalid(t, "struct DynamicToolSpec");
        }
        required_str(t, "name")?;
        required_str(t, "description")?;
        required(t, "inputSchema")?;
    }
    if !experimental {
        if !tools.is_empty() {
            return Err("thread/start.dynamicTools requires experimentalApi capability".into());
        }
        if !p["multiAgentMode"].is_null() {
            return Err("thread/start.multiAgentMode requires experimentalApi capability".into());
        }
    }
    let mut names: Vec<&str> = vec![];
    for t in &tools {
        let name = t["name"].as_str().unwrap_or("");
        if name.is_empty() {
            return Err("dynamic tool name must not be empty".into());
        }
        if names.contains(&name) {
            return Err(format!("duplicate dynamic tool name: {name}"));
        }
        names.push(name);
        let kind = match &t["inputSchema"] {
            Value::Object(_) => continue,
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Bool(_) => "boolean",
            _ => "sequence",
        };
        // UNOBSERVED(7E Task 0 Step 3): the kinds other than `number`.
        return Err(format!(
            "dynamic tool input schema is not supported for {name}: invalid type: {kind}, expected struct JsonSchema"
        ));
    }
    Ok(())
}

pub fn thread_resume(p: &Value) -> Result<String, String> {
    let id = required_str(p, "threadId")?;
    uuid(id).map_err(|e| format!("invalid session id: {e}"))?;
    settings(p)?;
    Ok(id.to_string())
}

pub fn thread_read(p: &Value) -> Result<String, String> {
    let id = thread_id(p, "thread")?;
    opt_bool(p, "includeTurns")?;
    Ok(id)
}

/// `(threadId, beforeTurnId)`.
pub fn thread_revert(p: &Value) -> Result<(String, String), String> {
    let id = thread_id(p, "thread")?;
    Ok((id, required_str(p, "beforeTurnId")?.to_string()))
}

/// `UserInput` items: a known `type`, and `text` for a text item.
fn input(p: &Value) -> Check {
    let Value::Array(items) = required(p, "input")? else {
        return invalid(&p["input"], "a sequence");
    };
    for it in items {
        match required_str(it, "type")? {
            "text" => {
                required_str(it, "text")?;
            }
            t if INPUT_TYPES.contains(&t) => {}
            t => return bad(unknown_variant(t, INPUT_TYPES)),
        }
    }
    Ok(())
}

pub fn turn_start(p: &Value) -> Result<String, String> {
    let id = thread_id(p, "thread")?;
    input(p)?;
    Ok(id)
}

/// `(threadId, expectedTurnId)`. UNOBSERVED(7E Task 0 Step 3): a non-UUID id here and in
/// `turn/interrupt` (the fake answers as `turn/start` does).
pub fn turn_steer(p: &Value) -> Result<(String, String), String> {
    let id = thread_id(p, "thread")?;
    let expected = required_str(p, "expectedTurnId")?.to_string();
    input(p)?;
    Ok((id, expected))
}

/// `(threadId, turnId)`.
pub fn turn_interrupt(p: &Value) -> Result<(String, String), String> {
    let id = thread_id(p, "thread")?;
    Ok((id, required_str(p, "turnId")?.to_string()))
}
