//! The Linux `agent` and `hook` profiles through the launcher (plan 7C Task 5, spec §8.11):
//! `fin-sandbox-exec` applies the profile and `exec`s `fin-sandbox-probe --none <op>`, which
//! makes one attempt. A denial passes only on an observed `EACCES`/`EXDEV` (filesystem and
//! TCP); each denied case is first run unsandboxed as a control, so a missing file never passes.
//! The seccomp filter is deferred (plan Task 4): no case here depends on it. Every case runs
//! with the socket namespace off (`FIN_TEST_SANDBOX_NAMESPACES=unavailable`), so it tests
//! Landlock alone, except the gap cases: `pathname_sockets_hidden_by_the_namespace` and
//! `udp_leaves_only_where_the_namespace_allows` assert what the namespace closes where it is
//! available (Decision 26) and the residual where it is not; `namespace_linux.rs` has the rest.
#![cfg(target_os = "linux")]

mod linux_support;

use std::path::{Path, PathBuf};
use std::process::Command;

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::linux::landlock::Abi;
use fin_sandbox::{AgentInput, FinEndpoint, HookFin, HookInput, Os};
use linux_support::{
    Outcome, PROBE, failure, landlock_or_skip, launch_raw, listener, namespaces_or_skip, probe, s,
    tempdir, write,
};

/// Landlock alone: the namespace (tested in `namespace_linux.rs`) would answer `ENOENT` for
/// hidden paths first and hide what these cases check.
const NS_OFF: (&str, &str) = ("FIN_TEST_SANDBOX_NAMESPACES", "unavailable");

/// Server data, a project, a second project and a home directory under one temporary root.
struct Tree {
    _d: tempfile::TempDir,
    t: PathBuf,
}

/// The server-data files no agent process may read (spec §8.11 "Server data").
const SERVER_DATA: &[&str] = &[
    "data/auth/local.key",
    "data/auth/console.key",
    "data/auth/credential.master",
    "data/settings.json",
    "data/control.db",
    "data/diagnostics/journal-0001.seg",
    "data/agent-traces/traces.db",
    "data/audit/audit.log",
];

impl Tree {
    fn new() -> Tree {
        let (_d, t) = tempdir();
        let mut files: Vec<(&str, &str)> = SERVER_DATA.iter().map(|p| (*p, "secret")).collect();
        files.extend([
            ("data/providers/codex/home/auth.json", "{}"),
            ("data/providers/codex/home/config.toml", "x"),
            ("data/providers/claudeCode/home/.credentials.json", "{}"),
            ("data/agents/s1/credential.json", "{}"),
            ("data/agents/s1/settings/settings.json", "{}"),
            (
                "data/agents/s1/git/config",
                fin_sandbox::data::session_git_config(),
            ),
            ("data/agents/s1/blobs/b1", "b"),
            ("data/agents/s2/credential.json", "{}"),
            ("data/agents/s2/tmp/note", "n"),
            ("p/sources/clip.mp4", "c"),
            ("p/workspace/s2/draft.txt", "d"),
            ("p/.finisher/segment", "s"),
            ("p/finisher.project.json", "{}"),
            ("p/CLAUDE.md", "m"),
            ("p/hooks/check.sh", "#!/bin/sh\n"),
            ("q/other.txt", "o"),
            ("home/.claude/settings.json", "{}"),
            ("home/.codex/config.toml", "x"),
            ("home/.ssh/id_ed25519", "k"),
        ]);
        for (p, text) in files {
            write(&t.join(p), text);
        }
        for d in [
            "p/workspace/s1",
            "p/workspace/shared",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "hook-tmp",
        ] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        Tree { _d, t }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel)
    }

    fn ps(&self, rel: &str) -> String {
        s(&self.p(rel))
    }

    /// The toolchain: the base list and the probe itself.
    fn toolchains() -> Vec<PathBuf> {
        let mut v = fin_sandbox::data::toolchain_base_resolved(Os::Linux);
        v.push(std::fs::canonicalize(PROBE).unwrap());
        v
    }

    fn agent(&self, harness: Harness, fin: Option<FinEndpoint>) -> LaunchRequest {
        LaunchRequest::Agent(AgentInput {
            session_id: "s1".into(),
            mode: AgentMode::Draft,
            harness,
            project_dir: self.p("p"),
            workspace_dir: self.p("p/workspace/s1"),
            shared_dir: Some(self.p("p/workspace/shared")),
            library_roots: vec![],
            session_dir: self.p("data/agents/s1"),
            sign_in_store: (harness == Harness::Codex)
                .then(|| self.p("data/providers/codex/home/auth.json")),
            toolchains: Tree::toolchains(),
            server_data: self.p("data"),
            home: self.p("home"),
            fin,
        })
    }

    fn hook(&self, fin: Option<HookFin>) -> LaunchRequest {
        LaunchRequest::Hook(HookInput {
            project_dir: self.p("p"),
            hook_file: self.p("p/hooks/check.sh"),
            toolchains: Tree::toolchains(),
            tmp: self.p("hook-tmp"),
            server_data: self.p("data"),
            home: self.p("home"),
            fin,
            timeout_ms: None,
        })
    }
}

