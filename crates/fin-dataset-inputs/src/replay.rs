//! The replay of a run's turns from the agent-trace store (sp7 §12.4; plan 7A Task 10): the
//! Rust twin of finisherd's `replayBodies` (`apps/finisherd/src/agent/journal-content.ts`,
//! which documents the `messagesDelta` format). Each turn's request body is rebuilt byte for
//! byte from its delta, its `system` and `tools` blobs and the previous turn's messages, then
//! checked against the turn's `bodyBlake3`. A turn is dropped (`None`) when its delta is based
//! on a turn other than the one replayed just before (`baseTurnId`), does not apply, or
//! rebuilds to other bytes; after a drop only a turn that stands alone (`from: 0`, or raw)
//! replays again. Message texts are kept as they were stored (raw JSON), never re-serialised,
//! so object key order survives.

use std::collections::HashMap;

use serde_json::Value;
use serde_json::value::RawValue;

/// One turn as the replay reads it.
#[derive(Debug, Clone)]
pub struct ReplayTurn<'a> {
    pub turn_id: &'a str,
    /// The `messagesDelta` value (inline, or its blob parsed).
    pub delta: &'a Value,
    /// The `system` and `tools` blobs' text.
    pub system: &'a str,
    pub tools: &'a str,
    /// The turn's `bodyBlake3` (`blake3:<hex>`); none never replays.
    pub body_blake3: Option<&'a str>,
}

type Raw = Box<RawValue>;

struct Rebuilt {
    body: String,
    messages: Vec<Raw>,
}

/// Each turn's exact request body, oldest first, or `None` where it cannot be trusted.
pub fn replay(turns: &[ReplayTurn<'_>]) -> Vec<Option<String>> {
    let mut previous: Vec<Raw> = Vec::new();
    let mut previous_id: Option<&str> = None;
    turns
        .iter()
        .map(|t| {
            let d = t.delta;
            let standalone =
                d.get("raw").is_some() || d.get("from").and_then(Value::as_u64) == Some(0);
            let based = standalone
                || d.get("baseTurnId")
                    .and_then(Value::as_str)
                    .is_some_and(|b| Some(b) == previous_id);
            let r = based
                .then(|| rebuild(d, &previous, t.system, t.tools))
                .flatten();
            match r {
                Some(r) if t.body_blake3 == Some(hash_of(&r.body).as_str()) => {
                    previous = r.messages;
                    previous_id = Some(t.turn_id);
                    Some(r.body)
                }
                _ => {
                    previous.clear();
                    previous_id = None;
                    None
                }
            }
        })
        .collect()
}

/// `blake3:<hex>` of `text`'s bytes, unkeyed.
pub fn hash_of(text: &str) -> String {
    format!("blake3:{}", blake3::hash(text.as_bytes()).to_hex())
}

fn is_system(m: &RawValue) -> bool {
    serde_json::from_str::<Value>(m.get())
        .ok()
        .is_some_and(|v| v.is_object() && v.get("role").and_then(Value::as_str) == Some("system"))
}

fn rebuild(d: &Value, previous: &[Raw], system: &str, tools: &str) -> Option<Rebuilt> {
    if d.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    if let Some(raw) = d.get("raw") {
        let body = raw.as_str()?.to_owned();
        let all = serde_json::from_str::<HashMap<String, Raw>>(&body)
            .ok()
            .and_then(|o| serde_json::from_str::<Vec<Raw>>(o.get("messages")?.get()).ok())
            .unwrap_or_default();
        let skip = usize::from(all.first().is_some_and(|m| is_system(m)));
        return Some(Rebuilt {
            messages: all.into_iter().skip(skip).collect(),
            body,
        });
    }
    let from = usize::try_from(d.get("from")?.as_u64()?).ok()?;
    if from > previous.len() {
        return None;
    }
    let new: Vec<Raw> = serde_json::from_str(d.get("messages")?.as_str()?).ok()?;
    let rest: HashMap<String, Raw> = serde_json::from_str(d.get("rest")?.as_str()?).ok()?;
    let sys: Raw = serde_json::from_str(system.trim()).ok()?;
    let tools: Raw = serde_json::from_str(tools.trim()).ok()?;
    let messages: Vec<Raw> = previous[..from].iter().cloned().chain(new).collect();
    let mut body = String::from("{");
    for (i, k) in d.get("keys")?.as_array()?.iter().enumerate() {
        let k = k.as_str()?;
        if i > 0 {
            body.push(',');
        }
        body.push_str(&serde_json::to_string(k).ok()?);
        body.push(':');
        match k {
            "messages" => {
                body.push('[');
                let head = (sys.get() != "null").then_some(&sys);
                for (j, m) in head.into_iter().chain(&messages).enumerate() {
                    if j > 0 {
                        body.push(',');
                    }
                    body.push_str(m.get());
                }
                body.push(']');
            }
            "tools" => body.push_str(tools.get()),
            _ => body.push_str(rest.get(k)?.get()),
        }
    }
    body.push('}');
    Some(Rebuilt { body, messages })
}
