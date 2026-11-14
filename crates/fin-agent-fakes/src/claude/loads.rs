//! What the fake loads at start "as the real CLI would from the paths it is given" (plan 7D
//! Task 14; the matrix of Task 0 Step 3, 2.1.321), so a test can plant a canary in every place
//! the CLI reads configuration from and see which ones a launch lets through.
//!
//! A canary is any text holding `FIN_CANARY_<name>`. Each candidate file is opened the way the
//! CLI would open it; the record says what happened (`loaded`, `absent`, or `unreadable` with
//! the OS error kind, which is how a sandbox's denial shows), and `canaries` lists the canaries
//! of the files that loaded. A record names its file as `where` (`cwd`, `cwd+<n>` for the n-th
//! parent, `config`, `home`, `store`) and `file` relative to it: never a full path, because the
//! observed file holds no host paths (like it holds no environment values). The sources and what turns each off, from Task 0:
//!
//! - memory files `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md` (and `.claude/CLAUDE.md`) of the
//!   working directory and every parent, and the user's `<config>/CLAUDE.md`: all of them off
//!   under `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`;
//! - the `.claude/settings.json` and `.claude/settings.local.json` of the working directory and every
//!   parent: only the
//!   sources `--setting-sources` names (default: all three); none under `--restricted`;
//! - the user's `<config>/settings.json` (`user` source; `<config>` is `CLAUDE_CONFIG_DIR`, else
//!   `$HOME/.claude`); none under `--restricted`;
//! - the working directory's `.mcp.json`: off under `--strict-mcp-config` and `--restricted`;
//! - the user's `.claude.json` (`$CLAUDE_CONFIG_DIR/.claude.json`, else `$HOME/.claude.json`):
//!   the CLI's own state, read whatever the flags say;
//! - the sign-in store `$CLAUDE_SECURESTORAGE_CONFIG_DIR/.credentials.json` when that variable
//!   is set. Nothing else of the sign-in home is ever read in a session.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::argv::Args;
use super::observed::Observed;

const CANARY: &str = "FIN_CANARY_";

/// The canary `text` holds, with its name, if any.
fn canary(text: &str) -> Option<String> {
    let at = text.find(CANARY)?;
    let name: String = text[at..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    Some(name)
}

struct Scan {
    loads: Vec<Value>,
    canaries: Vec<Value>,
}

impl Scan {
    fn read(&mut self, source: &str, at: &str, file: &str, base: &Path) {
        let path = base.join(file);
        let status = match std::fs::read_to_string(&path) {
            Ok(text) => {
                if let Some(c) = canary(&text) {
                    self.canaries
                        .push(json!({"source": source, "where": at, "file": file, "canary": c}));
                }
                json!("loaded")
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!("absent"),
            Err(e) => json!({"unreadable": format!("{:?}", e.kind())}),
        };
        self.loads
            .push(json!({"source": source, "where": at, "file": file, "status": status}));
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Loads the sources and records them in `observed` (`loads`, `canaries`).
pub fn observe(args: &Args, observed: &mut Observed) {
    let (loads, canaries) = scan(args);
    observed.set("loads", Value::Array(loads));
    observed.set("canaries", Value::Array(canaries));
}

fn scan(args: &Args) -> (Vec<Value>, Vec<Value>) {
    let mut s = Scan {
        loads: vec![],
        canaries: vec![],
    };
    let restricted = args.flags.contains_key("--restricted");
    let sources: Vec<&str> = match args.str("--setting-sources") {
        Some(v) => v.split(',').collect(),
        None => vec!["user", "project", "local"],
    };
    let wants = |name: &str| !restricted && sources.contains(&name);
    let cwd = std::env::current_dir().unwrap_or_default();
    let home = env_path("HOME");
    let config = env_path("CLAUDE_CONFIG_DIR").or_else(|| home.as_ref().map(|h| h.join(".claude")));

    // Memory files, up the tree.
    if std::env::var("CLAUDE_CODE_DISABLE_CLAUDE_MDS").as_deref() != Ok("1") {
        for (n, dir) in cwd.ancestors().enumerate() {
            let at = if n == 0 {
                "cwd".to_owned()
            } else {
                format!("cwd+{n}")
            };
            for name in [
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENTS.md",
                ".claude/CLAUDE.md",
            ] {
                s.read("memory", &at, name, dir);
            }
        }
        if let Some(c) = &config {
            s.read("user-memory", "config", "CLAUDE.md", c);
        }
    }
    // The project's settings are found from the working directory up (a project root above a
    // workspace is the project's `.claude/`).
    for (n, dir) in cwd.ancestors().enumerate() {
        let at = if n == 0 {
            "cwd".to_owned()
        } else {
            format!("cwd+{n}")
        };
        if wants("project") {
            s.read("project-settings", &at, ".claude/settings.json", dir);
        }
        if wants("local") {
            s.read("local-settings", &at, ".claude/settings.local.json", dir);
        }
    }
    if wants("user")
        && let Some(c) = &config
    {
        s.read("user-settings", "config", "settings.json", c);
    }
    if !restricted && !args.flags.contains_key("--strict-mcp-config") {
        s.read("project-mcp", "cwd", ".mcp.json", &cwd);
    }
    match (env_path("CLAUDE_CONFIG_DIR"), &home) {
        (Some(c), _) => s.read("user-config", "config", ".claude.json", &c),
        (None, Some(h)) => s.read("user-config", "home", ".claude.json", h),
        (None, None) => {}
    }
    if let Some(store) = env_path("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        s.read("sign-in-store", "store", ".credentials.json", &store);
    }
    (s.loads, s.canaries)
}

#[cfg(test)]
mod tests {
    use super::canary;

    #[test]
    fn a_canary_is_named_by_its_token() {
        assert_eq!(
            canary("a FIN_CANARY_memory_1 b").as_deref(),
            Some("FIN_CANARY_memory_1")
        );
        assert_eq!(
            canary("{\"x\":\"FIN_CANARY_a\"}").as_deref(),
            Some("FIN_CANARY_a")
        );
        assert_eq!(canary("nothing here"), None);
    }
}