/// Runs `op` through the launcher under `req`, Landlock alone.
fn launched(req: &LaunchRequest, op: &[String]) -> Outcome {
    launched_env(req, op, &[NS_OFF])
}

/// [`launched`] with `env` instead (the gap cases leave the namespace on).
fn launched_env(req: &LaunchRequest, op: &[String], env: &[(&str, &str)]) -> Outcome {
    let bytes = serde_json::to_vec(req).unwrap();
    let mut args = vec!["--policy-fd", "3", "--", PROBE, "--none"];
    args.extend(op.iter().map(String::as_str));
    let out = launch_raw(Some(&bytes), &args, env);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        Ok(v) if out.status.code() == Some(0) && v.get("op").is_some() => {
            Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
        }
        _ if out.status.code() == Some(78) => Outcome::Apply(failure(&out).to_string()),
        _ => Outcome::Other {
            code: out.status.code(),
            stdout: format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)),
        },
    }
}

fn op(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// `op` succeeds unsandboxed (the control) and is denied under `req`.
fn denied(req: &LaunchRequest, v: &[&str]) {
    let o = op(v);
    assert_eq!(
        probe(&["--none".into()], &o),
        Outcome::Ran(None),
        "control {v:?}"
    );
    undo(v);
    let got = launched(req, &o);
    assert!(got.denied(&o), "{v:?} was not denied: {got:?}");
}

/// Undoes what a control run of `v` created, so the sandboxed run meets the same tree.
fn undo(v: &[&str]) {
    let rm = |p: &str| {
        let p = Path::new(p);
        let _ = std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p));
    };
    match v {
        ["create" | "mkfifo" | "mkdir" | "bind-unix", p] => rm(p),
        ["link" | "symlink", _, to] => rm(to),
        ["rename", from, to] => std::fs::rename(to, from).unwrap(),
        _ => {}
    }
}

/// `op` succeeds under `req`.
fn allowed(req: &LaunchRequest, v: &[&str]) {
    assert_eq!(launched(req, &op(v)), Outcome::Ran(None), "{v:?}");
}

fn abi(test: &str) -> Option<Abi> {
    landlock_or_skip(test)
}

#[test]
fn agent_cannot_read_server_data() {
    let Some(_) = abi("agent_cannot_read_server_data") else {
        return;
    };
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    for f in SERVER_DATA {
        denied(&req, &["read", &t.ps(f)]);
    }
    denied(&req, &["list", &t.ps("data")]);
    denied(&req, &["list", &t.ps("data/auth")]);
    // The user's own CLI configuration and keys.
    denied(&req, &["write", &t.ps("home/.claude/settings.json")]);
    denied(&req, &["write", &t.ps("home/.codex/config.toml")]);
    denied(&req, &["read", &t.ps("home/.ssh/id_ed25519")]);
    denied(&req, &["read", &t.ps("q/other.txt")]);
}

