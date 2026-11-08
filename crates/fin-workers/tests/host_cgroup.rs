//! Delegated cgroups v2 (5C-1 plan, Task 4; Decision 16). Each test runs itself again inside a
//! transient user unit with `Delegate=yes` (`systemd-run --user -p Delegate=yes --wait`), where
//! the test process is alone in its cgroup, and is skipped with a notice where `systemd-run
//! --user` is not available. Where the unit's cgroup is not the test's alone (other processes
//! in it, as when agents share a cgroup, or a write into it meets `EBUSY`) a test is skipped
//! with a notice too, unless `FIN_REQUIRE_CGROUP=1` (CI) makes that a failure.

// Tests drive the host and inspect job directories and process state by path; none is a
// library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
#![cfg(target_os = "linux")]

mod host_support;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use fin_jobhost::*;
use host_support::*;
use serde_json::{Value, json};

const INNER: &str = "FIN_TEST_CGROUP_INNER";
const REQUIRE: &str = "FIN_REQUIRE_CGROUP";
const EBUSY: i32 = 16;
const MIB: u64 = 1 << 20;

fn required() -> bool {
    std::env::var(REQUIRE).is_ok_and(|v| v == "1")
}

/// Skips `test` with a notice (`false`), or fails it when `FIN_REQUIRE_CGROUP=1`.
fn unavailable(test: &str, why: &str) -> bool {
    assert!(!required(), "{test}: {REQUIRE}=1 but {why}");
    eprintln!("{test}: skipped, {why}");
    false
}

/// Whether this process is alone in its cgroup, which the delegated unit gives it. Another
/// agent's processes in the same cgroup make the detection (rightly) refuse it.
fn alone(test: &str) -> bool {
    let cg = own_cgroup();
    let me = std::process::id();
    let others: Vec<u32> = procs(&cg).into_iter().filter(|p| *p != me).collect();
    others.is_empty()
        || unavailable(
            test,
            &format!("{} holds other processes {others:?}", cg.display()),
        )
}

