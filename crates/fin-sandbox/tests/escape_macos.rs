//! The macOS escape suite (plan 7C Task 8, spec §8.11 "macOS"): the `agent` and `hook`
//! profiles through the real launcher (`fin-sandbox-exec` applies Seatbelt to itself and
//! `exec`s `fin-sandbox-probe --none <op>` or a system tool), and the `worker` and `plugin`
//! shapes through the probe. A denial passes only on an observed `EPERM`/`EACCES` (or the
//! launcher's `EXEC_FAILED` naming "Operation not permitted"), and each denied case is first
//! run unsandboxed as a control, so a missing file or tool never passes.
#![cfg(target_os = "macos")]

mod macos_support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::profile::for_os;
use fin_sandbox::{AgentInput, FinEndpoint, HookFin, HookInput, Os, PluginInput};
use macos_support::{Outcome, PROBE, assert_denied, launch, listener, probe, s, tempdir, write};

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

/// Server data, a project, another folder and a home directory under one temporary root.
struct Tree {
    _d: tempfile::TempDir,
    t: PathBuf,
    /// Bound sockets live as long as the tree.
    _socks: Vec<std::os::unix::net::UnixListener>,
}

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
            ("data/agents/s1/blobs/b1", "b"),
            ("data/agents/s2/credential.json", "{}"),
            ("data/agents/s2/tmp/note", "n"),
            ("data/run/next-to-socket", "x"),
            ("p/workspace/s2/draft.txt", "d"),
            ("p/.finisher/segment", "s"),
            ("p/finisher.project.json", "{}"),
            ("p/CLAUDE.md", "m"),
            ("p/hooks/check.sh", "#!/bin/sh\n"),
            ("q/other.txt", "o"),
            ("home/.claude/settings.json", "{}"),
            ("home/.codex/config.toml", "x"),
            ("home/.ssh/id_ed25519", "k"),
            ("home/Library/Keychains/login.keychain-db", "k"),
            ("home/Documents/letter.txt", "l"),
            ("home/Desktop/note.txt", "n"),
        ]);
        for (p, text) in files {
            write(&t.join(p), text);
        }
        for d in [
            "p/workspace/s1",
            "p/workspace/shared",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "data/agents/s1/git",
            "hook-tmp",
        ] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        // `fin.sock` and a socket beside it (short names: `sun_path` holds 104 bytes).
        let socks = ["data/run/fin.sock", "data/run/o.sock"]
            .iter()
            .map(|p| std::os::unix::net::UnixListener::bind(t.join(p)).unwrap())
            .collect();
        Tree {
            _d,
            t,
            _socks: socks,
        }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel)
    }

    fn ps(&self, rel: &str) -> String {
        s(&self.p(rel))
    }

    /// The toolchain: the base list and the probe itself.
    fn toolchains(extra: &[PathBuf]) -> Vec<PathBuf> {
        let mut v = fin_sandbox::data::toolchain_base_resolved(Os::Macos);
        v.push(std::fs::canonicalize(PROBE).unwrap());
        v.extend(extra.iter().cloned());
        v
    }

    fn agent_input(&self, harness: Harness, fin: Option<FinEndpoint>) -> AgentInput {
        AgentInput {
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
            toolchains: Tree::toolchains(&[]),
            server_data: self.p("data"),
            home: self.p("home"),
            fin,
        }
    }

    fn agent(&self, harness: Harness, fin: Option<FinEndpoint>) -> LaunchRequest {
        LaunchRequest::Agent(self.agent_input(harness, fin))
    }

    fn fin_sock(&self) -> Option<FinEndpoint> {
        Some(FinEndpoint::UnixSocket {
            path: self.p("data/run/fin.sock"),
        })
    }

    fn hook(&self, fin: Option<HookFin>) -> LaunchRequest {
        LaunchRequest::Hook(HookInput {
            project_dir: self.p("p"),
            hook_file: self.p("p/hooks/check.sh"),
            toolchains: Tree::toolchains(&[]),
            tmp: self.p("hook-tmp"),
            server_data: self.p("data"),
            home: self.p("home"),
            fin,
        })
    }

    fn plugin(&self) -> PluginInput {
        PluginInput {
            store_dir: self.p("data/plugins/store/acme.x/1.0.0"),
            runtime: vec![],
            system: Tree::toolchains(&[]),
            onnx_runtime: None,
            models: vec![],
            call_dir: self.p("data/plugins/calls/c1"),
            tmp: self.p("data/plugins/calls/c1/tmp"),
            plugin_data: None,
            gpu: vec![],
            memory_bytes: None,
            server_data: self.p("data"),
            home: self.p("home"),
        }
    }
}

