//! Unix host death and the watchdog (5C-1 plan, Task 4; Decisions 15, 18, 22): the host is
//! this test binary re-executed (`inner_host` with `FIN_TEST_HOST_ROLE`), killed with
//! `SIGKILL`; and, in process, the watchdog's own behaviour, foreign processes and
//! abandonment.

#![cfg(unix)]

mod host_support;

use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use fin_jobhost::*;
use host_support::*;
use serde_json::{Value, json};

const ROLE: &str = "FIN_TEST_HOST_ROLE";

fn signal(pid: u32, sig: i32) {
    // SAFETY: signals a test process by pid.
    unsafe { libc::kill(pid as i32, sig) };
}

/// The host process of the death tests: does nothing unless re-executed with `ROLE`. Prints
/// `PIDS <json>` once set up, then `WATCHDOG <pid>` whenever the watchdog is restarted, and
/// waits to be killed.
#[test]
fn inner_host() {
    let Ok(role) = std::env::var(ROLE) else {
        return;
    };
    let mut h = host_with(|o| o.watchdog = role != "nowatchdog");
    let g = h.group();
    let p = h.worker(g);
    let worker = h.hello(p).pid;
    let mut out = json!({
        "worker": worker,
        "tree": h.host.caps().kill_tree.as_str(),
        "dir": h.dir.path(),
    });
    match role.as_str() {
        "none" | "setsid" => {
            let params = json!({ "detach": role, "out": "pids.json" });
            h.start(p, "test.spawn", params);
            out["grandchild"] = spawn_pids(&h, "pids.json").grandchild.into();
        }
        "hang" => h.start(p, "test.hang", json!({})),
        "nowatchdog" => h.start(p, "test.sleep", json!({ "ms": 600_000 })),
        "late" => {
            // Two strangers in groups of their own. The watchdog is stopped, so it reads
            // `+g a`, `+g b` and the late `-g a` only after the host is dead.
            let stranger = || {
                Command::new("sleep")
                    .arg("600")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .process_group(0)
                    .spawn()
                    .unwrap()
                    .id()
            };
            let (a, b) = (stranger(), stranger());
            signal(h.host.watchdog_pid_for_test().unwrap(), libc::SIGSTOP);
            h.host.watchdog_line_for_test(&format!("+g {a}\n+g {b}\n"));
            h.host.watchdog_line_for_test(&format!("-g {a}\n"));
            out["a"] = a.into();
            out["b"] = b.into();
        }
        other => panic!("unknown role {other}"),
    }
    let mut last = h.host.watchdog_pid_for_test();
    out["watchdog"] = json!(last);
    say(&format!("PIDS {out}"));
    loop {
        std::thread::sleep(Duration::from_millis(20));
        let now = h.host.watchdog_pid_for_test();
        if let Some(n) = now
            && now != last
        {
            say(&format!("WATCHDOG {n}"));
            last = now;
        }
    }
}

/// One line to the outer test (stdout to a pipe is not flushed per line).
fn say(line: &str) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// A re-executed host and the pids it printed.
struct Inner {
    child: Child,
    lines: mpsc::Receiver<String>,
    pids: Value,
}