#[test]
fn agent_reaches_only_its_harness_store_and_its_session() {
    let Some(_) = abi("agent_reaches_only_its_harness_store_and_its_session") else {
        return;
    };
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    allowed(
        &req,
        &["read", &t.ps("data/providers/codex/home/auth.json")],
    );
    allowed(
        &req,
        &["write", &t.ps("data/providers/codex/home/auth.json")],
    );
    denied(
        &req,
        &["read", &t.ps("data/providers/codex/home/config.toml")],
    );
    denied(
        &req,
        &[
            "read",
            &t.ps("data/providers/claudeCode/home/.credentials.json"),
        ],
    );
    denied(
        &req,
        &["create", &t.ps("data/providers/codex/home/hooks.json")],
    );
    // Its own session: credential and settings read-only, tmp and provider writable, no blobs.
    allowed(&req, &["read", &t.ps("data/agents/s1/credential.json")]);
    denied(&req, &["write", &t.ps("data/agents/s1/credential.json")]);
    denied(
        &req,
        &["write", &t.ps("data/agents/s1/settings/settings.json")],
    );
    allowed(&req, &["create", &t.ps("data/agents/s1/tmp/scratch")]);
    allowed(
        &req,
        &["create", &t.ps("data/agents/s1/provider/state.json")],
    );
    denied(&req, &["read", &t.ps("data/agents/s1/blobs/b1")]);
    // Another session's directory.
    denied(&req, &["read", &t.ps("data/agents/s2/credential.json")]);
    denied(&req, &["read", &t.ps("data/agents/s2/tmp/note")]);
    denied(&req, &["list", &t.ps("data/agents/s2")]);
    // The project: its own workspace and shared written, another session's read only, state
    // protected.
    allowed(&req, &["create", &t.ps("p/workspace/s1/out.txt")]);
    allowed(&req, &["create", &t.ps("p/workspace/shared/out.txt")]);
    allowed(&req, &["read", &t.ps("p/workspace/s2/draft.txt")]);
    denied(&req, &["write", &t.ps("p/workspace/s2/draft.txt")]);
    denied(&req, &["write", &t.ps("p/.finisher/segment")]);
    denied(&req, &["write", &t.ps("p/finisher.project.json")]);
    denied(
        &req,
        &[
            "link",
            &t.ps("p/.finisher/segment"),
            &t.ps("p/workspace/s1/seg"),
        ],
    );
}

#[test]
fn claude_md_is_unreadable_for_claude_code_only() {
    let Some(_) = abi("claude_md_is_unreadable_for_claude_code_only") else {
        return;
    };
    let t = Tree::new();
    denied(
        &t.agent(Harness::ClaudeCode, None),
        &["read", &t.ps("p/CLAUDE.md")],
    );
    allowed(
        &t.agent(Harness::Codex, None),
        &["read", &t.ps("p/CLAUDE.md")],
    );
}

#[test]
fn agent_tcp_reaches_its_listener_and_never_binds() {
    let Some(abi) = abi("agent_tcp_reaches_its_listener_and_never_binds") else {
        return;
    };
    if abi.0 < 4 {
        eprintln!(
            "agent_tcp: skipped, Landlock ABI {} has no TCP rules",
            abi.0
        );
        return;
    }
    let t = Tree::new();
    let (_l1, fin) = listener();
    let (_l2, other) = listener();
    let req = t.agent(Harness::Codex, Some(FinEndpoint::Tcp { port: fin }));
    allowed(&req, &["connect-tcp", &fin.to_string()]);
    denied(&req, &["connect-tcp", &other.to_string()]);
    denied(&req, &["bind-tcp", "0"]);
    // Recorded residual (Decision 6a): `listen` on an unbound socket binds an ephemeral port
    // without passing Landlock's `bind` check (observed through ABI 8). If a kernel starts
    // refusing it, this assertion fails and the residual can be closed.
    if abi.0 <= 8 {
        assert_eq!(launched(&req, &op(&["listen-tcp"])), Outcome::Ran(None));
    }
}

#[test]
fn agent_devices_are_headless() {
    let Some(_) = abi("agent_devices_are_headless") else {
        return;
    };
    let t = Tree::new();
    for req in [t.agent(Harness::Codex, None), t.hook(None)] {
        for dev in ["/dev/null", "/dev/zero", "/dev/full"] {
            allowed(&req, &["write", dev]);
            allowed(&req, &["read", dev]);
        }
        allowed(&req, &["read", "/dev/urandom"]);
        allowed(&req, &["read", "/dev/random"]);
        // Landlock answers before the driver, so the tty is EACCES with or without a terminal.
        for dev in ["/dev/tty", "/dev/ptmx"] {
            let got = launched(&req, &op(&["read", dev]));
            assert_eq!(got, Outcome::Ran(Some(libc::EACCES)), "{dev}");
        }
        let got = launched(&req, &op(&["write", "/dev/urandom"]));
        assert_eq!(got, Outcome::Ran(Some(libc::EACCES)));
    }
}