/// Runs `program args…` through the launcher under `req`.
fn launch_program(req: &LaunchRequest, argv: &[&str], env: &[(&str, &str)]) -> Output {
    launch(&serde_json::to_vec(req).unwrap(), argv, env)
}

/// Runs the probe's `op` through the launcher under `req`.
fn launched(req: &LaunchRequest, op: &[String]) -> Outcome {
    let mut argv = vec![PROBE, "--none"];
    argv.extend(op.iter().map(String::as_str));
    let out = launch_program(req, &argv, &[]);
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
        Ok(v) if out.status.code() == Some(0) && v.get("op").is_some() => {
            Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
        }
        _ => Outcome::Other {
            code: out.status.code(),
            stdout: format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)),
        },
    }
}

fn op(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

/// Undoes what a control run of `v` created, so the sandboxed run meets the same tree.
fn undo(v: &[&str]) {
    let rm = |p: &str| {
        let p = Path::new(p);
        let _ = std::fs::remove_file(p).or_else(|_| std::fs::remove_dir(p));
    };
    match v {
        ["create" | "mkfifo" | "mkdir", p] => rm(p),
        ["rmdir", p] => std::fs::create_dir(p).unwrap(),
        ["unlink", p] => std::fs::write(p, "").unwrap(),
        ["link" | "symlink", _, to] => rm(to),
        ["rename", from, to] => std::fs::rename(to, from).unwrap(),
        _ => {}
    }
}

/// `op` succeeds unsandboxed (the control) and is denied under `req`.
#[track_caller]
fn denied(req: &LaunchRequest, v: &[&str]) {
    let o = op(v);
    assert_eq!(
        probe(&["--none".into()], &o),
        Outcome::Ran(None),
        "control {v:?}"
    );
    undo(v);
    let got = launched(req, &o);
    assert!(got.denied(), "{v:?} was not denied: {got:?}");
}

/// `op` succeeds under `req`.
#[track_caller]
fn allowed(req: &LaunchRequest, v: &[&str]) {
    assert_eq!(launched(req, &op(v)), Outcome::Ran(None), "{v:?}");
}

/// The launcher refused to `exec` the program: the sandbox's `EPERM`.
#[track_caller]
fn exec_refused(out: &Output, what: &str) {
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(126), "{what}: {out:?}");
    assert!(
        err.contains("EXEC_FAILED") && err.contains("Operation not permitted"),
        "{what}: {err}"
    );
}

#[test]
fn agent_cannot_read_server_data() {
    let t = Tree::new();
    let req = t.agent(Harness::Codex, t.fin_sock());
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
    // protected, and no hard link out of it.
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
    denied(
        &req,
        &[
            "rename",
            &t.ps("p/.finisher"),
            &t.ps("p/workspace/s1/state"),
        ],
    );
}

#[test]
fn shared_roots_cannot_be_removed_or_swapped_for_a_link() {
    // A session's write roots and the harness's shared sign-in store stay what they are: an
    // agent that replaced one with a link would make every later launch fail (a linked root
    // is refused) and hand the server a planted link.
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    let store = t.ps("data/providers/codex/home/auth.json");
    allowed(&req, &["write", &store]);
    denied(&req, &["unlink", &store]);
    denied(
        &req,
        &[
            "rename",
            &store,
            &t.ps("data/providers/codex/home/old.json"),
        ],
    );
    for root in ["p/workspace/s1", "p/workspace/shared", "data/agents/s1/tmp"] {
        denied(&req, &["rmdir", &t.ps(root)]);
        denied(
            &req,
            &[
                "rename",
                &t.ps(root),
                &t.ps("data/agents/s1/provider/moved"),
            ],
        );
    }
    // Their entries stay writable.
    allowed(&req, &["mkdir", &t.ps("p/workspace/s1/d")]);
    allowed(&req, &["rmdir", &t.ps("p/workspace/s1/d")]);
}

