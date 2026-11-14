//! `fake-claude` loads what a real CLI would from the paths it is given and reports every
//! planted canary it read (plan 7D Task 14: the matrix of Task 0 Step 3).

mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use common::{BASE_ARGS, BIN, Dir};
use serde_json::Value;

/// A planted tree: the user's home, a per-session config dir, the sign-in home with its store,
/// and a project whose workspace is the working directory.
struct Planted {
    dir: Dir,
}

impl Planted {
    fn new() -> Planted {
        let p = Planted {
            dir: Dir::new("loads"),
        };
        for (rel, tag) in [
            ("CLAUDE.md", "above_memory"),
            ("p/CLAUDE.md", "project_memory"),
            ("p/CLAUDE.local.md", "project_local_memory"),
            ("p/AGENTS.md", "project_agents"),
            ("p/.claude/settings.json", "project_settings_above"),
            ("p/workspace/s1/CLAUDE.md", "workspace_memory"),
            ("p/workspace/s1/AGENTS.md", "workspace_agents"),
            ("p/workspace/s1/.claude/settings.json", "project_settings"),
            (
                "p/workspace/s1/.claude/settings.local.json",
                "local_settings",
            ),
            ("p/workspace/s1/.claude/CLAUDE.md", "dot_claude_memory"),
            ("p/workspace/s1/.mcp.json", "project_mcp"),
            ("home/.claude/settings.json", "home_settings"),
            ("home/.claude/CLAUDE.md", "home_memory"),
            ("home/.claude.json", "home_state"),
            ("signin/settings.json", "signin_settings"),
            ("signin/CLAUDE.md", "signin_memory"),
            ("signin/.claude/settings.json", "signin_dot_claude"),
            ("signin/.claude.json", "signin_profile"),
        ] {
            p.plant(rel, tag);
        }
        // The store's file holds a credential, not a canary; the config dir is empty.
        p.write("signin/store/.credentials.json", "{}");
        std::fs::create_dir_all(p.path("cfg")).unwrap();
        p
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.dir.path(rel)
    }

    fn write(&self, rel: &str, text: &str) {
        let f = self.path(rel);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }

    fn plant(&self, rel: &str, tag: &str) {
        self.write(rel, &format!("{{\"note\": \"FIN_CANARY_{tag}\"}}"));
    }