/// `true` inside the delegated unit; otherwise runs `test` there (or skips) and answers
/// `false`.
fn inside(test: &str) -> bool {
    if std::env::var_os(INNER).is_some() {
        return alone(test);
    }
    let probe = Command::new("systemd-run")
        .args([
            "--user",
            "-p",
            "Delegate=yes",
            "--wait",
            "--quiet",
            "--collect",
            "true",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        eprintln!("{test}: skipped, `systemd-run --user -p Delegate=yes` is not available");
        return false;
    }
    let mut c = Command::new("systemd-run");
    if let Some(v) = std::env::var_os(REQUIRE) {
        c.arg(format!("-E{REQUIRE}={}", v.to_string_lossy()));
    }
    c.args([
        "--user",
        "-p",
        "Delegate=yes",
        "--wait",
        "--pipe",
        "--quiet",
        "--collect",
    ])
    .arg(format!("-E{INNER}=1"));
    if let Some(t) = std::env::var_os("TMPDIR") {
        c.arg(format!("-ETMPDIR={}", t.to_string_lossy()));
    }
    let status = c
        .arg(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture", "--test-threads=1"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "{test} failed in its delegated unit");
    false
}

/// This process's cgroup directory.
fn own_cgroup() -> PathBuf {
    let s = std::fs::read_to_string("/proc/self/cgroup").unwrap();
    let rel = s.lines().find_map(|l| l.strip_prefix("0::")).unwrap();
    Path::new("/sys/fs/cgroup").join(rel.trim_start_matches('/'))
}

fn procs(dir: &Path) -> Vec<u32> {
    std::fs::read_to_string(dir.join("cgroup.procs"))
        .unwrap()
        .lines()
        .map(|l| l.parse().unwrap())
        .collect()
}

fn sleeper() -> Child {
    Command::new("sleep").arg("600").spawn().unwrap()
}

#[test]
fn delegation_detected_only_when_alone() {
    if !inside("delegation_detected_only_when_alone") {
        return;
    }
    let cg = own_cgroup();
    let off = host_with(|o| o.cgroups = CgroupMode::Off);
    assert_eq!(off.host.caps().kill_tree, KillTree::ProcessGroup);
    assert!(!cg.join("host").exists(), "Off leaves the cgroup alone");
    drop(off);
    let h = host();
    let caps = h.host.caps();
    assert_eq!(caps.kill_tree, KillTree::Cgroup);
    assert!(caps.cgroup_memory && caps.leaf_limits, "{caps:?}");
    // A second host in this process finds itself in `<cg>/host` and uses the same root.
    let h2 = host();
    assert_eq!(h2.host.caps().kill_tree, KillTree::Cgroup);
}

#[test]
fn terminal_scope_is_not_used() {
    if !inside("terminal_scope_is_not_used") {
        return;
    }
    let cg = own_cgroup();
    let mut other = sleeper();
    let h = host();
    assert_eq!(h.host.caps().kill_tree, KillTree::ProcessGroup);
    assert!(!cg.join("host").exists() && !cg.join("jobs").exists());
    other.kill().unwrap();
    other.wait().unwrap();
}

#[test]
fn host_moves_into_host_leaf() {
    if !inside("host_moves_into_host_leaf") {
        return;
    }
    let cg = own_cgroup();
    let _h = host();
    assert_eq!(own_cgroup(), cg.join("host"));
    assert!(
        procs(&cg).is_empty(),
        "no process left in the delegated root"
    );
    let jobs = std::fs::read_to_string(cg.join("jobs/cgroup.subtree_control")).unwrap();
    assert!(jobs.contains("memory") && jobs.contains("pids"), "{jobs}");
}

#[test]
fn process_gets_its_leaf_before_exec() {
    if !inside("process_gets_its_leaf_before_exec") {
        return;
    }
    let mut h = host();
    let (_, v) = h.run("test.echo", json!({ "value": 1, "report": ["cgroup"] }));
    let at_start = v["cgroup"].as_str().unwrap().trim().to_owned();
    let leaf = at_start.rsplit('/').take(3).collect::<Vec<_>>();
    assert_eq!(leaf[2], "jobs", "{at_start}");
    assert!(
        leaf[1].starts_with('g') && leaf[0].starts_with('p'),
        "{at_start}"
    );
}

#[test]
fn cgroup_kill_takes_setsid_grandchild() {
    if !inside("cgroup_kill_takes_setsid_grandchild") {
        return;
    }
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "setsid", "out": "pids.json" }),
    );
    let pids = spawn_pids(&h, "pids.json");
    assert!(!gone(pids.grandchild));
    h.kill(g, KillReason::Cancel);
    assert_eq!(h.exit(p).reason, ExitReason::Killed(KillReason::Cancel));
    assert!(wait_gone(pids.grandchild), "cgroup.kill takes it");
}

#[test]
fn group_memory_max_and_oom_reported() {
    if !inside("group_memory_max_and_oom_reported") {
        return;
    }
    let mut h = host();
    let g = limited_group(&h, 64 * MIB);
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.oom", json!({ "mib": 128, "holdMs": 60_000 }));
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Oom, "{exit:?}");
    assert!(exit.memory_peak.is_some_and(|b| b <= 64 * MIB), "{exit:?}");
}

#[test]
fn leaf_pids_max_from_job_limits() {
    if !inside("leaf_pids_max_from_job_limits") {
        return;
    }
    let mut h = host_with(|o| o.spawner = rewrite(|c| c.job_limits.active_processes = Some(2)));
    assert!(h.host.caps().leaf_limits);
    let g = h.group();
    let p = h.worker(g);
    let pid = h.hello(p).pid;
    let cg = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap();
    let dir = Path::new("/sys/fs/cgroup").join(cg.trim().strip_prefix("0::/").unwrap());
    let max = std::fs::read_to_string(dir.join("pids.max")).unwrap();
    assert_eq!(max.trim(), "512", "2 processes × 256 tasks");
    h.kill(g, KillReason::Test);
}

