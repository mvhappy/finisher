//! The launcher stays the program's parent (plan 7C Decision 3 as amended): a subreaper that
//! forwards signals, passes the program's exit through and, when the session ends, kills every
//! descendant, a `setsid` one included. The session ends when the program exits, when the
//! launcher is hung up (`SIGHUP`, what `finisherd` sends at session end) and when its own
//! parent dies. Each case runs unconfined (Landlock stubbed disabled, F2) and, where Landlock
//! is enforced, confined; the reaper is the same in both.
#![cfg(target_os = "linux")]

mod linux_support;

use std::io::{BufRead, BufReader};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use linux_support::{LAUNCHER, agent, give_request, landlock_or_skip, launch_cmd, tempdir};

const PATH: (&str, &str) = ("PATH", "/usr/bin:/bin");
const NS_OFF: (&str, &str) = ("FIN_TEST_SANDBOX_NAMESPACES", "unavailable");
const UNCONFINED: (&str, &str) = (fin_sandbox::status::TEST_ENV, "disabled");

/// Starts a `setsid` grandchild that leaves the program's process group and session, and
/// waits until it has written its pid to `g.pid` in the working directory.
const ESCAPE: &str = "setsid sh -c 'echo $$ > g.pid.tmp; mv g.pid.tmp g.pid; exec sleep 300' \
                      </dev/null >/dev/null 2>&1 & while [ ! -s g.pid ]; do sleep 0.02; done";

/// The environments each case runs in: unconfined always, confined where Landlock is.
fn modes(test: &str) -> Vec<Vec<(&'static str, &'static str)>> {
    let mut v = vec![vec![PATH, UNCONFINED]];
    if landlock_or_skip(test).is_some() {
        v.push(vec![PATH, NS_OFF]);
        v.push(vec![PATH]);
    }
    v
}

/// The request's workspace (writable under the profile).
fn workspace(t: &Path) -> PathBuf {
    t.join("p/workspace/s1")
}

/// Exists and is not a zombie (an orphan's zombie waits for whichever subreaper adopted it).
fn alive(pid: i32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| !rest.starts_with('Z') && !rest.starts_with('X')),
        Err(_) => false,
    }
}

/// Waits up to 5 s for `pid` to be gone (its zombie reaped).
fn gone(pid: i32) -> bool {
    let until = Instant::now() + Duration::from_secs(5);
    while alive(pid) {
        if Instant::now() > until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

fn grandchild(t: &Path) -> i32 {
    let text = std::fs::read_to_string(workspace(t).join("g.pid")).expect("g.pid");
    text.trim().parse().expect("a pid")
}

/// Kills what a failed assertion would leave running.
struct Reap(Vec<i32>);
impl Drop for Reap {
    fn drop(&mut self) {
        for &p in &self.0 {
            if p > 0 && alive(p) {
                // SAFETY: a process this test started (through the launcher).
                unsafe { libc::kill(p, libc::SIGKILL) };
            }
        }
    }
}

#[test]
fn a_setsid_grandchild_goes_when_the_program_exits() {
    for env in modes("a_setsid_grandchild_goes_when_the_program_exits") {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let script = format!("{ESCAPE}; exit 3");
        let (mut cmd, keep) = launch_cmd(
            Some(&req),
            &["--policy-fd", "3", "--", "sh", "-c", &script],
            &env,
        );
        let out = cmd.current_dir(workspace(&t)).output().unwrap();
        drop(keep);
        // The program's exit code, as before.
        assert_eq!(out.status.code(), Some(3), "{env:?} {out:?}");
        let g = grandchild(&t);
        let _reap = Reap(vec![g]);
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the program"
        );
    }
}

#[test]
fn hang_up_ends_the_session() {
    for env in modes("hang_up_ends_the_session") {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let script = format!("{ESCAPE}; echo ready; exec sleep 300");
        let (mut cmd, keep) = launch_cmd(
            Some(&req),
            &["--policy-fd", "3", "--", "sh", "-c", &script],
            &env,
        );
        cmd.current_dir(workspace(&t)).stdout(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        drop(keep);
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n", "{env:?}");
        let g = grandchild(&t);
        let _reap = Reap(vec![g]);
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
        let status = child.wait().unwrap();
        // The program was killed, and the launcher's exit says so.
        assert_eq!(status.signal(), Some(libc::SIGKILL), "{env:?}");
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the hang-up"
        );
    }
}

#[test]
fn signals_reach_the_program_and_its_exit_passes_through() {
    for env in modes("signals_reach_the_program_and_its_exit_passes_through") {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let script = "trap 'echo int; exit 5' INT; echo running; while :; do sleep 0.05; done";
        let (mut cmd, keep) = launch_cmd(
            Some(&req),
            &["--policy-fd", "3", "--", "sh", "-c", script],
            &env,
        );
        cmd.stdout(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        drop(keep);
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "running\n", "{env:?}");
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "int\n", "{env:?}");
        assert_eq!(child.wait().unwrap().code(), Some(5), "{env:?}");
    }
}