impl Inner {
    fn start(role: &str) -> Inner {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "inner_host", "--nocapture", "--test-threads=1"])
            .env(ROLE, role)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (tx, lines) = mpsc::channel();
        let out = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                let _ = tx.send(l);
            }
        });
        let mut inner = Inner {
            child,
            lines,
            pids: Value::Null,
        };
        let pids = inner.line("PIDS ");
        inner.pids = serde_json::from_str(&pids).unwrap();
        inner
    }

    /// The rest of the next line starting with `prefix`.
    fn line(&self, prefix: &str) -> String {
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let l = self.lines.recv_timeout(left).expect("the host prints it");
            // libtest's `test inner_host ... ` may precede it on the line.
            if let Some(i) = l.find(prefix) {
                return l[i + prefix.len()..].to_owned();
            }
        }
    }

    fn pid(&self, key: &str) -> u32 {
        self.pids[key]
            .as_u64()
            .unwrap_or_else(|| panic!("no {key}")) as u32
    }

    fn cgroups(&self) -> bool {
        self.pids["tree"] == "cgroup"
    }

    fn kill_host(&mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        for key in ["worker", "grandchild", "a", "b", "watchdog"] {
            if let Some(p) = self.pids[key].as_u64() {
                signal(p as u32, libc::SIGCONT);
                signal(p as u32, libc::SIGKILL);
            }
        }
        if let Some(d) = self.pids["dir"].as_str() {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

#[test]
fn grandchild_gone_after_host_sigkill() {
    let mut i = Inner::start("none");
    assert!(!gone(i.pid("grandchild")));
    i.kill_host();
    assert!(wait_gone(i.pid("worker")), "the worker is gone");
    assert!(wait_gone(i.pid("grandchild")), "the grandchild is gone");
    assert!(wait_gone(i.pid("watchdog")), "the watchdog exited");
}

/// Decisions 15 and 18: with process groups a grandchild that `setsid`s out of its group
/// survives host death (a documented gap; delegated cgroups and sub-project 7's process
/// denial close it). A later fix shows up as this test failing.
#[test]
fn setsid_grandchild_escape_is_documented() {
    let mut i = Inner::start("setsid");
    let gc = i.pid("grandchild");
    i.kill_host();
    assert!(wait_gone(i.pid("worker")));
    assert!(wait_gone(i.pid("watchdog")));
    if i.cgroups() {
        assert!(wait_gone(gc), "cgroup.kill takes the setsid grandchild");
    } else {
        std::thread::sleep(Duration::from_millis(500));
        assert!(!gone(gc), "the setsid grandchild escaped its process group");
        eprintln!(
            "documented gap (Decision 18): with process groups, a setsid grandchild ({gc}) \
             survives host death and cancel"
        );
    }
}

#[test]
fn hung_worker_gone_after_host_sigkill() {
    let mut i = Inner::start("hang");
    std::thread::sleep(Duration::from_millis(200));
    i.kill_host();
    assert!(wait_gone(i.pid("worker")), "the hung worker is gone");
    assert!(wait_gone(i.pid("watchdog")));
}

/// The host wrote `-g a` and died before the watchdog read it: `a` (a stranger by then) is
/// not signalled; `b`, still listed, is.
#[test]
fn late_minus_g_is_honoured() {
    let mut i = Inner::start("late");
    let (a, b, wd) = (i.pid("a"), i.pid("b"), i.pid("watchdog"));
    i.kill_host();
    signal(wd, libc::SIGCONT);
    assert!(wait_gone(wd), "the watchdog exited");
    assert!(wait_gone(b), "the listed group is killed");
    std::thread::sleep(Duration::from_millis(200));
    assert!(!gone(a), "the group whose -g came late is not signalled");
}

#[test]
fn worker_exits_on_stdin_eof() {
    // Without a watchdog, a worker whose host is gone sees EOF and exits 70.
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.sleep", json!({ "ms": 600_000 }));
    h.host.close_stdin(p);
    let exit = h.exit(p);
    assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(70)));
    // And after a real host death.
    let mut i = Inner::start("nowatchdog");
    assert!(i.pids["watchdog"].is_null());
    i.kill_host();
    assert!(wait_gone(i.pid("worker")), "the worker is gone");
}

#[test]
fn watchdog_ignores_sigint_and_sigterm() {
    let h = host_with(|o| o.watchdog = true);
    assert!(h.host.caps().watchdog);
    let wd = h.host.watchdog_pid_for_test().unwrap();
    for sig in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGPIPE] {
        signal(wd, sig);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(!gone(wd), "the watchdog lives");
    assert_eq!(h.host.watchdog_pid_for_test(), Some(wd), "not restarted");
}

#[test]
fn watchdog_restarts_and_resends() {
    let mut i = Inner::start("none");
    signal(i.pid("watchdog"), libc::SIGKILL);
    let new: u32 = i.line("WATCHDOG ").parse().unwrap();
    assert_ne!(new, i.pid("watchdog"));
    i.pids["watchdog"] = new.into();
    std::thread::sleep(Duration::from_millis(100));
    i.kill_host();
    assert!(
        wait_gone(i.pid("grandchild")),
        "the new watchdog got the list"
    );
    assert!(wait_gone(new));
}

