//! The escape suites as one runner (plan 7C Task 13, spec §8.11): each case file of `cases/`
//! (`server_data.json`, `sessions.json`, `links_mounts.json`, `agent-claude.json`) runs per profile on this OS
//! through `fin-sandbox-probe`, the way each profile is applied in the product:
//! - `agent` and `hook`: the real launcher, `fin-sandbox-exec --policy-fd 3 -- probe --none`;
//! - `worker`: `probe --self`, the self-sandbox entry `fin-workers` calls before `hello`;
//! - `plugin`: `probe --policy` with the profile's policy (the `Spawner` applies the same one
//!   before `exec`).
//!
//! A `deny` case passes only on an observed denial, and only if the same attempt then succeeds
//! unsandboxed (the control), each run on a fresh fixture, so a missing file never passes. A
//! case whose control cannot run here (`control: root` as another user, `control: none`) is
//! recorded as `uncontrolled`, never as `denied`. On
//! Linux the launcher's socket namespace is off (Landlock alone, as in `escape_agent_linux.rs`)
//! except for cases that `need` it, which are skipped with the launcher's reason on a host
//! without unprivileged user namespaces. `FIN_REQUIRE_LANDLOCK=1` turns a missing Landlock
//! into a failure, `FIN_REQUIRE_NAMESPACES=1` a missing namespace.
//!
//! On Windows every case is skipped (plan 7C Decision 27: the suites prove the configuration,
//! `container_windows.rs`; escape attempts are left to a person). The hooks for a Windows
//! backend are [`Backend::Windows`] and the `windows` names in the case files' `os` lists.
//!
//! `FIN_ESCAPE_REPORT=<dir>` writes `<dir>/<case file>.json`, one record per case and profile,
//! which `scripts/sandbox-suite.mjs` turns into its table.

