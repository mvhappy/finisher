//! The CLI's `Bash` tool (`ask {bash: true}`, plan 7D Task 12), in the shapes 2.1.321 wrote
//! (verified 2.1.321, Task 0): the call is announced as an `assistant` `tool_use` before its
//! `can_use_tool`; the input is `{command, description?, timeout?, run_in_background?}`; once
//! answered, an allowed command runs as the CLI runs it (a login shell of `$SHELL`, as
//! `ask.run`: verified on 2.1.321, s3B) in the fake's working directory and environment, and the
//! call's `tool_result` is written (a denied one gets an error result).
//!
//! `run_in_background: true` under `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` (Finisher's launch
//! sets it) is rejected as an unknown parameter before any prompt: an error `tool_result`, no
//! `can_use_tool`, nothing run. Without the variable the real CLI starts a background task and
//! later writes `system/task_*` lines and a new turn; the fake only starts the process, does not
//! wait for it, and writes the result at once (the later lines are not modelled).

//!
//! Also what the CLI does with a call itself (Task 0 reconciliation, part B): a script step's
//! side effects, `ask.run`'s login shell, and the refusal of a write outside the working
//! directory (no prompt at all).

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};

use super::Runner;
use super::lines::fresh_uuid;
use crate::claude::script::{Ask, Spawn, WriteFile};

const UNKNOWN_BACKGROUND: &str = "<tool_use_error>InputValidationError: Bash failed due to the following issue:\nAn unexpected parameter `run_in_background` was provided</tool_use_error>";

fn background(ask: &Ask) -> bool {
    ask.input["run_in_background"] == true
}

/// The CLI's shell (verified on 2.1.321, 7D Task 0, s3B): `$SHELL`, else the passwd shell (this
/// fake stands in `/bin/sh`), as a login shell, which sources the rc files under `$HOME`.
fn login_shell(command: &str) -> (String, [String; 3]) {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    (shell, ["-l".into(), "-c".into(), command.into()])
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
            let (shell, args) = login_shell(&command);
            let started = std::process::Command::new(&shell)
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            self.observed.push(
                "spawned",
                json!({"argv": [&shell, &args[0], &args[1], &args[2]], "background": true}),
            );
            match started {
                Ok(child) => (
                    format!("Command running in background with ID: fake_{}", child.id()),
                    false,
                ),
                Err(e) => (e.to_string(), true),
            }
        } else {
            let (shell, args) = login_shell(&command);
            let out = std::process::Command::new(&shell)
                .args(&args)
                .stdin(Stdio::null())
                .output();
            let code = out.as_ref().ok().and_then(|o| o.status.code());
            self.observed.push(
                "spawned",
                json!({"argv": [&shell, &args[0], &args[1], &args[2]], "code": code}),
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

/// The tools `blockWritesOutsideWorkingDirectories` covers, with the input field naming the file.
const WRITE_TOOLS: [(&str, &str); 3] = [
    ("Write", "file_path"),
    ("Edit", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

/// `p` without `.` and `..` (lexically: no link is followed).
fn normal(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

impl Runner {
    pub(super) fn side_effects(&mut self, write: Option<&WriteFile>, spawn: Option<&Spawn>) {
        if let Some(w) = write {
            let _ = std::fs::write(&w.path, &w.contents);
        }
        if let Some(sp) = spawn {
            let code = sp.argv.split_first().and_then(|(prog, rest)| {
                std::process::Command::new(prog)
                    .args(rest)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .status()
                    .ok()
            });
            self.observed.push(
                "spawned",
                json!({"argv": sp.argv, "code": code.and_then(|c| c.code())}),
            );
        }
    }

    /// Verified on 2.1.321 (7D Task 0, s2a): with `blockWritesOutsideWorkingDirectories`, a
    /// Write or Edit outside the working directory never sends `can_use_tool`; the CLI writes
    /// the `is_error` result itself and lists the call in `result.permission_denials`. The path
    /// is compared lexically (TODO(7D Task 0 Step 2): whether the CLI resolves links first).
    /// True when `ask` was refused so.
    pub(super) fn refuse_outside_write(&mut self, ask: &Ask, tool_use_id: &str) -> bool {
        let block = self
            .settings
            .values
            .get("permissions")
            .and_then(|p| p.get("blockWritesOutsideWorkingDirectories"));
        if block != Some(&Value::Bool(true)) {
            return false;
        }
        let Some((_, key)) = WRITE_TOOLS.iter().find(|(t, _)| *t == ask.tool) else {
            return false;
        };
        let (Some(file), Ok(cwd)) = (ask.input[*key].as_str(), std::env::current_dir()) else {
            return false;
        };
        if normal(&cwd.join(file)).starts_with(normal(&cwd)) {
            return false;
        }
        let message = format!(
            "{file} is outside {}; the permissions.blockWritesOutsideWorkingDirectories setting blocks writes outside the working directories. Ask the user to add the directory with /add-dir, or to remove that setting.",
            cwd.display()
        );
        self.emit_script_line(&json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": message, "is_error": true, "tool_use_id": tool_use_id}]},
            "parent_tool_use_id": null, "session_id": self.session_id, "uuid": fresh_uuid(),
            "tool_use_result": format!("Error: {message}"),
            "tool_result_meta": [{"id": tool_use_id, "non_execution_kind": "permission-rule",
                "permission_decision": {"decision": "reject", "source": "config", "reason_type": "other"}}],
        }));
        self.deny(ask, tool_use_id);
        self.observed.push(
            "cliRefused",
            json!({"tool": ask.tool, "toolUseId": tool_use_id}),
        );
        true
    }

    /// The call joins the turn's `result.permission_denials` (verified on 2.1.321, 7D Task 0:
    /// a host deny, a prompt withdrawn by an interrupt, and the CLI's own refusal alike).
    pub(super) fn deny(&mut self, ask: &Ask, tool_use_id: &str) {
        self.denials.push(
            json!({"tool_name": ask.tool, "tool_use_id": tool_use_id, "tool_input": ask.input}),
        );
    }

    /// An allowed `Bash` call (`ask.run`) as the CLI runs it (`login_shell`). Its stdout is the
    /// call's `tool_result`.
    pub(super) fn run_bash(&mut self, ask: &Ask, tool_use_id: &str) {
        let command = ask.input["command"].as_str().unwrap_or_default();
        let (shell, args) = login_shell(command);
        let out = std::process::Command::new(&shell)
            .args(&args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        let (stdout, code) = match out {
            Ok(o) => (
                String::from_utf8_lossy(&o.stdout).into_owned(),
                o.status.code(),
            ),
            Err(e) => (e.to_string(), None),
        };
        self.observed.push(
            "bash",
            json!({"command": command, "shell": shell, "code": code, "stdout": stdout}),
        );
        self.emit_script_line(&json!({
            "type": "user",
            "message": {"role": "user", "content": [{"type": "tool_result", "content": stdout, "is_error": code != Some(0), "tool_use_id": tool_use_id}]},
            "parent_tool_use_id": null, "session_id": self.session_id, "uuid": fresh_uuid(),
        }));
    }
}
