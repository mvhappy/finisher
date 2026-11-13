//! Unix sockets hidden by a namespace (plan 7C Decision 26, Task 5b): `fin-sandbox-exec`
//! puts `agent` and `hook` processes in a user and mount namespace where `/run`, `/tmp`,
//! `/var/tmp`, `/dev/shm`, `$XDG_RUNTIME_DIR`, the home directory and the policy's denied
//! directories are empty, except for the policy's granted paths, drops every capability, and
//! removes the socket variables from the environment. The test tree lives under
//! `CARGO_TARGET_TMPDIR`, outside the fixed hidden roots, so the home and denied-directory
//! rules each have a case that only they decide. A hidden socket is
//! a denial only after an unsandboxed control connected to it, and the denial is `ENOENT`
//! (the path does not exist in the namespace). Hosts that refuse user namespaces (Ubuntu's
//! AppArmor restriction without Finisher's profile) skip the "available" cases with a printed
//! reason; `FIN_REQUIRE_NAMESPACES=1` makes them fail instead.
#![cfg(target_os = "linux")]

mod linux_support;

use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::launch::{LaunchRequest, SOCKET_ENV};
use fin_sandbox::{AgentInput, FinEndpoint, HookFin, HookInput, Namespaces, Os};
use linux_support::{
    LAUNCHER, Outcome, PROBE, landlock_or_skip, launch_raw, launcher_namespaces,
    namespaces_or_skip, probe, s, write,
};

const NS_OFF: (&str, &str) = ("FIN_TEST_SANDBOX_NAMESPACES", "unavailable");

/// A project, server data and a home under one temporary root, and a runtime directory, both
/// outside every fixed hidden root.
struct Tree {
    _d: tempfile::TempDir,
    _x: tempfile::TempDir,
    t: PathBuf,
    xdg: PathBuf,
}

impl Tree {
    fn new() -> Tree {
        let _d = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let t = std::fs::canonicalize(_d.path()).unwrap();
        let _x = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
        let xdg = std::fs::canonicalize(_x.path()).unwrap();
        for (p, text) in [
            ("data/auth/local.key", "secret"),
            ("data/agents/s1/credential.json", "{}"),
            ("data/agents/s1/tmp/note", "n"),
            ("p/sources/clip.mp4", "c"),
            ("p/hooks/check.sh", "#!/bin/sh\n"),
        ] {
            write(&t.join(p), text);
        }
        for d in [
            "p/workspace/s1",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "home/.ssh",
            "home/.cache",
            "data/run",
            "hook-tmp",
        ] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        write(&xdg.join("granted/resolv.conf"), "nameserver 127.0.0.53\n");
        Tree { _d, _x, t, xdg }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel)
    }

    /// The base toolchain, the probe, and one granted file inside the runtime directory (as
    /// `/etc/resolv.conf` resolves into `/run`).
    fn toolchains(&self) -> Vec<PathBuf> {
        let mut v = fin_sandbox::data::toolchain_base_resolved(Os::Linux);
        v.push(std::fs::canonicalize(PROBE).unwrap());
        v.push(self.xdg.join("granted/resolv.conf"));
        v
    }

    fn agent(&self) -> LaunchRequest {
        LaunchRequest::Agent(AgentInput {
            session_id: "s1".into(),
            mode: AgentMode::Draft,
            harness: Harness::Codex,
            project_dir: self.p("p"),
            workspace_dir: self.p("p/workspace/s1"),
            shared_dir: None,
            library_roots: vec![],
            session_dir: self.p("data/agents/s1"),
            sign_in_store: None,
            toolchains: self.toolchains(),
            server_data: self.p("data"),
            home: self.p("home"),
            fin: None,
        })
    }

    fn hook(&self, fin: Option<HookFin>) -> LaunchRequest {
        LaunchRequest::Hook(HookInput {
            project_dir: Some(self.p("p")),
            hook_file: self.p("p/hooks/check.sh"),
            toolchains: self.toolchains(),
            tmp: self.p("hook-tmp"),
            server_data: self.p("data"),
            home: self.p("home"),
            fin,
            timeout_ms: None,
        })
    }

    /// Runs the probe's `op` through the launcher, with `$XDG_RUNTIME_DIR` set to the tree's.
    fn launched(&self, req: &LaunchRequest, op: &[&str], env: &[(&str, &str)]) -> Outcome {
        let bytes = serde_json::to_vec(req).unwrap();
        let mut args = vec!["--policy-fd", "3", "--", PROBE, "--none"];
        args.extend(op);
        let xdg = s(&self.xdg);
        let mut e = vec![("XDG_RUNTIME_DIR", xdg.as_str())];
        e.extend(env);
        let out = launch_raw(Some(&bytes), &args, &e);
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        match serde_json::from_str::<serde_json::Value>(stdout.trim()) {
            Ok(v) if out.status.success() && v.get("op").is_some() => {
                Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
            }
            _ => Outcome::Other {
                code: out.status.code(),
                stdout: format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)),
            },
        }
    }
}