#[test]
fn agent_makes_fifos_but_no_sockets() {
    let Some(_) = abi("agent_makes_fifos_but_no_sockets") else {
        return;
    };
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    allowed(&req, &["mkfifo", &t.ps("p/workspace/s1/fifo")]);
    allowed(&req, &["mkfifo", &t.ps("data/agents/s1/tmp/fifo")]);
    denied(&req, &["mkfifo", &t.ps("p/fifo")]);
    denied(&req, &["bind-unix", &t.ps("p/workspace/s1/sock")]);
    denied(&req, &["bind-unix", &t.ps("data/agents/s1/tmp/sock")]);
    let hook = t.hook(None);
    allowed(&hook, &["mkfifo", &t.ps("hook-tmp/fifo")]);
    denied(&hook, &["bind-unix", &t.ps("hook-tmp/sock")]);
}

#[test]
fn pathname_sockets_hidden_by_the_namespace() {
    let test = "pathname_sockets_hidden_by_the_namespace";
    let Some(abi) = abi(test) else {
        return;
    };
    let t = Tree::new();
    // In the home directory, a hidden root (a socket like `~/.ssh` ControlMaster's or
    // `gpg-agent`'s); `namespace_linux.rs` covers `/run` and `$XDG_RUNTIME_DIR`.
    let path = t.p("home/.gnupg/S.gpg-agent");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let _l = std::os::unix::net::UnixListener::bind(&path).unwrap();
    let o = op(&["connect-unix", &s(&path)]);
    assert_eq!(probe(&["--none".into()], &o), Outcome::Ran(None), "control");
    for req in [t.agent(Harness::Codex, None), t.hook(None)] {
        let got = launched_env(&req, &o, &[]);
        if namespaces_or_skip(test) {
            // Decision 26: the location is an empty tmpfs inside the namespace.
            assert_eq!(got, Outcome::Ran(Some(libc::ENOENT)));
        } else if abi.0 >= 9 {
            // `RESOLVE_UNIX`: only the policy's sockets.
            assert_eq!(got, Outcome::Ran(Some(libc::EACCES)));
        } else {
            // Recorded residual (`SANDBOX_SOCKETS_EXPOSED`): no namespace, Landlock below ABI
            // 9 and no seccomp filter, so nothing refuses it.
            assert_eq!(got, Outcome::Ran(None));
        }
    }
}

#[test]
fn udp_leaves_only_where_the_namespace_allows() {
    let test = "udp_leaves_only_where_the_namespace_allows";
    let Some(_) = abi(test) else {
        return;
    };
    // Landlock handles TCP only. A `hook` without network gets an empty network namespace
    // (Decision 26), so its datagram cannot leave; an `agent` (and a hook that declares `fin`)
    // keeps the host's network, so UDP stays a residual for it until the seccomp filter: the
    // datagram is observed arriving, and this must fail when the filter is wired in.
    let t = Tree::new();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let port = sock.local_addr().unwrap().port().to_string();
    let o = op(&["send-udp", &port]);
    let mut b = [0u8; 4];
    if namespaces_or_skip(test) {
        assert_eq!(
            launched_env(&t.hook(None), &o, &[]),
            Outcome::Ran(Some(libc::ENETUNREACH))
        );
    } else {
        assert_eq!(launched_env(&t.hook(None), &o, &[]), Outcome::Ran(None));
        assert_eq!(sock.recv(&mut b).unwrap(), 1);
    }
    assert_eq!(
        launched_env(&t.agent(Harness::Codex, None), &o, &[]),
        Outcome::Ran(None)
    );
    assert_eq!(sock.recv(&mut b).unwrap(), 1);
}