    /// Runs the fake in the workspace with `args` and `env` (nothing else), stdin closed;
    /// returns the canaries it reports and every load it made.
    fn run(&self, args: &[&str], env: &[(&str, PathBuf)]) -> (BTreeSet<String>, Vec<Value>) {
        let observed = self.path("observed.json");
        let _ = std::fs::remove_file(&observed);
        let mut cmd = Command::new(BIN);
        cmd.args(BASE_ARGS)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("FAKE_CLAUDE_OBSERVED", &observed)
            .current_dir(self.path("p/workspace/s1"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        // A freshly copied binary can be briefly busy; this one is cargo's own.
        let status = cmd.status().expect("run fake-claude");
        assert!(status.code().is_some(), "{status:?}");
        let raw = std::fs::read_to_string(&observed).unwrap();
        let rec = serde_json::from_str::<Vec<Value>>(&raw)
            .unwrap()
            .pop()
            .unwrap();
        let canaries = rec["canaries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| {
                c["canary"]
                    .as_str()
                    .unwrap()
                    .trim_start_matches("FIN_CANARY_")
                    .to_owned()
            })
            .collect();
        (canaries, rec["loads"].as_array().unwrap().clone())
    }
}

fn set(tags: &[&str]) -> BTreeSet<String> {
    tags.iter().map(|t| (*t).to_owned()).collect()
}

/// Where each load looked (`cwd`, `config`, `store`, ...), without repeats.
fn places(loads: &[Value]) -> BTreeSet<String> {
    loads
        .iter()
        .map(|l| l["where"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_bare_launch_loads_every_planted_source() {
    let p = Planted::new();
    let (got, _) = p.run(&[], &[("HOME", p.path("home"))]);
    // Memory files up the tree, the project's settings and `.mcp.json`, the user's settings,
    // memory and state in `$HOME/.claude`: all of it, and nothing of the sign-in home.
    assert_eq!(
        got,
        set(&[
            "above_memory",
            "project_memory",
            "project_local_memory",
            "project_agents",
            "project_settings_above",
            "workspace_memory",
            "workspace_agents",
            "dot_claude_memory",
            "project_settings",
            "local_settings",
            "project_mcp",
            "home_settings",
            "home_memory",
            "home_state",
        ])
    );
}

#[test]
fn finishers_launch_lets_nothing_planted_through() {
    let p = Planted::new();
    // The flags and environment Finisher gives (plan 7D Decisions 1 and 5), with the user's
    // real home as `HOME` (a session's own is a different matter, Task 0) and the sign-in home
    // reachable only through its store.
    let env = [
        ("HOME", p.path("home")),
        ("CLAUDE_CONFIG_DIR", p.path("cfg")),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", p.path("signin/store")),
        ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", PathBuf::from("1")),
    ];
    let (got, loads) = p.run(&["--setting-sources", "user", "--strict-mcp-config"], &env);
    assert_eq!(got, set(&[]), "{loads:#?}");
    // Only the CLI's own places are opened: its (empty) config dir and the store; never the
    // user's home, the project, a parent directory or the rest of the sign-in home.
    assert_eq!(places(&loads), set(&["config", "store"]), "{loads:#?}");
    let files: BTreeSet<&str> = loads.iter().map(|l| l["file"].as_str().unwrap()).collect();
    assert_eq!(
        files,
        [".claude.json", ".credentials.json", "settings.json"].into()
    );
}

#[test]
fn restricted_reads_no_configuration_but_the_state_and_the_store() {
    let p = Planted::new();
    let env = [
        ("HOME", p.path("home")),
        ("CLAUDE_CONFIG_DIR", p.path("cfg")),
        ("CLAUDE_SECURESTORAGE_CONFIG_DIR", p.path("signin/store")),
        ("CLAUDE_CODE_DISABLE_CLAUDE_MDS", PathBuf::from("1")),
    ];
    let (got, loads) = p.run(&["--restricted"], &env);
    assert_eq!(got, set(&[]));
    assert_eq!(loads.len(), 2, "{loads:#?}");
    assert_eq!(places(&loads), set(&["config", "store"]));
}

#[test]
fn each_switch_turns_off_its_own_sources() {
    let p = Planted::new();
    let home = [("HOME", p.path("home"))];
    // `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` alone: every memory file is unread, settings still load.
    let mut env = home.to_vec();
    env.push(("CLAUDE_CODE_DISABLE_CLAUDE_MDS", PathBuf::from("1")));
    let (got, _) = p.run(&[], &env);
    assert_eq!(
        got,
        set(&[
            "project_settings_above",
            "project_settings",
            "local_settings",
            "project_mcp",
            "home_settings",
            "home_state"
        ])
    );
    // `--setting-sources user` alone: the project's and local settings are unread.
    let (got, _) = p.run(&["--setting-sources", "user"], &home);
    assert!(!got.contains("project_settings") && !got.contains("local_settings"));
    assert!(
        got.contains("home_settings")
            && got.contains("project_mcp")
            && got.contains("project_memory")
    );
    // `--strict-mcp-config` alone: `.mcp.json` is unread.
    let (got, _) = p.run(&["--strict-mcp-config"], &home);
    assert!(!got.contains("project_mcp") && got.contains("project_settings"));
}

#[cfg(unix)]
#[test]
fn an_unreadable_file_is_reported_not_loaded() {
    use std::os::unix::fs::PermissionsExt;
    let p = Planted::new();
    let f = p.path("p/CLAUDE.md");
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&f).is_ok() {
        return; // root reads anything
    }
    let (got, loads) = p.run(&[], &[("HOME", p.path("home"))]);
    assert!(!got.contains("project_memory"));
    let hit = loads
        .iter()
        .find(|l| l["where"] == "cwd+2" && l["file"] == "CLAUDE.md")
        .unwrap();
    assert_eq!(hit["status"]["unreadable"], "PermissionDenied");
}
