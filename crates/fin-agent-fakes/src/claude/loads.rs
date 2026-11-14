//! What the fake reads at start "as the real CLI would from the paths it is given" (plan 7D
//! Task 14; the matrix of Task 0 Step 3, 2.1.321), so a test can plant a canary in every place
//! the CLI reads configuration from and see which ones a launch lets through.
//!
//! Reading and applying are apart, as Task 0 saw them: the CLI **attempts** some reads whatever
//! its flags say and **applies** only what they allow. Each attempt is a record in `loads`:
//! `op` (`read` a file, `list` a directory), `status` (`ok`, `absent`, or `unreadable` with the
//! OS error kind, which is how a sandbox's denial shows) and `applies` (the flags let its
//! content apply). `canaries` lists the canaries of the files that were read *and* apply: a
//! canary is any text holding `FIN_CANARY_<name>`. A record names its place as `where` (`cwd`,
//! `cwd+<n>` for the n-th parent, `config`, `home`, `store`) and `file` relative to it: never a
//! full path, because the observed file holds no host paths (like it holds no environment
//! values). The sources and what turns each off, from Task 0:
//!
//! - memory files `CLAUDE.md`, `CLAUDE.local.md`, `AGENTS.md` (and `.claude/CLAUDE.md`) of the
//!   working directory and every parent, and the user's `<config>/CLAUDE.md`: all of them off
//!   (not even attempted) under `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1`;
//! - the `.claude/settings.json` and `.claude/settings.local.json` of the working directory and
//!   every parent: they apply only for the sources `--setting-sources` names (default: all
//!   three), none under `--restricted`; **the working directory's two are read whatever the flags
//!   say** (Task 0: `--setting-sources user` and `--restricted` alike), a parent's only when they
//!   apply;
//! - `.claude/{agents,commands,output-styles,skills,workflows}` of the working directory and
//!   every parent up to `/`: **listed whatever the flags say** (Task 0), applying with the
//!   `project` source;
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

/// The `.claude/` subdirectories the CLI lists in the working directory and every parent.
const LISTED: [&str; 5] = ["agents", "commands", "output-styles", "skills", "workflows"];

fn failed(e: &std::io::Error) -> Value {
    if e.kind() == std::io::ErrorKind::NotFound {
        json!("absent")
    } else {
        json!({"unreadable": format!("{:?}", e.kind())})
    }
}

struct Scan {
    loads: Vec<Value>,
    canaries: Vec<Value>,
}

impl Scan {
    fn record(
        &mut self,
        op: &str,
        source: &str,
        at: &str,
        file: &str,
        status: Value,
        applies: bool,
    ) {
        self.loads.push(json!({
            "op": op, "source": source, "where": at, "file": file, "status": status, "applies": applies,
        }));
    }

    /// Reads `base/file`; its canary counts only when it `applies`.
    fn read(&mut self, source: &str, at: &str, file: &str, base: &Path, applies: bool) {
        let status = match std::fs::read_to_string(base.join(file)) {
            Ok(text) => {
                if let Some(c) = canary(&text).filter(|_| applies) {
                    self.canaries
                        .push(json!({"source": source, "where": at, "file": file, "canary": c}));
                }
                json!("ok")
            }
            Err(e) => failed(&e),
        };
        self.record("read", source, at, file, status, applies);
    }

    /// Lists `base/file` (the names, not the contents).
    fn list(&mut self, source: &str, at: &str, file: &str, base: &Path, applies: bool) {
        let status = match std::fs::read_dir(base.join(file)) {
            Ok(_) => json!("ok"),
            Err(e) => failed(&e),
        };
        self.record("list", source, at, file, status, applies);
    }
}

/// `cwd` for the working directory, `cwd+<n>` for its n-th parent.
fn place(n: usize) -> String {
    if n == 0 {
        "cwd".to_owned()
    } else {
        format!("cwd+{n}")
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
            for name in [
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENTS.md",
                ".claude/CLAUDE.md",
            ] {
                s.read("memory", &place(n), name, dir, true);
            }
        }
        if let Some(c) = &config {
            s.read("user-memory", "config", "CLAUDE.md", c, true);
        }
    }
    // The project's settings are found from the working directory up (a project root above a
    // workspace is the project's `.claude/`). The working directory's two are read even when
    // they do not apply; a parent's only when they do. Each directory's `.claude/` is listed.
    for (n, dir) in cwd.ancestors().enumerate() {
        let at = place(n);
        for (source, file) in [
            ("project", ".claude/settings.json"),
            ("local", ".claude/settings.local.json"),
        ] {
            if wants(source) || n == 0 {
                s.read(&format!("{source}-settings"), &at, file, dir, wants(source));
            }
        }
        for sub in LISTED {
            s.list(
                "project-dir",
                &at,
                &format!(".claude/{sub}"),
                dir,
                wants("project"),
            );
        }
    }
    if wants("user")
        && let Some(c) = &config
    {
        s.read("user-settings", "config", "settings.json", c, true);
    }
    if !restricted && !args.flags.contains_key("--strict-mcp-config") {
        s.read("project-mcp", "cwd", ".mcp.json", &cwd, true);
    }
    match (env_path("CLAUDE_CONFIG_DIR"), &home) {
        (Some(c), _) => s.read("user-config", "config", ".claude.json", &c, true),
        (None, Some(h)) => s.read("user-config", "home", ".claude.json", h, true),
        (None, None) => {}
    }
    if let Some(store) = env_path("CLAUDE_SECURESTORAGE_CONFIG_DIR") {
        s.read("sign-in-store", "store", ".credentials.json", &store, true);
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
