//! A permission prompt: a `can_use_tool` control request (`--permission-prompt-tool stdio`, or
//! no flag), or, when the flag names an MCP tool (`mcp__<server>__<tool>`), an `mcp_message`
//! `tools/call` of that tool whose text content is the `{behavior, ...}` answer.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Ev, Runner};
use crate::claude::script::Ask;

pub(super) enum Outcome {
    Allowed,
    Denied,
    /// Withdrawn by `cancelAfterMs` (`control_cancel_request`); neither branch runs.
    Cancelled,
    Interrupted,
}

/// The MCP tool `--permission-prompt-tool` names, as `(server, tool)`.
fn mcp_tool(flag: Option<&str>) -> Option<(String, String)> {
    let (server, tool) = flag?.strip_prefix("mcp__")?.split_once("__")?;
    Some((server.to_string(), tool.to_string()))
}

/// The `{behavior, ...}` body of an answer, or `{error}`.
fn body(v: &Value, via_mcp: bool) -> Value {
    let resp = &v["response"];
    if resp["subtype"] != "success" {
        return json!({"error": resp["error"]});
    }
    if !via_mcp {
        return resp["response"].clone();
    }
    let rpc = &resp["response"]["mcp_response"];
    rpc["result"]["content"][0]["text"]
        .as_str()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_else(|| json!({"error": rpc["error"]}))
}

impl Runner {
    pub(super) fn next_rid(&mut self) -> String {
        self.req_n += 1;
        format!("fake-req-{}", self.req_n)
    }

    /// The prompt for `ask`, and the call's `tool_use_id`.
    pub(super) fn ask(&mut self, ask: &Ask) -> (Outcome, String) {
        let via = mcp_tool(self.args.str("--permission-prompt-tool"));
        let tool_use_id = ask
            .tool_use_id
            .clone()
            .unwrap_or_else(|| format!("toolu_fake_{}", self.req_n + 1));
        if ask.bash {
            self.announce(ask, &tool_use_id);
            if self.bash_rejected(ask, &tool_use_id) {
                return (Outcome::Cancelled, tool_use_id);
            }
        }
        let rid = match &via {
            None => {
                let rid = self.next_rid();
                let request = if ask.bash {
                    // The request 2.1.321 sent for every Bash call (verified 2.1.321, Task 0).
                    let description = ask.input["description"].clone();
                    json!({
                        "subtype": "can_use_tool", "tool_name": ask.tool, "display_name": ask.tool,
                        "input": ask.input, "description": description,
                        "decision_reason_type": "rule", "decision_reason_rule_source": "flagSettings",
                        "server_policy_ask_only": false, "tool_use_id": tool_use_id,
                    })
                } else {
                    json!({
                        "subtype": "can_use_tool", "tool_name": ask.tool, "input": ask.input,
                        "permission_suggestions": [], "blocked_path": null, "decision_reason": null,
                        "tool_use_id": tool_use_id,
                    })
                };
                self.emit(
                    &json!({"type": "control_request", "request_id": rid, "request": request}),
                );
                rid
            }
            Some((server, tool)) => {
                let id = self.req_n + 1;
                self.mcp_send(
                    server,
                    json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
                        "name": tool,
                        "arguments": {"tool_name": ask.tool, "input": ask.input, "tool_use_id": tool_use_id},
                    }}),
                )
            }
        };
        let deadline = ask
            .cancel_after_ms
            .map(|ms| Instant::now() + Duration::from_millis(ms));
        loop {
            if self.eof {
                // No answer can come once stdin is closed: the call is refused.
                return (Outcome::Denied, tool_use_id);
            }
            let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
            match self.next(left) {
                Ev::Answer(v, raw) if v["response"]["request_id"] == rid.as_str() => {
                    let body = body(&v, via.is_some());
                    let allowed = body["behavior"] == "allow";
                    self.observed.push(
                        "answers",
                        json!({"tool": ask.tool, "requestId": rid,
                            "via": if via.is_some() { "mcp" } else { "stdio" },
                            "answer": body, "raw": raw}),
                    );
                    let outcome = if allowed {
                        Outcome::Allowed
                    } else {
                        Outcome::Denied
                    };
                    return (outcome, tool_use_id);
                }
                Ev::Timeout => {
                    self.emit(&json!({"type": "control_cancel_request", "request_id": rid}));
                    self.observed.push("cancelled", json!(rid));
                    return (Outcome::Cancelled, tool_use_id);
                }
                Ev::Interrupted => return (Outcome::Interrupted, tool_use_id),
                Ev::User(u) => self.pending.push_back(u),
                _ => {}
            }
        }
    }
}