fn control(op: &[&str]) {
    let o: Vec<String> = op.iter().map(|x| x.to_string()).collect();
    assert_eq!(
        probe(&["--none".into()], &o),
        Outcome::Ran(None),
        "control {op:?}"
    );
}

/// A socket for the `/run` case where this user can make one: `/run/user/<uid>` (a desktop
/// session's runtime directory) or `/run` itself (root, the privileged-container run).
fn run_socket() -> Option<(PathBuf, UnixListener)> {
    // SAFETY: getuid cannot fail.
    let uid = unsafe { libc::getuid() };
    let name = format!("fin-ns-test-{}.sock", std::process::id());
    for dir in [format!("/run/user/{uid}"), "/run".into()] {
        let p = Path::new(&dir).join(&name);
        let _ = std::fs::remove_file(&p);
        if let Ok(l) = UnixListener::bind(&p) {
            return Some((p, l));
        }
    }
    None
}

struct Unlink(PathBuf);
impl Drop for Unlink {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A socket in `dir` named for this test process, removed on drop.
fn socket_in(dir: &str, test: &str) -> Option<(Unlink, UnixListener)> {
    let p = Path::new(dir).join(format!("fin-ns-{test}-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&p);
    UnixListener::bind(&p).ok().map(|l| (Unlink(p), l))
}

#[test]
fn sockets_in_every_hidden_root_are_hidden() {
    let test = "sockets_in_every_hidden_root_are_hidden";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    // The runtime directory, the home directory (not a sensitive path: only the home rule
    // hides it) and server data (only the denied-directory rule hides it: `fin.sock`).
    let mut socks = vec![
        t.xdg.join("bus"),
        t.p("home/.cache/ide.sock"),
        t.p("data/run/fin.sock"),
    ];
    let mut _keep: Vec<UnixListener> = socks
        .iter()
        .map(|p| UnixListener::bind(p).unwrap())
        .collect();
    let mut _unlink = Vec::new();
    for dir in ["/tmp", "/var/tmp", "/dev/shm"] {
        let (u, l) = socket_in(dir, "fixed").unwrap_or_else(|| panic!("a socket in {dir}"));
        socks.push(u.0.clone());
        _keep.push(l);
        _unlink.push(u);
    }
    match run_socket() {
        Some((p, l)) => {
            socks.push(p.clone());
            _keep.push(l);
            _unlink.push(Unlink(p));
        }
        None => {
            eprintln!("{test}: no socket under /run can be made by this user; that case is skipped")
        }
    }
    for sock in &socks {
        control(&["connect-unix", &s(sock)]);
        for req in [t.agent(), t.hook(None)] {
            let got = t.launched(&req, &["connect-unix", &s(sock)], &[]);
            assert_eq!(got, Outcome::Ran(Some(libc::ENOENT)), "{}", sock.display());
            // The same launch without the namespace reaches it: the namespace is what hides it.
            let got = t.launched(&req, &["connect-unix", &s(sock)], &[NS_OFF]);
            assert_eq!(
                got,
                Outcome::Ran(None),
                "{} without the namespace",
                sock.display()
            );
        }
    }
}

#[test]
fn no_capability_survives_into_the_program() {
    let test = "no_capability_survives_into_the_program";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    let sock = t.p("home/.cache/ide.sock");
    let _l = UnixListener::bind(&sock).unwrap();
    let home = s(&t.p("home"));
    // `open_tree` copies the mounts below a directory as they are under any mount on top:
    // with the namespace's capabilities (a program started as root) it would reach the
    // hidden socket. Control: root can do it unsandboxed; another user has no capability.
    // SAFETY: getuid cannot fail.
    if unsafe { libc::getuid() } == 0 {
        control(&["open-tree", &s(&t.t), "home/.cache/ide.sock"]);
    }
    let got = t.launched(
        &t.agent(),
        &["open-tree", &s(&t.t), "home/.cache/ide.sock"],
        &[],
    );
    assert_eq!(
        got,
        Outcome::Ran(Some(libc::EPERM)),
        "open_tree of {home}'s parent"
    );
    // No nested user namespace either (the per-namespace limit is 0).
    let got = t.launched(&t.agent(), &["unshare-user"], &[]);
    assert!(
        matches!(got, Outcome::Ran(Some(e)) if e == libc::ENOSPC || e == libc::EPERM),
        "{got:?}"
    );
}

#[test]
fn a_hidden_working_directory_is_left_for_the_root() {
    let test = "a_hidden_working_directory_is_left_for_the_root";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    let dir = t.p("home/.cache");
    let _l = UnixListener::bind(dir.join("x.sock")).unwrap();
    let run = |env: &[(&str, &str)]| {
        let bytes = serde_json::to_vec(&t.agent()).unwrap();
        let (mut cmd, keep) = linux_support::launch_cmd(
            Some(&bytes),
            &[
                "--policy-fd",
                "3",
                "--",
                PROBE,
                "--none",
                "connect-unix",
                "x.sock",
            ],
            env,
        );
        cmd.current_dir(&dir);
        let out = cmd.output().unwrap();
        drop(keep);
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_default();
        v["errno"].as_i64()
    };
    // Control: without the namespace the relative path reaches the socket.
    assert_eq!(run(&[NS_OFF]), None);
    // With it, the hidden directory is left for `/`, where `x.sock` does not exist.
    assert_eq!(run(&[]), Some(libc::ENOENT as i64));
}

#[test]
fn granted_paths_under_hidden_roots_keep_their_content() {
    let test = "granted_paths_under_hidden_roots_keep_their_content";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    // A socket the user made inside the project (a granted root) stays reachable: the
    // namespace hides locations, Landlock decides what is inside the grants.
    let own = t.p("p/workspace/s1/own.sock");
    let _l = UnixListener::bind(&own).unwrap();
    let agent = t.agent();
    for (op, path) in [
        ("read", t.xdg.join("granted/resolv.conf")),
        ("read", t.p("p/sources/clip.mp4")),
        ("read", PathBuf::from("/dev/null")),
        // The session's directory in server data, which the denied-directory rule hides.
        ("read", t.p("data/agents/s1/tmp/note")),
        ("connect-unix", own.clone()),
    ] {
        let got = t.launched(&agent, &[op, &s(&path)], &[]);
        assert_eq!(got, Outcome::Ran(None), "{op} {}", path.display());
    }
    // A sibling of the granted file in the hidden runtime directory is gone.
    write(&t.xdg.join("granted-not.txt"), "x");
    control(&["read", &s(&t.xdg.join("granted-not.txt"))]);
    let got = t.launched(&agent, &["read", &s(&t.xdg.join("granted-not.txt"))], &[]);
    assert_eq!(got, Outcome::Ran(Some(libc::ENOENT)));
    // Writes still land in the session's roots, on the real filesystem.
    let made = t.p("p/workspace/s1/made.txt");
    assert_eq!(
        t.launched(&agent, &["create", &s(&made)], &[]),
        Outcome::Ran(None)
    );
    assert!(made.exists());
    // Server data stays denied (Landlock answers inside the bound-back tree's parents too).
    let got = t.launched(
        &agent,
        &["read", &t.p("data/auth/local.key").to_string_lossy()],
        &[],
    );
    assert!(
        matches!(got, Outcome::Ran(Some(e)) if e == libc::EACCES || e == libc::ENOENT),
        "{got:?}"
    );
}

#[test]
fn the_parents_root_does_not_reach_a_hidden_socket() {
    let test = "the_parents_root_does_not_reach_a_hidden_socket";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    let sock = t.xdg.join("bus");
    let _l = UnixListener::bind(&sock).unwrap();
    // The launched process's parent is this test, outside the namespace. Landlock's ptrace
    // scoping refuses its `/proc/<pid>/root` (EACCES): a guard, not the namespace's doing.
    let via = format!("/proc/{}/root{}", std::process::id(), s(&sock));
    control(&["connect-unix", &via]);
    let got = t.launched(&t.agent(), &["connect-unix", &via], &[]);
    assert!(
        matches!(got, Outcome::Ran(Some(e)) if e == libc::EACCES || e == libc::ENOENT),
        "{got:?}"
    );
}

#[test]
fn a_hook_without_network_has_its_own_network_namespace() {
    let test = "a_hook_without_network_has_its_own_network_namespace";
    if landlock_or_skip(test).is_none() || !namespaces_or_skip(test) {
        return;
    }
    let t = Tree::new();
    let udp = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    udp.set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let port = udp.local_addr().unwrap().port().to_string();
    control(&["send-udp", &port]);
    let mut b = [0u8; 4];
    assert_eq!(udp.recv(&mut b).unwrap(), 1);
    // Only a downed `lo`: nothing reaches the host's loopback, so UDP, an ephemeral listener
    // and abstract sockets are the hook's own.
    let got = t.launched(&t.hook(None), &["send-udp", &port], &[]);
    assert_eq!(got, Outcome::Ran(Some(libc::ENETUNREACH)));
    // A hook that declares `fin` keeps the host's network (it needs the agent listener).
    let (_tcp, fin) = linux_support::listener();
    let with_fin = t.hook(Some(HookFin {
        credential: t.p("data/agents/s1/credential.json"),
        endpoint: FinEndpoint::Tcp { port: fin },
    }));
    assert_eq!(
        t.launched(&with_fin, &["send-udp", &port], &[]),
        Outcome::Ran(None)
    );
    assert_eq!(udp.recv(&mut b).unwrap(), 1);
}

#[test]
fn socket_variables_are_removed_in_every_mode() {
    let t = Tree::new();
    let mut env: Vec<(&str, &str)> = SOCKET_ENV.iter().map(|v| (*v, "/tmp/x.sock")).collect();
    env.push(("PATH", "/usr/bin:/bin"));
    env.push(("FIN_KEEP_ME", "yes"));
    let req = serde_json::to_vec(&t.agent()).unwrap();
    for extra in [
        vec![],
        vec![NS_OFF],
        vec![("FIN_TEST_SANDBOX_STATUS", "disabled")],
    ] {
        let mut e = env.clone();
        e.extend(extra.iter().copied());
        let out = launch_raw(Some(&req), &["--policy-fd", "3", "--", "/usr/bin/env"], &e);
        assert!(out.status.success(), "{extra:?}: {out:?}");
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("FIN_KEEP_ME=yes"), "{extra:?}: {text}");
        for v in SOCKET_ENV {
            assert!(
                !text.lines().any(|l| l.starts_with(&format!("{v}="))),
                "{v} with {extra:?}"
            );
        }
    }
}

#[test]
fn without_namespaces_landlock_alone_applies_and_probe_says_so() {
    let test = "without_namespaces_landlock_alone_applies_and_probe_says_so";
    assert_eq!(
        launcher_namespaces(&[NS_OFF]),
        Namespaces::Unavailable {
            reason: "stubbed".into()
        }
    );
    if landlock_or_skip(test).is_none() {
        return;
    }
    let t = Tree::new();
    let sock = t.xdg.join("bus");
    let _l = UnixListener::bind(&sock).unwrap();
    // The residual the warning names: the socket is reachable…
    assert_eq!(
        t.launched(&t.agent(), &["connect-unix", &s(&sock)], &[NS_OFF]),
        Outcome::Ran(None)
    );
    // …and Landlock still applies.
    let key = s(&t.p("data/auth/local.key"));
    assert_eq!(
        t.launched(&t.agent(), &["read", &key], &[NS_OFF]),
        Outcome::Ran(Some(libc::EACCES))
    );
}

#[test]
fn the_probe_answers_on_this_host() {
    // Whatever the host allows, `--probe` answers with a reason when it cannot.
    match launcher_namespaces(&[]) {
        Namespaces::Available => {}
        Namespaces::Unavailable { reason } => assert!(!reason.is_empty()),
        Namespaces::NotApplicable => panic!("Linux is applicable"),
    }
    let _ = LAUNCHER;
}

#[test]
fn the_apparmor_profile_parses() {
    let profile = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../resources/linux/apparmor/finisher-sandbox-exec"
    );
    let text = std::fs::read_to_string(profile).unwrap();
    assert!(
        text.contains("userns,"),
        "the profile allows user namespaces"
    );
    let Ok(parser) = which("apparmor_parser") else {
        eprintln!(
            "the_apparmor_profile_parses: apparmor_parser is not installed; syntax unchecked"
        );
        return;
    };
    // Parsed only: `-Q` skips the kernel load, `-K` the cache. Nothing is loaded.
    let out = std::process::Command::new(parser)
        .args(["-Q", "-K", profile])
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
}

fn which(name: &str) -> Result<PathBuf, ()> {
    for dir in ["/usr/sbin", "/sbin", "/usr/bin", "/bin"] {
        let p = Path::new(dir).join(name);
        if p.exists() {
            return Ok(p);
        }
    }
    Err(())
}