#[test]
fn leftover_groups_swept_at_start() {
    if !inside("leftover_groups_swept_at_start") {
        return;
    }
    let cg = own_cgroup();
    // A previous run's group, still holding a process.
    let left = cg.join("jobs/g9999/p1");
    std::fs::create_dir_all(&left).unwrap();
    let mut stray = sleeper();
    std::fs::write(left.join("cgroup.procs"), stray.id().to_string()).unwrap();
    let h = host();
    assert_eq!(h.host.caps().kill_tree, KillTree::Cgroup);
    let until = Instant::now() + WAIT;
    while stray.try_wait().unwrap().is_none() {
        assert!(Instant::now() < until, "the stray process is killed");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!cg.join("jobs/g9999").exists(), "its cgroup is removed");
}

/// The host process for `setsid_grandchild_gone_after_host_sigkill`: waits for `go` on stdin
/// (by then the test moved it into the delegated root alone), then runs `test.spawn` with
/// `setsid` under a watchdog and prints `PIDS <json>`.
#[test]
fn inner_cgroup_host() {
    if std::env::var_os("FIN_TEST_CGROUP_HOST").is_none() {
        return;
    }
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    let mut h = host_with(|o| o.watchdog = true);
    assert_eq!(h.host.caps().kill_tree, KillTree::Cgroup);
    let g = h.group();
    let p = h.worker(g);
    let worker = h.hello(p).pid;
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "setsid", "out": "pids.json" }),
    );
    let pids = spawn_pids(&h, "pids.json");
    let out = json!({
        "worker": worker,
        "grandchild": pids.grandchild,
        "watchdog": h.host.watchdog_pid_for_test(),
    });
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "\nPIDS {out}").unwrap();
    stdout.flush().unwrap();
    drop(stdout);
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Host death in cgroup mode: the watchdog's `cgroup.kill` takes even a `setsid` grandchild
/// (the gap Decision 18 documents for process groups is closed here).
#[test]
fn setsid_grandchild_gone_after_host_sigkill() {
    if !inside("setsid_grandchild_gone_after_host_sigkill") {
        return;
    }
    let cg = own_cgroup();
    // This test process moves aside, so the host process is alone in the root. A cgroup that
    // cannot take it (`EBUSY`: it is shared, or its controllers rule out processes) is not one
    // this test can run in.
    std::fs::create_dir_all(cg.join("outer")).unwrap();
    match std::fs::write(
        cg.join("outer/cgroup.procs"),
        std::process::id().to_string(),
    ) {
        Err(e) if e.raw_os_error() == Some(EBUSY) => {
            unavailable(
                "setsid_grandchild_gone_after_host_sigkill",
                &format!("{} refuses a process (EBUSY): {e}", cg.display()),
            );
            return;
        }
        r => r.unwrap(),
    }
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "inner_cgroup_host",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("FIN_TEST_CGROUP_HOST", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::fs::write(cg.join("cgroup.procs"), child.id().to_string()).unwrap();
    child.stdin.take().unwrap().write_all(b"go\n").unwrap();
    let out = BufReader::new(child.stdout.take().unwrap());
    let pids: Value = out
        .lines()
        .map_while(Result::ok)
        .find_map(|l| {
            l.strip_prefix("PIDS ")
                .map(|j| serde_json::from_str(j).unwrap())
        })
        .expect("the host prints its pids");
    let pid = |k: &str| pids[k].as_u64().unwrap() as u32;
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(wait_gone(pid("worker")), "the worker is gone");
    assert!(
        wait_gone(pid("grandchild")),
        "the setsid grandchild is gone"
    );
    assert!(wait_gone(pid("watchdog")), "the watchdog exited");
}
