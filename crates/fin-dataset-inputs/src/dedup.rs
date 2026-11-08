//! The duplicate rule (sp7 §12; plan 7A Task 10). Session phases and synthetic tool results
//! come from finisherd's outbox, which redelivers after a crash between the record and its
//! `outboxDone`: across a crash an entry may appear twice. A reader keeps the first
//! `agent.toolCall` and `agent.toolResult` of each `callId` and the first `agent.approval` of
//! each `approvalId`, and drops an `agent.session` that repeats its session's previous phase
//! (same `phase` and `reason`).

use std::collections::{HashMap, HashSet};

use fin_core::events::Envelope;
use serde_json::Value;

/// `entries` (one store's, oldest first) with the duplicates removed.
pub fn dedup(entries: Vec<Envelope>) -> Vec<Envelope> {
    let mut seen: HashSet<(&'static str, String)> = HashSet::new();
    let mut phases: HashMap<String, (Value, Value)> = HashMap::new();
    let id = |e: &Envelope, field: &str| {
        e.payload
            .get(field)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    entries
        .into_iter()
        .filter(|e| {
            let (class, field) = match e.kind.as_str() {
                "agent.toolCall" => ("call", "callId"),
                "agent.toolResult" => ("result", "callId"),
                "agent.approval" => ("approval", "approvalId"),
                "agent.session" => {
                    let Some(session) = id(e, "sessionId") else {
                        return true;
                    };
                    let p = &e.payload;
                    let phase = (
                        p.get("phase").cloned().unwrap_or(Value::Null),
                        p.get("reason").cloned().unwrap_or(Value::Null),
                    );
                    return phases.insert(session, phase.clone()) != Some(phase);
                }
                _ => return true,
            };
            id(e, field).is_none_or(|k| seen.insert((class, k)))
        })
        .collect()
}
