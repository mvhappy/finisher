//! A resumed transcript's tool calls that never got a result (`onResumeDangling`).
//!
//! TODO(7D Task 0 Step 1/5): what the real CLI does on `--resume` with a `tool_use` whose
//! result never came (its process was killed mid-tool): leaves it, adds its own interrupted
//! result, or asks again. Modelled here as `leave` (the default) or `reask`: a `can_use_tool`
//! with the same `tool_use_id` at the first turn; the call runs again only when allowed.

use serde_json::{Value, json};

use super::Runner;
use super::ask::Outcome;
use crate::claude::script::{Ask, OnDangling};

impl Runner {
    /// At the first turn of a resumed process (once): each dangling call asked again under `reask`.
    pub(super) fn reask_dangling(&mut self) {
        if self.script.on_resume_dangling != OnDangling::Reask {
            self.dangling.clear();
            return;
        }
        for (id, tool, input) in std::mem::take(&mut self.dangling) {
            let ask = Ask {
                tool,
                input,
                cancel_after_ms: None,
                tool_use_id: Some(id.clone()),
                bash: false,
            };
            let key = match self.ask(&ask).0 {
                Outcome::Allowed => "reran",
                Outcome::Denied => "rerunDenied",
                Outcome::Cancelled | Outcome::Interrupted => "rerunDropped",
            };
            self.observed.push(key, json!(id));
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
