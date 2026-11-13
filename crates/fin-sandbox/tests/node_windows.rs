//! Node inside a Windows container (plan 7C Decision 25, Task 9 Step 3): the launcher carries
//! `--preserve-symlinks --preserve-symlinks-main --require <guard>` on Node's command line and
//! in `NODE_OPTIONS`, and the guard turns a `child_process` call with piped stdio, which would
//! hang inside a container, into a `WINDOWS_NODE_PIPES` error within its probe's timeout.
//!
//! Needs Node: `FIN_TEST_NODE=<path to node.exe>` or `node.exe` on `PATH` (else each case is
//! skipped with a notice;
//! `FIN_REQUIRE_NODE=1` turns the skip into a failure). Node is copied into the test's
//! toolchain folder, which the toolchain capability is granted on, as `finisherd` grants the
//! real toolchain roots.
#![cfg(windows)]

mod windows_support;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::AgentInput;
use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::windows::grants::{self, Trustee};
use fin_sandbox::windows::launcher::{NODE_GUARD, NODE_GUARD_ENV};
use fin_sandbox::windows::sid::Sid;
use fin_sandbox::windows::{acl, appcontainer, capability};
use windows_support::{Run, Tree, run_in};

/// `FIN_TEST_NODE`, else `node.exe` on `PATH`.
fn node_or_skip(test: &str) -> Option<PathBuf> {
    let on_path = || {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join("node.exe"))
            .find(|n| n.is_file())
    };
    match std::env::var_os("FIN_TEST_NODE")
        .map(PathBuf::from)
        .or_else(on_path)
    {
        Some(n) => Some(n),
        None => {
            assert_ne!(
                std::env::var("FIN_REQUIRE_NODE").as_deref(),
                Ok("1"),
                "{test}: FIN_TEST_NODE is not set"
            );
            eprintln!("{test}: skipped, FIN_TEST_NODE is not set and no node.exe is on PATH");
            None
        }
    }
}

struct Setup {
    t: Tree,
    node: PathBuf,
    req: LaunchRequest,
    container: String,
}

impl Setup {
    fn new(node: &Path) -> Setup {
        let t = Tree::new();
        for d in [
            "p/workspace/s1",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "data/agents/s1/localappdata",
            "tools",
        ] {
            t.dir(d);
        }
        let copy = t.p("tools/node.exe");
        std::fs::copy(node, &copy).unwrap();
        let i = AgentInput {
            session_id: "s1".into(),
            mode: AgentMode::Draft,
            harness: Harness::Codex,
            project_dir: t.p("p"),
            workspace_dir: t.p("p/workspace/s1"),
            shared_dir: None,
            library_roots: vec![],
            session_dir: t.p("data/agents/s1"),
            sign_in_store: None,
            toolchains: vec![t.p("tools")],
            server_data: t.p("data"),
            home: t.p("home"),
            fin: None,
        };
        for g in grants::agent(&i)
            .unwrap()
            .into_iter()
            .filter(|g| g.path.exists())
        {
            let sid = match &g.trustee {
                Trustee::Sid(s) => Sid::parse(s).unwrap(),
                Trustee::Container(c) => appcontainer::sid(c).unwrap(),
            };
            acl::grant(&g.path, &sid, g.mask, true).unwrap();
        }
        let container = capability::project_container(&i.project_dir);
        Setup {
            t,
            node: copy,
            req: LaunchRequest::Agent(i),
            container,
        }
    }

    /// Runs `script` (written into the workspace, with `files` beside it) with Node in the
    /// container.
    fn node(&self, script: &str, files: &[(&str, &str)]) -> (Run, Duration) {
        let path = self.t.write("p/workspace/s1/main.js", script);
        for (n, text) in files {
            self.t.write(&format!("p/workspace/s1/{n}"), text);
        }
        let node = self.node.to_string_lossy().into_owned();
        let t0 = Instant::now();
        // `finisherd` starts agents in their workspace; Node resolves `--require` from its
        // working directory and reads the nearest `package.json` above it.
        let cwd = self.t.p("p/workspace/s1");
        let r = run_in(
            &self.req,
            &[&node, &path.to_string_lossy()],
            &[],
            Some(&cwd),
        );
        (r, t0.elapsed())
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        let _ = appcontainer::delete(&self.container);
    }
}