#[test]
fn the_parents_death_ends_the_session() {
    for env in modes("the_parents_death_ends_the_session") {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        let program = format!("{ESCAPE}; echo ready; exec sleep 300");
        // A parent that starts the launcher, says its pid and stays until it is killed.
        let mut parent = Command::new("sh");
        parent
            .args([
                "-c",
                "\"$0\" --policy-fd 3 -- sh -c \"$1\" & echo $!; exec sleep 300",
                LAUNCHER,
                &program,
            ])
            .env_clear()
            .envs(env.iter().copied())
            .current_dir(workspace(&t))
            .stdout(Stdio::piped());
        let keep = give_request(&mut parent, &req);
        let mut parent = parent.spawn().unwrap();
        drop(keep);
        let mut out = BufReader::new(parent.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let launcher: i32 = line.trim().parse().expect("the launcher's pid");
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line, "ready\n", "{env:?}");
        let g = grandchild(&t);
        let _reap = Reap(vec![g, launcher]);
        parent.kill().unwrap();
        parent.wait().unwrap();
        assert!(gone(launcher), "{env:?}: the launcher outlived its parent");
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the parent"
        );
    }
}

const INNER: &str = "FIN_TEST_CGROUP_INNER";
const REQUIRE: &str = "FIN_REQUIRE_CGROUP";

/// `true` inside a delegated user unit (`systemd-run --user -p Delegate=yes`), where this
/// process is alone in its cgroup; otherwise runs `test` there (or skips with a notice, a
/// failure under `FIN_REQUIRE_CGROUP=1`) and answers `false`. As `fin-workers`' `host_cgroup`.
fn inside(test: &str) -> bool {
    let required = std::env::var(REQUIRE).is_ok_and(|v| v == "1");
    if std::env::var_os(INNER).is_some() {
        return match fin_sandbox::linux::reaper::delegated_cgroup() {
            Ok(_) => true,
            Err(why) => {
                assert!(!required, "{test}: {REQUIRE}=1 but {why}");
                eprintln!("{test}: skipped, {why}");
                false
            }
        };
    }
    let unit = [
        "--user",
        "-p",
        "Delegate=yes",
        "--wait",
        "--quiet",
        "--collect",
    ];
    let probe = Command::new("systemd-run")
        .args(unit)
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if !probe.is_ok_and(|s| s.success()) {
        assert!(
            !required,
            "{test}: {REQUIRE}=1 but systemd-run --user is not available"
        );
        eprintln!("{test}: skipped, `systemd-run --user -p Delegate=yes` is not available");
        return false;
    }
    let mut c = Command::new("systemd-run");
    c.args(unit).arg("--pipe").arg(format!("-E{INNER}=1"));
    for v in [REQUIRE, "TMPDIR", "FIN_REQUIRE_LANDLOCK"] {
        if let Some(x) = std::env::var_os(v) {
            c.arg(format!("-E{v}={}", x.to_string_lossy()));
        }
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

#[test]
fn a_delegated_cgroup_holds_the_session_and_is_removed() {
    let test = "a_delegated_cgroup_holds_the_session_and_is_removed";
    if !inside(test) {
        return;
    }
    let own = fin_sandbox::linux::reaper::delegated_cgroup().unwrap();
    for env in modes(test) {
        let (_d, t) = tempdir();
        let req = serde_json::to_vec(&agent(&t)).unwrap();
        // A setsid grandchild, seen from outside: in the session's cgroup, gone after the
        // hang-up with the cgroup itself.
        let script = format!("{ESCAPE}; echo ready; exec sleep 300");
        let (mut cmd, keep) = launch_cmd(
            Some(&req),
            &["--policy-fd", "3", "--", "sh", "-c", &script],
            &env,
        );
        cmd.current_dir(workspace(&t)).stdout(Stdio::piped());
        let mut child = cmd.spawn().unwrap();
        drop(keep);
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line, "ready\n", "{env:?}");
        let g = grandchild(&t);
        let _reap = Reap(vec![g]);
        let cg = std::fs::read_to_string(format!("/proc/{g}/cgroup")).unwrap();
        let session = format!("fin-session-{}", child.id());
        assert!(
            cg.trim_end().ends_with(&format!("/{session}")),
            "{env:?}: {cg}"
        );
        assert!(own.join(&session).is_dir(), "{env:?}");
        // SAFETY: signalling our own child.
        assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGHUP) }, 0);
        assert_eq!(
            child.wait().unwrap().signal(),
            Some(libc::SIGKILL),
            "{env:?}"
        );
        assert!(
            gone(g),
            "{env:?}: the setsid grandchild {g} outlived the hang-up"
        );
        assert!(
            !own.join(&session).exists(),
            "{env:?}: the session's cgroup was left"
        );
    }
}