#[test]
fn a_linked_denied_path_is_denied_at_its_target() {
    // Dotfiles kept as links (`~/.ssh -> dotfiles/ssh`) are denied where the link leads.
    let t = Tree::new();
    let dot = t.p("home/dotfiles/ssh");
    write(&dot.join("id_rsa"), "k");
    std::fs::rename(t.p("home/.ssh"), t.p("home/old-ssh")).unwrap();
    std::os::unix::fs::symlink(&dot, t.p("home/.ssh")).unwrap();
    // The library root is the home directory, so only the denial keeps the key out.
    let mut input = t.agent_input(Harness::Codex, None);
    input.library_roots = vec![t.p("home/dotfiles")];
    let req = LaunchRequest::Agent(input);
    denied(&req, &["read", &s(&dot.join("id_rsa"))]);
    denied(&req, &["read", &t.ps("home/.ssh/id_rsa")]);
}

#[test]
fn a_linked_library_root_is_refused() {
    let t = Tree::new();
    std::os::unix::fs::symlink(t.p("q"), t.p("lib-link")).unwrap();
    let mut input = t.agent_input(Harness::Codex, None);
    input.library_roots = vec![t.p("lib-link")];
    let out = launch_program(
        &LaunchRequest::Agent(input),
        &[PROBE, "--none", "list", "/"],
        &[],
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(78), "{out:?}");
    assert!(
        err.contains("SANDBOX_UNAVAILABLE") && err.contains("no other read root"),
        "{err}"
    );
}