const PIPED: &str = r#"
const cp = require("node:child_process");
const out = {};
for (const [n, f] of [
  ["spawn", () => cp.spawn("cmd.exe", ["/c", "echo hi"])],
  ["spawnSync", () => cp.spawnSync("cmd.exe", ["/c", "echo hi"])],
  ["execSync", () => cp.execSync("echo hi")],
]) {
  const t = Date.now();
  try { f(); out[n] = "ran"; } catch (e) { out[n] = e.code; }
  out[n + "Ms"] = Date.now() - t;
}
console.log(JSON.stringify(out));
cp.execFileSync("cmd.exe", ["/c", "echo hi"]);
"#;

#[test]
fn node_piped_spawn_fails_fast() {
    let Some(node) = node_or_skip("node_piped_spawn_fails_fast") else {
        return;
    };
    let st = Setup::new(&node);
    let (r, took) = st.node(PIPED, &[]);
    assert!(took < Duration::from_secs(20), "{took:?} {r:?}");
    let v = r.json();
    for n in ["spawn", "spawnSync", "execSync"] {
        assert_eq!(v[n], "WINDOWS_NODE_PIPES", "{n}: {r:?}");
        assert!(v[format!("{n}Ms")].as_u64().unwrap() < 5000, "{n}: {r:?}");
    }
    // The last call is not caught: Node exits non-zero with the error.
    assert_ne!(r.code, Some(0), "{r:?}");
    assert!(r.stderr.contains("WINDOWS_NODE_PIPES"), "{r:?}");
}

