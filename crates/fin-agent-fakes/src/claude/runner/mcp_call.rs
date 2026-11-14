//! A call of an SDK MCP server's tool (`ask {mcp: true}`, plan 7D Task 13): Finisher's brokered
//! `mcp__finisher__shell` (Decision 21). As the CLI plays any tool: an `assistant` `tool_use`,
//! the permission prompt (`can_use_tool` with Decision 4's `ask` list), then, when allowed, a
//! `tools/call` over `mcp_message` to the server that listed the tool, and the call's
//! `tool_result` carrying the answer's `content` (`is_error` from `isError`, or the JSON-RPC
//! error's message). A tool no connected server listed is refused before any prompt, as the CLI
//! refuses an unknown tool. An interrupt while the call waits ends the turn as usual and
//! withdraws the call with `notifications/cancelled {requestId}`.
//!
//! TODO(7D Task 13 Step 0): checked against the real CLI on Windows: the `_meta`
//! `claudecode/toolUseId` key of `tools/call`, and whether an interrupt sends
//! `notifications/cancelled` (the MCP client's way) before or after the turn's `result`.

use serde_json::{Value, json};

use super::lines::fresh_uuid;
use super::{Ev, Runner};
use crate::claude::script::Ask;

/// `mcp__<server>__<tool>` as `(server, tool)`.
fn split(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("mcp__")?.split_once("__")
}

impl Runner {
    /// A tool no connected SDK server listed: the CLI's own error result, no prompt. True when so.
    pub(super) fn mcp_absent(&mut self, ask: &Ask, tool_use_id: &str) -> bool {
        if !ask.mcp || self.mcp_tools().contains(&ask.tool) {
            return false;
        }
        let text = format!(
            "<tool_use_error>Error: No such tool available: {}</tool_use_error>",
            ask.tool
        );
        self.mcp_result(tool_use_id, json!(text), true);
        true
    }

    /// The answered call: `tools/call` when allowed, then its `tool_result`.
    pub(super) fn mcp_call(&mut self, ask: &Ask, tool_use_id: &str, allowed: bool) {
        let Some((server, tool)) = split(&ask.tool).map(|(s, t)| (s.to_string(), t.to_string()))
        else {
            return;
        };
        if !allowed {
            let text = format!("Permission to use {} has been denied.", ask.tool);
            self.mcp_result(tool_use_id, json!(text), true);
            return;
        }
        self.req_n += 1;
        let id = self.req_n;
        let rid = self.mcp_send(
            &server,
            json!({"jsonrpc": "2.0", "id": id, "method": "tools/call", "params": {
                "name": tool, "arguments": ask.input,
                "_meta": {"claudecode/toolUseId": tool_use_id},
            }}),
        );
        loop {
            match self.next(None) {
                Ev::Answer(v, _) if v["response"]["request_id"] == rid.as_str() => {
                    let rpc = &v["response"]["response"]["mcp_response"];
                    let result = &rpc["result"];
                    let (content, is_error) = if v["response"]["subtype"] != "success" {
                        (
                            json!(v["response"]["error"].as_str().unwrap_or("error")),
                            true,
                        )
                    } else if !rpc["error"].is_null() || result.is_null() {
                        let m = rpc["error"]["message"]
                            .as_str()
                            .unwrap_or("the MCP call failed");
                        (json!(m), true)
                    } else {
                        (result["content"].clone(), result["isError"] == true)
                    };
                    self.observed.push(
                        "mcpCalls",
                        json!({"tool": ask.tool, "requestId": rid, "toolUseId": tool_use_id,
                            "result": result, "error": rpc["error"]}),
                    );
                    self.mcp_result(tool_use_id, content, is_error);
                    return;
                }
                Ev::Interrupted => {
                    self.mcp_notify(
                        &server,
                        json!({"jsonrpc": "2.0", "method": "notifications/cancelled",
                            "params": {"requestId": id, "reason": "interrupted"}}),
                    );
                    return;
                }
                Ev::Eof => {
                    // No answer can come once stdin is closed.
                    self.mcp_result(tool_use_id, json!("the MCP server did not answer"), true);
                    return;
                }
                Ev::User(u) => self.pending.push_back(u),
                _ => {}
            }
        }
    }

    /// The call's `tool_result`, in the transcript too (`bash.rs` `tool_result`).
    fn mcp_result(&mut self, tool_use_id: &str, content: Value, is_error: bool) {
        self.emit_conversation(&json!({
            "type": "user", "session_id": self.session_id, "parent_tool_use_id": null,
            "uuid": fresh_uuid(),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error},
            ]},
        }));
    }
}