#[test]
fn watchdog_is_not_reported_as_worker_exit() {
    let (log, lines) = captured_log();
    let mut h = host_with(|o| {
        o.watchdog = true;
        o.log = log;
    });
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    let wd = h.host.watchdog_pid_for_test().unwrap();
    signal(wd, libc::SIGKILL);
    let until = Instant::now() + WAIT;
    while h.host.watchdog_pid_for_test().is_none_or(|n| n == wd) {
        assert!(Instant::now() < until, "the watchdog is restarted");
        std::thread::sleep(Duration::from_millis(10));
    }
    h.start(p, "test.echo", json!({ "value": 3 }));
    assert_eq!(h.result(p)["value"], 3);
    // The cold worker exits after its job; nothing else is reported.
    let exit = h.exit(p);
    assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(0)));
    let events = h.events_within(Duration::from_millis(300));
    assert!(
        !events.iter().any(|e| matches!(e, HostEvent::Exit { .. })),
        "{events:?}"
    );
    assert!(lines.lock().unwrap().iter().any(|l| l.contains("watchdog")));
}

/// Decision 15: the `-g` is on the watchdog's pipe before the leader is reaped, so the pgid
/// cannot be reused while it is listed.
#[test]
fn minus_g_precedes_the_leader_reap() {
    let mut h = host_with(|o| o.watchdog = true);
    if h.host.caps().kill_tree != KillTree::ProcessGroup {
        return eprintln!("cgroup mode: the watchdog lists cgroups, not pgids");
    }
    let g = h.group();
    let p = h.worker(g);
    let pgid = h.hello(p).pid as i32;
    assert!(h.host.watchdog_groups_for_test().contains(&pgid));
    h.start(p, "test.echo", json!({ "value": 1 }));
    h.result(p);
    h.exit(p);
    h.group_empty(g);
    assert!(
        h.host.watchdog_groups_for_test().contains(&pgid),
        "listed until close"
    );
    h.host.close_group(g);
    let until = Instant::now() + WAIT;
    // SAFETY: signal 0 only checks for existence.
    while unsafe { libc::kill(-pgid, 0) } == 0
        || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    {
        assert!(Instant::now() < until, "the leader is reaped");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!h.host.watchdog_groups_for_test().contains(&pgid));
}

#[test]
fn foreign_processes_are_counted() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json" }),
    );
    let pids = spawn_pids(&h, "pids.json");
    h.kill(g, KillReason::Test);
    assert_eq!(h.exit(p).foreign, Some(1), "the grandchild");
    assert!(wait_gone(pids.grandchild));
    let (p2, _) = h.run("test.echo", json!({ "value": 1 }));
    assert_eq!(h.exit(p2).foreign, Some(0));
}

#[test]
fn abandon_after_kill_wait() {
    let (log, lines) = captured_log();
    let mut h = host_with(|o| {
        o.log = log;
        o.timing.kill_wait_ms = 300;
        // The waiter looks only after 2 s: until then the killed process seems alive.
        o.waiter_delay_for_test = Some(Duration::from_secs(2));
    });
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    let t0 = Instant::now();
    h.kill(g, KillReason::Cancel);
    assert!(
        t0.elapsed() < Duration::from_millis(1500),
        "the kill wait ends it"
    );
    assert_eq!(h.exit(p).reason, ExitReason::Abandoned);
    h.group_empty(g);
    let later = h.events_within(Duration::from_millis(2500));
    assert!(
        !later
            .iter()
            .any(|e| matches!(e, HostEvent::Exit { proc, .. } if *proc == p)),
        "reported once: {later:?}"
    );
    assert!(
        later
            .iter()
            .any(|e| matches!(e, HostEvent::AbandonedExit { proc } if *proc == p)),
        "its real exit is reported: {later:?}"
    );
    let lines = lines.lock().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.contains("abandoned") && l.contains("still alive"))
    );
    assert!(lines.iter().any(|l| l.contains("exited now")), "{lines:?}");
}
