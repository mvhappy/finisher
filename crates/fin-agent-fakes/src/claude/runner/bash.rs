//! The CLI's `Bash` tool (`ask {bash: true}`, plan 7D Task 12), in the shapes 2.1.321 wrote
//! (verified 2.1.321, Task 0): the call is announced as an `assistant` `tool_use` before its
//! `can_use_tool`; the input is `{command, description?, timeout?, run_in_background?}`; once
//! answered, an allowed command runs with `/bin/sh -c` in the fake's working directory and
//! environment and the call's `tool_result` is written (a denied one gets an error result).
//!
//! `run_in_background: true` under `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` (Finisher's launch
//! sets it) is rejected as an unknown parameter before any prompt: an error `tool_result`, no
//! `can_use_tool`, nothing run. Without the variable the real CLI starts a background task and
//! later writes `system/task_*` lines and a new turn; the fake only starts the process, does not
//! wait for it, and writes the result at once (the later lines are not modelled).

use std::process::Stdio;

use serde_json::json;

use super::Runner;
use super::lines::fresh_uuid;
use crate::claude::script::Ask;

const UNKNOWN_BACKGROUND: &str = "<tool_use_error>InputValidationError: Bash failed due to the following issue:\nAn unexpected parameter `run_in_background` was provided</tool_use_error>";

fn background(ask: &Ask) -> bool {
    ask.input["run_in_background"] == true
}

impl Runner {
    /// The `assistant` line naming the call.
    pub(super) fn announce(&mut self, ask: &Ask, tool_use_id: &str) {
        self.emit(&json!({
            "type": "assistant", "session_id": self.session_id, "parent_tool_use_id": null,
            "uuid": fresh_uuid(),
            "message": {"id": fresh_uuid(), "model": self.model(), "role": "assistant",
                "content": [{"type": "tool_use", "id": tool_use_id, "name": ask.tool, "input": ask.input}]},
        }));
    }

    /// A background call while background tasks are disabled: rejected before any prompt.
    pub(super) fn bash_rejected(&mut self, ask: &Ask, tool_use_id: &str) -> bool {
        let disabled =
            std::env::var("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS").is_ok_and(|v| v == "1");
        if !(disabled && background(ask)) {
            return false;
        }
        self.tool_result(tool_use_id, UNKNOWN_BACKGROUND, true);
        true
    }

    /// Runs the allowed command (or not) and writes the call's `tool_result`.
    pub(super) fn bash(&mut self, ask: &Ask, tool_use_id: &str, allowed: bool) {
        let command = ask.input["command"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let (content, is_error) = if !allowed {
            ("Permission to use Bash has been denied.".to_string(), true)
        } else if background(ask) {
            let started = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            self.observed.push(
                "spawned",
                json!({"argv": ["/bin/sh", "-c", command], "background": true}),
            );
            match started {
                Ok(child) => (
                    format!("Command running in background with ID: fake_{}", child.id()),
                    false,
                ),
                Err(e) => (e.to_string(), true),
            }
        } else {
            let out = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .stdin(Stdio::null())
                .output();
            let code = out.as_ref().ok().and_then(|o| o.status.code());
            self.observed.push(
                "spawned",
                json!({"argv": ["/bin/sh", "-c", command], "code": code}),
            );
            match out {
                Ok(o) => {
                    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                    text.push_str(&String::from_utf8_lossy(&o.stderr));
                    (text, !o.status.success())
                }
                Err(e) => (e.to_string(), true),
            }
        };
        self.tool_result(tool_use_id, &content, is_error);
    }

    fn tool_result(&mut self, tool_use_id: &str, content: &str, is_error: bool) {
        self.emit(&json!({
            "type": "user", "session_id": self.session_id, "parent_tool_use_id": null,
            "uuid": fresh_uuid(),
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error},
            ]},
        }));
    }
}
