//! A resumed transcript's tool calls that never got a result (`onResumeDangling`).
//!
//! TODO(7D Task 0 Step 1/5): what the real CLI does on `--resume` with a `tool_use` whose
//! result never came (its process was killed mid-tool): leaves it, adds its own interrupted
//! result, or asks again. Modelled here as `leave` (the default) or `reask`: a `can_use_tool`
//! with the same `tool_use_id` at the first turn (not announced again); the call runs again only
//! when allowed, and its result (or a denial's) goes into the transcript.

use serde_json::{Value, json};

use super::Runner;
use super::ask::Outcome;
use crate::claude::script::{Ask, OnDangling};

impl Runner {
    /// At the first turn of a resumed process (once): each dangling call asked again under
    /// `reask`, as the call it was: a `Bash` call allowed runs its command again and an MCP call
    /// is called again (the duplicate side effect the runtime must refuse); an answered one gets
    /// its result, so it is dangling no more.
    pub(super) fn reask_dangling(&mut self) {
        if self.script.on_resume_dangling != OnDangling::Reask {
            self.dangling.clear();
            return;
        }
        for (id, tool, input) in std::mem::take(&mut self.dangling) {
            let ask = Ask {
                bash: tool == "Bash",
                mcp: tool.starts_with("mcp__"),
                tool,
                input,
                cancel_after_ms: None,
                tool_use_id: Some(id.clone()),
                run: false,
            };
            let outcome = self.ask_as(&ask, false).0;
            let key = match outcome {
                Outcome::Allowed => "reran",
                Outcome::Denied | Outcome::Refused => "rerunDenied",
                Outcome::Cancelled | Outcome::Interrupted => "rerunDropped",
            };
            // Recorded before the call runs again: a test sees a re-run at once.
            self.observed.push(key, json!(id));
            if matches!(outcome, Outcome::Allowed | Outcome::Denied) {
                let allowed = matches!(outcome, Outcome::Allowed);
                if ask.bash {
                    self.bash(&ask, &id, allowed);
                } else if ask.mcp {
                    self.mcp_call(&ask, &id, allowed);
                }
            }
        }
    }

    /// A clean exit (stdin's end, an interrupt knob's exit): the transcript gets its `cost-state`
    /// entry (verified 2.1.321, Task 0), so a resume restores the totals.
    pub(super) fn finish(&self) -> i32 {
        self.finish_with(self.exit_code())
    }

    pub(super) fn finish_with(&self, code: i32) -> i32 {
        self.transcript.append_totals(&self.totals);
        code
    }

    /// An interrupt mid-tool (verified 2.1.321, Task 0, s1b): each call without a result gets an
    /// error result, then `[Request interrupted by user for tool use]`, both in the transcript too
    /// (so a resume finds nothing dangling). True when there was one.
    pub(super) fn abort_tools(&mut self) -> bool {
        let open = self.transcript.open_calls();
        for (id, _, _) in &open {
            let line = json!({"type": "user", "session_id": self.session_id, "parent_tool_use_id": null,
                "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": id, "is_error": true,
                    "content": "Error: the tool was stopped while running"}]}});
            self.emit_script_line(&line);
        }
        if !open.is_empty() {
            let text: Value = json!({"type": "user", "session_id": self.session_id, "parent_tool_use_id": null,
                "message": {"role": "user", "content": [{"type": "text", "text": "[Request interrupted by user for tool use]"}]}});
            self.emit_script_line(&text);
        }
        !open.is_empty()
    }
}