#![cfg_attr(windows, allow(dead_code))]

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct File {
    cases: Vec<Case>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Case {
    name: String,
    profiles: Vec<String>,
    op: Vec<String>,
    expect: Expect,
    /// The OSes the case applies to (default: every one).
    #[serde(default)]
    os: Option<Vec<String>>,
    /// `namespaces`: the launcher's socket namespace (Linux, Decision 26). `unixSockets`:
    /// Landlock ABI 9 (kernel 6.15), which refuses a pathname socket not listed (Linux).
    #[serde(default)]
    needs: Option<String>,
    /// The `agent` profile's harness: `codex` (default) or `claudeCode` (plan 7D Task 14).
    #[serde(default)]
    harness: Option<String>,
    /// `required` (default for `deny`), `root` (only as root) or `none`.
    #[serde(default)]
    control: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum Expect {
    Word(String),
    Errno { errno: Vec<String> },
}

/// One case's result for one profile.
#[derive(Debug, Clone, serde::Serialize)]
struct Record {
    os: &'static str,
    file: String,
    case: String,
    profile: String,
    /// `denied`, `uncontrolled` (denied, no control), `allowed`, `skipped` or `failed`.
    result: &'static str,
    detail: String,
}

/// What one probe run showed.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(windows, allow(dead_code))]
enum Outcome {
    Ran(Option<i32>),
    Apply(String),
    Other { code: Option<i32>, out: String },
}

#[cfg(target_os = "linux")]
const OS: &str = "linux";
#[cfg(target_os = "macos")]
const OS: &str = "macos";
#[cfg(windows)]
const OS: &str = "windows";

/// The errnos a denial may show on this OS (`ENOENT` never is one).
#[cfg(target_os = "linux")]
const DENIALS: &[i32] = &[libc::EACCES, libc::EXDEV];
#[cfg(target_os = "macos")]
const DENIALS: &[i32] = &[libc::EPERM, libc::EACCES];

/// How this OS applies the profiles, or why it cannot here.
enum Backend {
    #[cfg(unix)]
    Unix,
    #[allow(dead_code)]
    Skip(String),
    #[cfg(windows)]
    #[allow(dead_code)]
    Windows,
}

fn backend() -> Backend {
    #[cfg(target_os = "linux")]
    {
        use fin_sandbox::linux::landlock::{Support, query};
        match query() {
            Support::Abi(_) => Backend::Unix,
            other => {
                assert_ne!(
                    std::env::var("FIN_REQUIRE_LANDLOCK").as_deref(),
                    Ok("1"),
                    "FIN_REQUIRE_LANDLOCK=1 but Landlock answers {other:?}"
                );
                Backend::Skip(format!("Landlock is not available ({other:?})"))
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        Backend::Unix
    }
    #[cfg(windows)]
    {
        Backend::Skip(
            "Windows escape attempts are left to a person (plan 7C Decision 27); \
             tests/container_windows.rs proves the configuration"
                .into(),
        )
    }
}

/// The fixture every case file runs on.
struct Tree {
    _d: tempfile::TempDir,
    t: PathBuf,
    #[cfg(unix)]
    _socks: Vec<std::os::unix::net::UnixListener>,
}

/// The server-data files no sandboxed process may read (spec §8.11 "Server data").
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

/// The links each profile's write root holds to `auth/local.key` (only names: reading
/// through one is denied at the target).
const PLANTED: &[&str] = &[
    "p/workspace/s1/to-key",
    "hook-tmp/to-key",
    "data/jobs/j1/tmp/to-key",
    "data/jobs/p1/tmp/to-key",
];

impl Tree {
    fn new() -> Tree {
        let d = tempfile::tempdir().unwrap();
        let t = std::fs::canonicalize(d.path()).unwrap();
        let mut files: Vec<(&str, &str)> = SERVER_DATA.iter().map(|p| (*p, "secret")).collect();
        files.extend([
            ("data/providers/codex/home/auth.json", "{}"),
            ("data/agents/s1/credential.json", "{}"),
            ("data/agents/s1/settings/settings.json", "{}"),
            ("data/agents/s1/blobs/b1", "b"),
            ("data/agents/s2/credential.json", "{}"),
            ("data/agents/s2/provider/state.json", "{}"),
            ("data/agents/s2/tmp/note", "n"),
            ("data/jobs/j1/in/manifest.json", "{}"),
            ("data/jobs/p1/in/request.json", "{}"),
            ("data/plugins/store/acme/plugin.json", "{}"),
            ("p/sources/clip.mp4", "c"),
            ("p/workspace/s1/mine.txt", "m"),
            ("p/workspace/s2/draft.txt", "d"),
            ("p/.finisher/segment", "s"),
            ("p/finisher.project.json", "{}"),
            ("p/hooks/check.sh", "#!/bin/sh\n"),
            // What Claude Code would load by itself (plan 7D Task 14): the project's files, the
            // user's, another session's configuration directory, the sign-in home and its store.
            ("p/CLAUDE.md", "project memory"),
            ("p/CLAUDE.local.md", "local memory"),
            ("p/.claude/settings.json", "{}"),
            ("home/.claude/settings.json", "{}"),
            ("home/.claude.json", "{}"),
            ("data/agents/s2/provider/settings.json", "{}"),
            ("data/providers/claudeCode/home/.claude.json", "{}"),
            (
                "data/providers/claudeCode/home/store/.credentials.json",
                "{}",
            ),
            ("q/other.txt", "o"),
            ("home/.ssh/id_ed25519", "k"),
        ]);
        for (p, text) in files {
            let p = t.join(p);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        for d in [
            "p/workspace/shared",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "data/agents/s1/git",
            "data/jobs/j1/out",
            "data/jobs/j1/tmp",
            "data/jobs/p1/out",
            "data/jobs/p1/tmp",
            "hook-tmp",
            "home/.cache",
        ] {
            std::fs::create_dir_all(t.join(d)).unwrap();
        }
        #[cfg(unix)]
        for l in PLANTED {
            std::os::unix::fs::symlink(t.join("data/auth/local.key"), t.join(l)).unwrap();
        }
        // The socket the open_tree case's root control connects to.
        #[cfg(target_os = "linux")]
        let mut socks =
            vec![std::os::unix::net::UnixListener::bind(t.join("home/.cache/ide.sock")).unwrap()];
        #[cfg(target_os = "macos")]
        let mut socks = vec![];
        // Another session's Claude Code inbox (`<tmp>/cc-socks-<uid>/<pid>.sock`, plan 7D Task 0).
        #[cfg(unix)]
        {
            std::fs::create_dir_all(t.join("inbox/cc-socks-1")).unwrap();
            socks.push(
                std::os::unix::net::UnixListener::bind(t.join("inbox/cc-socks-1/4242.sock"))
                    .unwrap(),
            );
        }
        Tree {
            _d: d,
            t,
            #[cfg(unix)]
            _socks: socks,
        }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.join(rel)
    }

    fn op(&self, case: &Case) -> Vec<String> {
        let t = self.t.to_string_lossy();
        case.op.iter().map(|a| a.replace("$T", &t)).collect()
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use fin_core::events::payloads::agent::{AgentMode, Harness};
    use fin_sandbox::launch::LaunchRequest;
    use fin_sandbox::{AgentInput, HookInput, Os, PluginInput, WorkerInput, WorkerScope};
    use std::process::{Command, Output};

    pub const PROBE: &str = env!("CARGO_BIN_EXE_fin-sandbox-probe");

    fn os() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }

    fn toolchains() -> Vec<PathBuf> {
        let mut v = fin_sandbox::data::toolchain_base_resolved(os());
        v.push(std::fs::canonicalize(PROBE).unwrap());
        v
    }

    fn agent(t: &Tree, harness: Harness) -> LaunchRequest {
        let claude = harness == Harness::ClaudeCode;
        LaunchRequest::Agent(AgentInput {
            session_id: "s1".into(),
            mode: AgentMode::Draft,
            harness,
            project_dir: t.p("p"),
            workspace_dir: t.p("p/workspace/s1"),
            shared_dir: Some(t.p("p/workspace/shared")),
            library_roots: vec![],
            session_dir: t.p("data/agents/s1"),
            sign_in_home: None,
            // A Claude Code store is a directory in the sign-in home (plan 7D Decision 3).
            sign_in_store: Some(if claude {
                t.p("data/providers/claudeCode/home/store")
            } else {
                t.p("data/providers/codex/home/auth.json")
            }),
            toolchains: toolchains(),
            server_data: t.p("data"),
            home: t.p("home"),
            fin: None,
        })
    }

    fn hook(t: &Tree) -> LaunchRequest {
        LaunchRequest::Hook(HookInput {
            project_dir: Some(t.p("p")),
            hook_file: t.p("p/hooks/check.sh"),
            toolchains: toolchains(),
            tmp: t.p("hook-tmp"),
            server_data: t.p("data"),
            home: t.p("home"),
            fin: None,
            timeout_ms: None,
        })
    }

    fn worker(t: &Tree) -> WorkerInput {
        WorkerInput {
            tmp: t.p("data/jobs/j1/tmp"),
            caches: vec![],
            devices: vec![],
            server_data: t.p("data"),
            scope: WorkerScope::Cold {
                job_dir: t.p("data/jobs/j1"),
                sources: vec![t.p("p/sources/clip.mp4")],
            },
        }
    }

    fn plugin(t: &Tree) -> PluginInput {
        PluginInput {
            store_dir: t.p("data/plugins/store/acme"),
            runtime: vec![],
            // The system roots, as `escape_macos.rs` gives them (Seatbelt reads them for the
            // already running probe; Landlock does not need them).
            system: toolchains(),
            onnx_runtime: None,
            models: vec![],
            call_dir: t.p("data/jobs/p1"),
            tmp: t.p("data/jobs/p1/tmp"),
            plugin_data: None,
            gpu: vec![],
            memory_bytes: None,
            server_data: t.p("data"),
            home: t.p("home"),
        }
    }

    fn parse(out: &Output, apply_code: i32) -> Outcome {
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let v: Option<serde_json::Value> = serde_json::from_str(stdout.trim()).ok();
        match (out.status.code(), v) {
            (Some(0), Some(v)) if v.get("op").is_some() => {
                Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
            }
            (Some(c), _) if c == apply_code => {
                Outcome::Apply(format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)))
            }
            (code, _) => Outcome::Other {
                code,
                out: format!("{stdout}{}", String::from_utf8_lossy(&out.stderr)),
            },
        }
    }

    fn json_file(t: &Tree, name: &str, v: &impl serde::Serialize) -> String {
        let p = t.p(name);
        std::fs::write(&p, serde_json::to_vec(v).unwrap()).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// The probe with `flags` and `op`, in an empty environment.
    pub fn probe(flags: &[String], op: &[String]) -> Outcome {
        let out = Command::new(PROBE)
            .args(flags)
            .args(op)
            .env_clear()
            .output()
            .expect("run fin-sandbox-probe");
        parse(&out, 3)
    }

    /// `op` through the launcher under `req`; `namespaces: false` turns the namespace off.
    fn launched(t: &Tree, req: &LaunchRequest, op: &[String], namespaces: bool) -> Outcome {
        let bytes = serde_json::to_vec(req).unwrap();
        let mut argv = vec![PROBE, "--none"];
        argv.extend(op.iter().map(String::as_str));
        #[cfg(target_os = "linux")]
        let out = {
            let env: &[(&str, &str)] = if namespaces {
                &[]
            } else {
                &[(fin_sandbox::linux::namespace::TEST_ENV, "unavailable")]
            };
            let mut args = vec!["--policy-fd", "3", "--"];
            args.extend(argv);
            crate::linux_support::launch_raw(Some(&bytes), &args, env)
        };
        #[cfg(target_os = "macos")]
        let out = {
            let _ = namespaces;
            crate::macos_support::launch(&bytes, &argv, &[])
        };
        let _ = t;
        parse(&out, 78)
    }

    /// `op` under `profile` built from the fixture `t`.
    pub fn attempt(
        t: &Tree,
        case: &Case,
        profile: &str,
        op: &[String],
        namespaces: bool,
    ) -> Outcome {
        let harness = match case.harness.as_deref() {
            None | Some("codex") => Harness::Codex,
            Some("claudeCode") => Harness::ClaudeCode,
            Some(other) => panic!("{}: unknown harness {other:?}", case.name),
        };
        match profile {
            "agent" => launched(t, &agent(t, harness), op, namespaces),
            "hook" => launched(t, &hook(t), op, namespaces),
            "worker" => {
                let req = fin_jobs::sandbox::SelfSandbox {
                    profile: "worker".into(),
                    args: serde_json::to_value(worker(t)).unwrap(),
                };
                probe(&["--self".into(), json_file(t, "self.json", &req)], op)
            }
            "plugin" => {
                let policy = fin_sandbox::profile::plugin(&plugin(t)).expect("plugin policy");
                probe(
                    &["--policy".into(), json_file(t, "policy.json", &policy)],
                    op,
                )
            }
            other => panic!("unknown profile {other:?}"),
        }
    }

    /// Whether the launcher makes its socket namespace here, or why not.
    pub fn namespaces() -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            match crate::linux_support::launcher_namespaces(&[]) {
                fin_sandbox::Namespaces::Available => Ok(()),
                other => {
                    assert_ne!(
                        std::env::var("FIN_REQUIRE_NAMESPACES").as_deref(),
                        Ok("1"),
                        "FIN_REQUIRE_NAMESPACES=1 but the launcher answers {other:?}"
                    );
                    Err(format!(
                        "no unprivileged user namespace for the launcher ({other:?})"
                    ))
                }
            }
        }
        #[cfg(target_os = "macos")]
        Err("no socket namespace on macOS".into())
    }

    /// Whether the kernel refuses a pathname socket the policy does not list, or why not
    /// (Landlock ABI 9; Seatbelt always does).
    pub fn unix_sockets() -> Result<(), String> {
        #[cfg(target_os = "linux")]
        {
            use fin_sandbox::linux::landlock::{Support, query};
            match query() {
                Support::Abi(a) if a.0 >= 9 => Ok(()),
                other => Err(format!(
                    "Landlock below ABI 9 does not scope pathname sockets ({other:?}); \
                     SANDBOX_SOCKETS_EXPOSED"
                )),
            }
        }
        #[cfg(target_os = "macos")]
        Ok(())
    }

    pub fn is_root() -> bool {
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() == 0 }
    }

    pub fn errno(name: &str) -> i32 {
        match name {
            "EPERM" => libc::EPERM,
            "EACCES" => libc::EACCES,
            "EXDEV" => libc::EXDEV,
            "ENOSPC" => libc::ENOSPC,
            other => panic!("unknown errno {other}"),
        }
    }
}

#[cfg(target_os = "linux")]
mod linux_support;
#[cfg(target_os = "macos")]
mod macos_support;

/// Runs one case for one profile and says what it showed.
#[cfg(unix)]
fn run(case: &Case, profile: &str) -> (&'static str, String) {
    let namespaces = case.needs.as_deref() == Some("namespaces");
    match case.needs.as_deref() {
        None => {}
        Some("namespaces") => {
            if let Err(why) = unix::namespaces() {
                return ("skipped", why);
            }
        }
        Some("unixSockets") => {
            if let Err(why) = unix::unix_sockets() {
                return ("skipped", why);
            }
        }
        Some(n) => panic!("{}: unknown need {n:?}", case.name),
    }
    let tree = Tree::new();
    let op = tree.op(case);
    let got = unix::attempt(&tree, case, profile, &op, namespaces);
    let (want_errnos, deny) = match &case.expect {
        Expect::Word(w) if w == "allow" => (vec![], false),
        Expect::Word(w) if w == "deny" => (DENIALS.to_vec(), true),
        Expect::Word(w) => panic!("{}: unknown expectation {w:?}", case.name),
        Expect::Errno { errno } => (errno.iter().map(|e| unix::errno(e)).collect(), true),
    };
    if !deny {
        return if got == Outcome::Ran(None) {
            ("allowed", String::new())
        } else {
            ("failed", format!("expected success: {got:?}"))
        };
    }
    let denied = matches!(got, Outcome::Ran(Some(e)) if want_errnos.contains(&e));
    if !denied {
        return ("failed", format!("not denied: {got:?}"));
    }
    let control = match case.control.as_deref().unwrap_or("required") {
        "required" => true,
        "root" => unix::is_root(),
        "none" => false,
        other => panic!("{}: unknown control {other:?}", case.name),
    };
    if control {
        let fresh = Tree::new();
        let c = unix::probe(&["--none".into()], &fresh.op(case));
        if c != Outcome::Ran(None) {
            return ("failed", format!("the unsandboxed control failed: {c:?}"));
        }
    }
    let Outcome::Ran(Some(e)) = got else {
        unreachable!()
    };
    if control {
        ("denied", format!("errno {e}"))
    } else {
        (
            "uncontrolled",
            format!("errno {e}, no control on this host"),
        )
    }
}

/// Runs every case of `file` for every profile it names.
fn suite(file: &str, text: &str) {
    let parsed: File = serde_json::from_str(text).unwrap_or_else(|e| panic!("{file}: {e}"));
    let backend = backend();
    let mut records = vec![];
    for case in &parsed.cases {
        for profile in &case.profiles {
            let applies = case.os.as_ref().is_none_or(|l| l.iter().any(|o| o == OS));
            let (result, detail) = match &backend {
                _ if !applies => ("skipped", format!("not a {OS} case")),
                Backend::Skip(why) => ("skipped", why.clone()),
                #[cfg(windows)]
                Backend::Windows => ("skipped", "no Windows backend yet".into()),
                #[cfg(unix)]
                Backend::Unix => run(case, profile),
            };
            if result == "skipped" {
                eprintln!("{file}: {profile}: {}: skipped, {detail}", case.name);
            }
            records.push(Record {
                os: OS,
                file: file.into(),
                case: case.name.clone(),
                profile: profile.clone(),
                result,
                detail,
            });
        }
    }
    report(file, &records);
    let failed: Vec<&Record> = records.iter().filter(|r| r.result == "failed").collect();
    assert!(failed.is_empty(), "{file}: {failed:#?}");
    // Every case ran or said why not.
    assert_eq!(
        records.len(),
        parsed.cases.iter().map(|c| c.profiles.len()).sum::<usize>()
    );
}

fn report(file: &str, records: &[Record]) {
    let Some(dir) = std::env::var_os("FIN_ESCAPE_REPORT") else {
        return;
    };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(format!("{}.json", file.trim_end_matches(".json")));
    std::fs::write(path, serde_json::to_vec_pretty(records).unwrap()).unwrap();
}

#[test]
fn server_data() {
    suite("server_data.json", include_str!("cases/server_data.json"));
}

#[test]
fn sessions() {
    suite("sessions.json", include_str!("cases/sessions.json"));
}

#[test]
fn links_mounts() {
    suite("links_mounts.json", include_str!("cases/links_mounts.json"));
}

#[test]
fn agent_claude() {
    suite("agent-claude.json", include_str!("cases/agent-claude.json"));
}

#[test]
fn every_case_file_names_known_profiles_and_oses() {
    // Pure: a typo in a case file would otherwise run nothing for it.
    for (file, text) in [
        ("server_data.json", include_str!("cases/server_data.json")),
        ("sessions.json", include_str!("cases/sessions.json")),
        ("links_mounts.json", include_str!("cases/links_mounts.json")),
        ("agent-claude.json", include_str!("cases/agent-claude.json")),
    ] {
        let parsed: File = serde_json::from_str(text).unwrap();
        assert!(!parsed.cases.is_empty(), "{file}");
        let mut names = std::collections::BTreeSet::new();
        for c in &parsed.cases {
            assert!(!c.profiles.is_empty(), "{file}: {}", c.name);
            for p in &c.profiles {
                assert!(
                    ["agent", "hook", "worker", "plugin"].contains(&p.as_str()),
                    "{file}: {}: {p}",
                    c.name
                );
                assert!(
                    names.insert((p.clone(), c.name.clone())),
                    "{file}: duplicate {}",
                    c.name
                );
            }
            for o in c.os.iter().flatten() {
                assert!(
                    ["linux", "macos", "windows"].contains(&o.as_str()),
                    "{file}: {o}"
                );
            }
            assert!(!c.op.is_empty(), "{file}: {}", c.name);
            match &c.expect {
                Expect::Word(w) => assert!(["allow", "deny"].contains(&w.as_str()), "{file}: {w}"),
                Expect::Errno { errno } => assert!(!errno.is_empty(), "{file}: {}", c.name),
            }
            if let Some(n) = &c.needs {
                assert!(
                    ["namespaces", "unixSockets"].contains(&n.as_str()),
                    "{file}: {}",
                    c.name
                );
            }
            if let Some(h) = &c.harness {
                assert!(
                    ["codex", "claudeCode"].contains(&h.as_str()),
                    "{file}: {}",
                    c.name
                );
                assert!(
                    c.profiles.iter().all(|p| p == "agent"),
                    "{file}: {}",
                    c.name
                );
            }
            if let Some(k) = &c.control {
                assert!(
                    ["required", "root", "none"].contains(&k.as_str()),
                    "{file}: {k}"
                );
            }
        }
    }
}