#[test]
fn node_inherit_and_ignore_stdio_still_work() {
    let Some(node) = node_or_skip("node_inherit_and_ignore_stdio_still_work") else {
        return;
    };
    let st = Setup::new(&node);
    let script = r#"
const cp = require("node:child_process");
const a = cp.spawnSync("cmd.exe", ["/c", "exit 7"], { stdio: "ignore" }).status;
const b = cp.spawnSync("cmd.exe", ["/c", "exit 9"], { stdio: "inherit" }).status;
console.log(JSON.stringify({ a, b }));
"#;
    let (r, _) = st.node(script, &[]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let v = r.json();
    assert_eq!(
        (v["a"].as_i64(), v["b"].as_i64()),
        (Some(7), Some(9)),
        "{r:?}"
    );
}

#[test]
fn node_script_runs_with_preserve_symlinks() {
    let Some(node) = node_or_skip("node_script_runs_with_preserve_symlinks") else {
        return;
    };
    let st = Setup::new(&node);
    let script =
        r#"console.log(JSON.stringify({ v: require("./lib.js"), argv: process.execArgv }));"#;
    let (r, _) = st.node(script, &[("lib.js", "module.exports = 42;")]);
    assert_eq!(r.code, Some(0), "{r:?}");
    let v = r.json();
    assert_eq!(v["v"], 42, "{r:?}");
    let argv = v["argv"].to_string();
    assert!(
        argv.contains("--preserve-symlinks-main") && argv.contains("--require"),
        "{argv}"
    );
}

#[test]
fn node_options_carry_the_guard_to_grandchildren() {
    let Some(node) = node_or_skip("node_options_carry_the_guard_to_grandchildren") else {
        return;
    };
    let st = Setup::new(&node);
    // The child Node gets no flags on its command line: only NODE_OPTIONS carries them, and
    // its script path needs `--preserve-symlinks-main` too.
    let script = r#"
const cp = require("node:child_process");
const r = cp.spawnSync(process.execPath, [__dirname + "\\child.js"], { stdio: "inherit" });
process.exit(r.status === 3 ? 0 : 10 + (r.status ?? 0));
"#;
    let child = r#"
try { require("node:child_process").spawnSync("cmd.exe", ["/c", "echo hi"]); process.exit(4); }
catch (e) { process.exit(e.code === "WINDOWS_NODE_PIPES" ? 3 : 5); }
"#;
    let (r, took) = st.node(script, &[("child.js", child)]);
    assert_eq!(r.code, Some(0), "{r:?}");
    assert!(took < Duration::from_secs(20), "{took:?}");
}

#[test]
fn guard_inert_when_pipes_work() {
    let Some(node) = node_or_skip("guard_inert_when_pipes_work") else {
        return;
    };
    // Outside a container libuv's pipes work: the guard's probe passes and the call goes
    // through unchanged.
    let t = Tree::new();
    let guard = t.write("guard.cjs", NODE_GUARD);
    let out = std::process::Command::new(&node)
        .args(["--require", &guard.to_string_lossy(), "-e"])
        .arg(r#"process.stdout.write(require("node:child_process").execSync("echo hi").toString().trim())"#)
        .env(NODE_GUARD_ENV, "1")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hi", "{out:?}");
}

/// A hook with a wall time, Node in its toolchain, `script` as its file.
fn hook_setup(node: &Path, script: &str, timeout_ms: u64) -> (Tree, LaunchRequest, String) {
    use fin_sandbox::HookInput;
    let t = Tree::new();
    for d in ["p", "data/hooks/h1", "tools"] {
        t.dir(d);
    }
    std::fs::copy(node, t.p("tools/node.exe")).unwrap();
    let file = t.write("p/hooks/h.js", script);
    let i = HookInput {
        project_dir: Some(t.p("p")),
        hook_file: file,
        toolchains: vec![t.p("tools")],
        tmp: t.p("data/hooks/h1"),
        server_data: t.p("data"),
        home: t.p("home"),
        fin: None,
        timeout_ms: Some(timeout_ms),
    };
    for g in grants::hook(&i)
        .unwrap()
        .into_iter()
        .filter(|g| g.path.exists())
    {
        let sid = match &g.trustee {
            Trustee::Sid(s) => Sid::parse(s).unwrap(),
            Trustee::Container(c) => appcontainer::sid(c).unwrap(),
        };
        acl::grant(&g.path, &sid, g.mask, true).unwrap();
    }
    let container = capability::project_container(i.project_dir.as_ref().unwrap());
    (t, LaunchRequest::Hook(i), container)
}

fn run_hook(t: &Tree, req: &LaunchRequest) -> Run {
    let node = t.p("tools/node.exe").to_string_lossy().into_owned();
    let script = t.p("p/hooks/h.js").to_string_lossy().into_owned();
    let cwd = t.p("data/hooks/h1");
    run_in(req, &[&node, &script], &[], Some(&cwd))
}

#[test]
fn wall_time_reports_node_pipes_for_a_node_without_the_guard() {
    let Some(node) = node_or_skip("wall_time_reports_node_pipes_for_a_node_without_the_guard")
    else {
        return;
    };
    // The grandchild Node loads no guard (its environment drops it) and hangs on piped
    // stdio, as a Node started by a tool that resets NODE_OPTIONS would.
    let script = r#"
const cp = require("node:child_process");
const env = { ...process.env };
delete env.NODE_OPTIONS;
delete env.FIN_SANDBOX_NODE_GUARD;
cp.spawnSync(process.execPath, ["--preserve-symlinks", "--preserve-symlinks-main", "-e",
  "require('node:child_process').spawnSync('cmd.exe', ['/c', 'echo hi'])"], { stdio: "inherit", env });
"#;
    let (t, req, container) = hook_setup(&node, script, 6000);
    let t0 = Instant::now();
    let r = run_hook(&t, &req);
    let _ = appcontainer::delete(&container);
    assert_eq!(r.code, Some(fin_sandbox::launch::EXIT_WALL_TIME), "{r:?}");
    assert!(t0.elapsed() < Duration::from_secs(30), "{:?}", t0.elapsed());
    assert!(r.stderr.contains(r#""op":"nodePipes""#), "{r:?}");
}

#[test]
fn wall_time_reports_wall_time_for_a_busy_program() {
    let Some(node) = node_or_skip("wall_time_reports_wall_time_for_a_busy_program") else {
        return;
    };
    let script = r#"
const fs = require("node:fs");
const f = require("node:path").join(process.cwd(), "busy.txt");
for (;;) { fs.writeFileSync(f, String(Date.now())); }
"#;
    let (t, req, container) = hook_setup(&node, script, 4000);
    let r = run_hook(&t, &req);
    let _ = appcontainer::delete(&container);
    assert_eq!(r.code, Some(fin_sandbox::launch::EXIT_WALL_TIME), "{r:?}");
    assert!(r.stderr.contains(r#""op":"wallTime""#), "{r:?}");
}