#[test]
fn claude_md_is_unreadable_for_claude_code_only() {
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
fn fin_reaches_its_socket_and_nothing_else_of_server_data() {
    let t = Tree::new();
    let req = t.agent(Harness::Codex, t.fin_sock());
    allowed(&req, &["connect-unix", &t.ps("data/run/fin.sock")]);
    denied(&req, &["connect-unix", &t.ps("data/run/o.sock")]);
    denied(&req, &["read", &t.ps("data/run/next-to-socket")]);
    denied(&req, &["list", &t.ps("data/run")]);
    // Without the endpoint, not even `fin.sock`.
    denied(
        &t.agent(Harness::Codex, None),
        &["connect-unix", &t.ps("data/run/fin.sock")],
    );
}

#[test]
fn agent_loopback_reaches_only_finishers_listener() {
    let t = Tree::new();
    let (_l1, fin) = listener();
    let (_l2, other) = listener();
    // No listener on macOS (`fin.sock`): every loopback port is refused.
    let req = t.agent(Harness::Codex, t.fin_sock());
    denied(&req, &["connect-tcp", &fin.to_string()]);
    // A listener port named in the policy is the one exception.
    let req = t.agent(Harness::Codex, Some(FinEndpoint::Tcp { port: fin }));
    allowed(&req, &["connect-tcp", &fin.to_string()]);
    denied(&req, &["connect-tcp", &other.to_string()]);
    denied(&req, &["bind-tcp", "0"]);
    // Seatbelt's `localhost` covers the host's own addresses too: a service bound to every
    // interface is no more reachable through the host's LAN address.
    let any = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
    let port = any.local_addr().unwrap().port().to_string();
    if let Some(ip) = own_ipv4() {
        denied(&req, &["connect-ip", &ip, &port]);
    } else {
        eprintln!("agent_loopback: no non-loopback IPv4 address; the own-address case skipped");
    }
}

/// A non-loopback IPv4 address of this host (`getifaddrs`), if it has one.
fn own_ipv4() -> Option<String> {
    let mut ifs: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills a list freed below; each entry is read while it lives.
    unsafe {
        if libc::getifaddrs(&mut ifs) != 0 {
            return None;
        }
        let mut cur = ifs;
        let mut found = None;
        while !cur.is_null() {
            let a = (*cur).ifa_addr;
            if !a.is_null() && i32::from((*a).sa_family) == libc::AF_INET {
                let sin = &*(a as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_loopback() && !ip.is_link_local() {
                    found = Some(ip.to_string());
                    break;
                }
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(ifs);
        found
    }
}

#[test]
fn the_seatbelt_base_and_the_agents_extras() {
    let t = Tree::new();
    let agent = t.agent(Harness::Codex, None);
    let hook = t.hook(None);
    // Every profile: `sysctl` reads, `/` listed, signals to itself and its own children.
    for req in [&agent, &hook] {
        allowed(req, &["sysctl", "hw.ncpu"]);
        allowed(req, &["list", "/"]);
        allowed(req, &["signal-self"]);
        denied(req, &["signal-parent"]);
        allowed(req, &["write", "/dev/null"]);
        allowed(req, &["read", "/dev/urandom"]);
        denied(req, &["write", "/dev/random"]);
    }
    // A shell in its workspace (`getcwd` needs its ancestors' metadata) signals its child.
    let out = macos_support::launch_at(
        Some(&t.p("p/workspace/s1")),
        &serde_json::to_vec(&agent).unwrap(),
        &["/bin/sh", "-c", "pwd; sleep 30 & kill $!; wait $!; echo $?"],
        &[],
    );
    let text = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [t.ps("p/workspace/s1").as_str(), "143"],
        "{out:?}"
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("not permitted"),
        "{out:?}"
    );
    // The agent's CLIs (spike S9 B6): the preferences daemon's and notifyd's shared memory,
    // kernel-control sockets. A hook gets none of them.
    for name in ["apple.cfprefs.daemonv1", "apple.shm.notification_center"] {
        allowed(&agent, &["shm-open", name]);
        denied(&hook, &["shm-open", name]);
    }
    allowed(&agent, &["system-socket"]);
    denied(&hook, &["system-socket"]);
    allowed(&agent, &["mach-lookup", "com.apple.cfprefsd.daemon"]);
    allowed(&agent, &["mach-lookup", "com.apple.cfprefsd.agent"]);
}

#[test]
fn name_resolution_is_the_agents_only() {
    // DNS goes through mDNSResponder's socket (spike S9 B6); `hook` has no network at all.
    let t = Tree::new();
    let mdns = fin_sandbox::macos::sbpl::MDNS_SOCKET;
    allowed(&t.agent(Harness::Codex, None), &["connect-unix", mdns]);
    denied(&t.hook(None), &["connect-unix", mdns]);
}

#[test]
fn agent_cannot_start_anything_outside_the_sandbox() {
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    let marker = t.p("q/osascript-ran");
    let label = format!("dev.finisher.test.{}", std::process::id());
    let terminals = || pids("Terminal");
    let before = terminals();
    let osa = format!("do shell script \"touch '{}'\"", s(&marker));
    let cases: [(&[&str], &str); 4] = [
        (&["/usr/bin/open", "-a", "Terminal"], "open -a Terminal"),
        (&["/usr/bin/open", "https://example.invalid/"], "open <url>"),
        (&["/usr/bin/osascript", "-e", &osa], "osascript -e"),
        (
            &[
                "/bin/launchctl",
                "submit",
                "-l",
                &label,
                "--",
                "/usr/bin/true",
            ],
            "launchctl submit",
        ),
    ];
    for (argv, what) in cases {
        exec_refused(&launch_program(&req, argv, &[]), what);
    }
    // The controls: each tool runs unsandboxed (harmless arguments: nothing is started).
    let runs = |prog: &str, args: &[&str]| {
        let out = Command::new(prog).args(args).output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    assert!(
        runs("/usr/bin/open", &["-h"]).contains("Usage"),
        "open runs"
    );
    assert_eq!(runs("/usr/bin/osascript", &["-e", "return 7"]).trim(), "7");
    assert!(
        !runs("/bin/launchctl", &["version"]).is_empty(),
        "launchctl runs"
    );
    // Nothing new ran outside the sandbox.
    assert!(!marker.exists(), "osascript's command ran");
    let listed = Command::new("/bin/launchctl")
        .args(["list", &label])
        .output()
        .unwrap();
    assert!(!listed.status.success(), "a launchd job {label} was added");
    let after = terminals();
    assert!(
        after.iter().all(|p| before.contains(p)),
        "a Terminal started: {before:?} → {after:?}"
    );
}

/// The pids of the user's processes named `name`.
fn pids(name: &str) -> Vec<u32> {
    let out = Command::new("/usr/bin/pgrep")
        .args(["-x", "-U", &unsafe { libc::getuid() }.to_string(), name])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect()
}

#[test]
fn only_the_mach_allow_list_is_reachable() {
    let t = Tree::new();
    let req = t.agent(Harness::Codex, None);
    // Allowed by `mach-agent.txt`.
    allowed(
        &req,
        &["mach-lookup", "com.apple.system.opendirectoryd.libinfo"],
    );
    // Services that would act for the agent outside the sandbox. The pasteboard is the case
    // only the mach allow-list protects (spike S9 B6: `(deny default)` refuses `lsopen` and
    // setuid exec by itself, so `open` fails even with every lookup allowed).
    for service in [
        "com.apple.pasteboard.1",
        "com.apple.lsd.mapdb",
        "com.apple.SecurityServer",
        "com.apple.coreservices.launchservicesd",
        "com.apple.coreservices.appleevents",
        "com.apple.tccd",
        "com.apple.windowserver.active",
    ] {
        denied(&req, &["mach-lookup", service]);
    }
}

#[test]
fn another_applications_keychain_item_is_unreadable() {
    let t = Tree::new();
    let kc = t.p("home/Library/Keychains/test.keychain-db");
    let kcs = s(&kc);
    let sec = |args: &[&str]| {
        Command::new("/usr/bin/security")
            .args(args)
            .output()
            .unwrap()
    };
    assert!(
        sec(&["create-keychain", "-p", "fin-test", &kcs])
            .status
            .success()
    );
    let _cleanup = Cleanup(kc.clone());
    // An item whose access list names no application (`-T ""`).
    let add = sec(&[
        "add-generic-password",
        "-a",
        "fin",
        "-s",
        "fin-sandbox-test",
        "-w",
        "s3cret",
        "-T",
        "",
        &kcs,
    ]);
    assert!(add.status.success(), "{add:?}");
    // The control: unsandboxed, the system tool finds the item (its attributes; no prompt).
    assert!(
        sec(&["find-generic-password", "-s", "fin-sandbox-test", &kcs])
            .status
            .success()
    );
    let req = t.agent(Harness::Codex, None);
    let out = launch_program(
        &req,
        &[
            "/usr/bin/security",
            "find-generic-password",
            "-s",
            "fin-sandbox-test",
            "-w",
            &kcs,
        ],
        &[],
    );
    exec_refused(&out, "security find-generic-password");
    assert!(!String::from_utf8_lossy(&out.stdout).contains("s3cret"));
    // Both layers on their own: the keychain files and the Security server.
    denied(&req, &["read", &kcs]);
    denied(&req, &["list", &t.ps("home/Library/Keychains")]);
    denied(&req, &["mach-lookup", "com.apple.SecurityServer"]);
    let real = std::env::var_os("HOME").map(|h| Path::new(&h).join("Library/Keychains"));
    if let Some(real) = real.filter(|p| p.is_dir()) {
        denied(&req, &["list", &s(&real)]);
    }
}

/// Deletes the test keychain (which `create-keychain` with a path leaves off the search list).
struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = Command::new("/usr/bin/security")
            .args(["delete-keychain", &s(&self.0)])
            .output();
    }
}

#[test]
fn git_works_under_the_session_configuration() {
    let Some(dev) = developer_dir() else {
        eprintln!("git_works_under_the_session_configuration: skipped, no developer directory");
        return;
    };
    let t = Tree::new();
    let git = dev.join("usr/bin/git");
    let cfg = t.p("data/agents/s1/git/config");
    std::fs::write(&cfg, fin_sandbox::data::session_git_config()).unwrap();
    let mut input = t.agent_input(Harness::Codex, None);
    input.toolchains = Tree::toolchains(std::slice::from_ref(&dev));
    let req = LaunchRequest::Agent(input);
    let ws = t.ps("p/workspace/s1");
    let tmp = t.ps("data/agents/s1/tmp");
    let home = t.ps("home");
    let env = [
        ("GIT_CONFIG_GLOBAL", s(&cfg)),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        ("HOME", home),
        ("USER", "agent".into()),
        ("LOGNAME", "agent".into()),
        ("SHELL", "/bin/sh".into()),
        ("TMPDIR", tmp),
        ("PATH", format!("{}:/usr/bin:/bin", s(&dev.join("usr/bin")))),
    ];
    let env: Vec<(&str, &str)> = env.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let g = s(&git);
    let run = |args: &[&str]| {
        let mut argv = vec![g.as_str(), "-C", ws.as_str()];
        argv.extend_from_slice(args);
        let out = launch_program(&req, &argv, &env);
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    write(&t.p("p/workspace/s1/a.txt"), "a");
    run(&["init", "-q"]);
    run(&["add", "a.txt"]);
    run(&[
        "-c",
        "user.name=agent",
        "-c",
        "user.email=agent@invalid",
        "commit",
        "-q",
        "-m",
        "x",
    ]);
    run(&["status", "--porcelain"]);
}

/// The active developer directory (`xcode-select -p`), whose `usr/bin/git` is the real binary
/// (the `/usr/bin` shim may run `xcodebuild`, which the sandbox refuses: spike S9 B6).
fn developer_dir() -> Option<PathBuf> {
    let out = Command::new("/usr/bin/xcode-select")
        .arg("-p")
        .output()
        .ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    let p = std::fs::canonicalize(p).ok()?;
    p.join("usr/bin/git").is_file().then_some(p)
}

#[test]
fn hook_has_no_ip_and_reads_the_project_only() {
    let t = Tree::new();
    let (_l, port) = listener();
    let req = t.hook(None);
    allowed(&req, &["read", &t.ps("p/CLAUDE.md")]);
    allowed(&req, &["create", &t.ps("hook-tmp/x")]);
    denied(&req, &["write", &t.ps("p/CLAUDE.md")]);
    denied(&req, &["create", &t.ps("p/new.txt")]);
    denied(&req, &["connect-tcp", &port.to_string()]);
    denied(&req, &["read", &t.ps("data/auth/local.key")]);
    denied(&req, &["connect-unix", &t.ps("data/run/fin.sock")]);
    // A hook that declares `fin` reaches `fin.sock` and reads its credential, nothing else.
    write(&t.p("data/agents/s1/hook-credential.json"), "{}");
    let req = t.hook(Some(HookFin {
        credential: t.p("data/agents/s1/hook-credential.json"),
        endpoint: FinEndpoint::UnixSocket {
            path: t.p("data/run/fin.sock"),
        },
    }));
    allowed(&req, &["connect-unix", &t.ps("data/run/fin.sock")]);
    allowed(
        &req,
        &["read", &t.ps("data/agents/s1/hook-credential.json")],
    );
    denied(&req, &["connect-tcp", &port.to_string()]);
    denied(&req, &["read", &t.ps("data/agents/s1/credential.json")]);
}

#[test]
fn a_hostile_path_name_is_granted_exactly() {
    let t = Tree::new();
    let odd = t.p("lib/a \"q\" \\b (p) ;s");
    write(&odd.join("f"), "f");
    write(&t.p("lib/a/f"), "g");
    let mut input = t.agent_input(Harness::Codex, None);
    input.library_roots = vec![odd.clone()];
    let req = LaunchRequest::Agent(input);
    allowed(&req, &["read", &s(&odd.join("f"))]);
    denied(&req, &["read", &t.ps("lib/a/f")]);
    denied(&req, &["write", &s(&odd.join("f"))]);
}

#[test]
fn the_worker_shape_cannot_fork_exec_or_reach_the_network() {
    let (_d, t) = tempdir();
    for d in ["data/jobs/j1/in", "data/jobs/j1/out", "data/jobs/j1/tmp"] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    let input = fin_sandbox::WorkerInput {
        tmp: t.join("data/jobs/j1/tmp"),
        caches: vec![],
        devices: vec![],
        server_data: t.join("data"),
        scope: fin_sandbox::WorkerScope::Cold {
            job_dir: t.join("data/jobs/j1"),
            sources: vec![],
        },
    };
    let policy = for_os::worker(Os::Macos, &input).unwrap();
    let (_l, port) = listener();
    assert_denied(&t, &policy, &["fork"]);
    macos_support::assert_allowed(&t, &policy, &["signal-self"]);
    macos_support::assert_allowed(&t, &policy, &["list", "/"]);
    assert_denied(&t, &policy, &["connect-tcp", &port.to_string()]);
    assert_denied(
        &t,
        &policy,
        &["mach-lookup", "com.apple.system.opendirectoryd.libinfo"],
    );
    // `exec` replaces the probe when it works, so its control is that the tool runs.
    let (sandboxed, _) = macos_support::both(&t, &policy, &["exec", "/usr/bin/true"]);
    assert!(sandboxed.denied(), "exec: {sandboxed:?}");
    assert!(Command::new("/usr/bin/true").status().unwrap().success());
}

#[test]
fn the_plugin_shape_keeps_out_of_personal_folders_and_apple_events() {
    let t = Tree::new();
    let mut input = t.plugin();
    std::fs::create_dir_all(&input.store_dir).unwrap();
    std::fs::create_dir_all(input.call_dir.join("in")).unwrap();
    std::fs::create_dir_all(input.call_dir.join("out")).unwrap();
    std::fs::create_dir_all(&input.tmp).unwrap();
    let real_home = std::env::var_os("HOME").map(PathBuf::from);
    if let Some(h) = &real_home {
        input.home = std::fs::canonicalize(h).unwrap();
    }
    let policy = for_os::plugin(Os::Macos, &input).unwrap();
    let (_l, port) = listener();
    let dir = &t.t;
    assert_denied(dir, &policy, &["read", &t.ps("data/auth/local.key")]);
    assert_denied(dir, &policy, &["read", &t.ps("p/CLAUDE.md")]);
    assert_denied(dir, &policy, &["connect-tcp", &port.to_string()]);
    assert_denied(dir, &policy, &["connect-unix", &t.ps("data/run/fin.sock")]);
    assert_denied(
        dir,
        &policy,
        &["mach-lookup", "com.apple.coreservices.appleevents"],
    );
    assert_denied(dir, &policy, &["mach-lookup", "com.apple.tccd"]);
    assert_denied(dir, &policy, &["create", &s(&input.call_dir.join("in/x"))]);
    // TCC-protected folders: refused by the profile's own rules, before TCC could prompt.
    if let Some(h) = &input.home.to_str().map(PathBuf::from) {
        for folder in ["Documents", "Desktop"] {
            let p = h.join(folder);
            if p.is_dir() {
                let started = std::time::Instant::now();
                let (got, _) = macos_support::both(dir, &policy, &["list", &s(&p)]);
                assert!(got.denied(), "{folder}: {got:?}");
                assert!(started.elapsed().as_secs() < 10, "{folder}: a prompt?");
            }
        }
    }
    // With the test tree as home, the same folders (readable unsandboxed) are denied.
    let mut fake = t.plugin();
    fake.home = t.p("home");
    let policy = for_os::plugin(Os::Macos, &fake).unwrap();
    assert_denied(dir, &policy, &["read", &t.ps("home/Documents/letter.txt")]);
    assert_denied(dir, &policy, &["read", &t.ps("home/Desktop/note.txt")]);
}
