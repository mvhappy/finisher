//! What the CLI does with a call itself: a script step's side effects, the Bash tool's login
//! shell, and the refusal of a write outside the working directory (no prompt at all).

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;

use serde_json::{Value, json};

use super::Runner;
use super::lines::fresh_uuid;
use crate::claude::script::{Ask, Spawn, WriteFile};

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

    /// An allowed `Bash` call (`ask.run`) as the CLI runs it: verified on 2.1.321 (7D Task 0,
    /// s3B), a login shell, `$SHELL` or else the passwd shell (this fake stands in `/bin/sh`),
    /// which sources the rc files under `$HOME`. Its stdout is the call's `tool_result`.
    pub(super) fn run_bash(&mut self, ask: &Ask, tool_use_id: &str) {
        let command = ask.input["command"].as_str().unwrap_or_default();
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
        let out = std::process::Command::new(&shell)
            .args(["-l", "-c", command])
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