#[test]
fn git_works_under_the_session_configuration() {
    let Some(_) = abi("git_works_under_the_session_configuration") else {
        return;
    };
    let t = Tree::new();
    let ws = t.p("p/workspace/s1");
    let git = |args: &[&str], sandboxed: bool| {
        let env: Vec<(String, String)> = vec![
            ("PATH".into(), "/usr/bin:/bin".into()),
            ("HOME".into(), t.ps("home")),
            ("USER".into(), "agent".into()),
            ("LOGNAME".into(), "agent".into()),
            ("SHELL".into(), "/bin/sh".into()),
            ("TMPDIR".into(), t.ps("data/agents/s1/tmp")),
            (
                "GIT_CONFIG_GLOBAL".into(),
                t.ps("data/agents/s1/git/config"),
            ),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_AUTHOR_NAME".into(), "Agent".into()),
            ("GIT_AUTHOR_EMAIL".into(), "agent@example.invalid".into()),
            ("GIT_COMMITTER_NAME".into(), "Agent".into()),
            ("GIT_COMMITTER_EMAIL".into(), "agent@example.invalid".into()),
        ];
        let env: Vec<(&str, &str)> = env.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let mut argv = vec!["git", "-C", ws.to_str().unwrap()];
        argv.extend(args);
        let out = if sandboxed {
            let req = serde_json::to_vec(&t.agent(Harness::Codex, None)).unwrap();
            let mut a = vec!["--policy-fd", "3", "--"];
            a.extend(&argv);
            let mut env = env.clone();
            env.push(NS_OFF);
            launch_raw(Some(&req), &a, &env)
        } else {
            Command::new(argv[0])
                .args(&argv[1..])
                .env_clear()
                .envs(env.iter().copied())
                .output()
                .unwrap()
        };
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    git(&["init", "-q"], false);
    write(&ws.join("a.txt"), "a");
    assert!(git(&["status", "--porcelain"], true).contains("a.txt"));
    git(&["add", "-A"], true);
    git(&["commit", "-q", "-m", "first"], true);
    assert_eq!(git(&["log", "--format=%s"], true).trim(), "first");
    for (k, v) in [
        ("core.fsmonitor", "false"),
        ("commit.gpgsign", "false"),
        ("credential.helper", ""),
    ] {
        assert_eq!(git(&["config", "--get", k], true).trim(), v, "{k}");
    }
}

#[test]
fn hook_reads_the_project_and_has_no_network() {
    let Some(abi) = abi("hook_reads_the_project_and_has_no_network") else {
        return;
    };
    let t = Tree::new();
    let (_l, port) = listener();
    let hook = t.hook(None);
    allowed(&hook, &["read", &t.ps("p/sources/clip.mp4")]);
    allowed(&hook, &["read", &t.ps("p/hooks/check.sh")]);
    denied(&hook, &["write", &t.ps("p/sources/clip.mp4")]);
    denied(&hook, &["create", &t.ps("p/workspace/s1/x")]);
    allowed(&hook, &["create", &t.ps("hook-tmp/x")]);
    denied(&hook, &["read", &t.ps("data/auth/local.key")]);
    denied(&hook, &["read", &t.ps("data/agents/s1/credential.json")]);
    if abi.0 >= 4 {
        denied(&hook, &["connect-tcp", &port.to_string()]);
        denied(&hook, &["bind-tcp", "0"]);
        // Declaring `fin` opens the listener's port and nothing else.
        let with_fin = t.hook(Some(HookFin {
            credential: t.p("data/agents/s1/credential.json"),
            endpoint: FinEndpoint::Tcp { port },
        }));
        allowed(&with_fin, &["connect-tcp", &port.to_string()]);
        allowed(
            &with_fin,
            &["read", &t.ps("data/agents/s1/credential.json")],
        );
        let (_l2, other) = listener();
        denied(&with_fin, &["connect-tcp", &other.to_string()]);
    }
}

#[test]
fn every_toolchain_entry_is_canonical() {
    // A link in a root is refused when the ruleset is built, so the resolved list must hold none.
    for p in fin_sandbox::data::toolchain_base_resolved(Os::Linux) {
        assert_eq!(std::fs::canonicalize(&p).unwrap(), p);
        assert!(Path::new(&p).is_absolute());
    }
}
