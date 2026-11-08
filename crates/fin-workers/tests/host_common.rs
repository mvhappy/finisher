//! `fin-jobhost` over the test `fin-workers`, on every OS (5C-1 plan, Tasks 3 and 5): process
//! groups on Unix, Job Objects on Windows. Where the OSes differ (Windows has no signals), the
//! assertions say so.

// Tests drive the host and inspect job directories and process state by path; none is a
// library reader (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]
mod host_support;

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::time::Duration;

use fin_jobhost::*;
use fin_jobs::{CancelReason, HostMsg, WorkerMsg};
use host_support::*;
use serde_json::{Value, json};

#[test]
fn spawn_hello_start_result_exit() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    let hello = h.hello(p);
    assert_eq!(hello.family, "test");
    assert_eq!(hello.envelope.as_deref(), Some(fin_jobs::envelope_id()));
    h.start(p, "test.echo", json!({ "value": 42 }));
    assert_eq!(h.result(p)["value"], 42);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Exited);
    assert_eq!(exit.code, Some(0));
    h.group_empty(g);
}

/// The spawn's callback runs, then `Spawned`, before any other event of the process: even
/// one that exits at once (`--bogus`) or is killed by the spawn thread itself.
#[test]
fn spawned_leads_after_the_callback() {
    use std::sync::{Arc, Mutex};
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = log.clone();
    let mut h = host_observed(
        |_| {},
        move |e| {
            let tag = match e {
                HostEvent::Spawned { proc, .. } => format!("spawned {}", proc.0),
                HostEvent::GroupEmpty { .. } | HostEvent::AbandonedExit { .. } => return,
                e => format!("event {}", proc_of(e).unwrap().0),
            };
            seen.lock().unwrap().push(tag);
        },
    );
    let g = h.group();
    let mut procs = Vec::new();
    for args in [&["test"][..], &["--bogus"][..]] {
        let (tx, rx) = mpsc::channel();
        let log = log.clone();
        h.host.spawn(
            g,
            h.req(args),
            Box::new(move |r| {
                let p = r.expect("it spawns");
                log.lock().unwrap().push(format!("done {}", p.0));
                let _ = tx.send(p);
            }),
        );
        procs.push(rx.recv_timeout(WAIT).unwrap());
    }
    h.host.kill_group(g, KillReason::Test, Box::new(|| {}));
    for &p in &procs {
        h.exit(p);
        match h.next(|e| matches!(e, HostEvent::Spawned { proc, .. } if *proc == p)) {
            HostEvent::Spawned { group, .. } => assert_eq!(group, g),
            _ => unreachable!(),
        }
        let log = log.lock().unwrap();
        let mine: Vec<&String> = log
            .iter()
            .filter(|l| l.ends_with(&format!(" {}", p.0)))
            .collect();
        assert_eq!(mine[0], &format!("done {}", p.0), "{log:?}");
        assert_eq!(mine[1], &format!("spawned {}", p.0), "{log:?}");
        assert!(mine[2..].iter().all(|l| l.starts_with("event")), "{log:?}");
    }
}

/// Once its waiter reaped it, a process is never signalled by pid again (the pid may be
/// reused), even while an abandoned process's exit is still held (`abandon_for_test`).
#[test]
fn kill_by_pid_skips_a_reaped_process() {
    let mut h = host();
    let g = h.group();
    let leader = h.worker(g);
    let p = h.worker(g);
    h.hello(leader);
    h.hello(p);
    h.host.abandon_for_test(p, Duration::from_secs(5));
    assert!(h.host.kill_proc_for_test(p), "a live process is signalled");
    let until = std::time::Instant::now() + Duration::from_secs(3);
    while h.host.kill_proc_for_test(p) {
        assert!(
            std::time::Instant::now() < until,
            "still signalled 3 s after its kill, during the held exit"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        h.events_within(Duration::ZERO)
            .iter()
            .all(|e| !matches!(e, HostEvent::Exit { proc, .. } if *proc == p)),
        "the exit is still held"
    );
}

#[test]
fn bundled_program_resolves_only_known_names() {
    let h = host();
    let g = h.group();
    for name in ["sh", "../fin-workers", "fin-analyze"] {
        let mut req = h.req(&["test"]);
        req.program = Program::Bundled(name.into());
        let err = h.spawn(g, req).expect_err(name);
        assert!(matches!(err, HostError::Program(_)), "{name}: {err}");
        assert_eq!(err.code(), "WORKER_UNAVAILABLE");
    }
    let mut req = h.req(&["test"]);
    req.program = Program::Path("relative/fin-workers".into());
    assert!(matches!(h.spawn(g, req), Err(HostError::Program(_))));
}

#[test]
fn env_is_the_allow_list() {
    let mut h = host();
    let (_, v) = h.run("test.echo", json!({ "value": 1, "report": ["env", "cwd"] }));
    let env: BTreeMap<String, String> = serde_json::from_value(v["env"].clone()).unwrap();
    let want: BTreeMap<String, String> = base_env(h.dir.path())
        .into_iter()
        .map(|(k, v)| (k.into_string().unwrap(), v.into_string().unwrap()))
        .collect();
    assert_eq!(env, want);
    let cwd = std::fs::canonicalize(v["cwd"].as_str().unwrap()).unwrap();
    assert_eq!(cwd, std::fs::canonicalize(h.dir.path()).unwrap());
}

#[test]
fn protocol_errors_kill_the_process() {
    let mut h = host();
    for (what, want) in [
        ("json", "json"),
        ("seq", "seq"),
        ("long", "long"),
        ("otherJob", "otherJob"),
        ("unknownType", "unknownType"),
    ] {
        let g = h.group();
        let p = h.worker(g);
        h.hello(p);
        h.start(p, "test.garbage", json!({ "what": what }));
        let exit = h.exit(p);
        assert_eq!(
            exit.reason,
            ExitReason::ProtocolError(want.into()),
            "{what}"
        );
        assert!(killed(&exit), "{what}: {exit:?}");
    }
}

#[test]
fn protocol_mismatch_on_bad_hello() {
    let mut h = host();
    let g = h.group();
    let p = h.spawn(g, h.req(&["testbad"])).unwrap();
    let exit = match h.next_of(p) {
        HostEvent::Exit { info, .. } => info,
        e => panic!("no hello is forwarded, got {e:?}"),
    };
    assert_eq!(
        exit.reason,
        ExitReason::ProtocolMismatch {
            expected: fin_jobs::envelope_id().into(),
            got: "0000000000000000".into(),
        }
    );
}

#[test]
fn plugin_minor_unknown_type_gets_error_reply() {
    let mut h = host();
    let g = h.group();
    let mut req = h.req(&["testplugin"]);
    req.program = Program::Path(BIN.into());
    req.profile = Profile::Plugin;
    let p = h.spawn(g, req).unwrap();
    let hello = h.hello(p);
    assert_eq!((hello.minor, hello.envelope), (1, None));
    h.start(p, "test.pluginMinor", json!({}));
    let v = h.result(p);
    assert_eq!(v["reply"]["error"]["code"], "UNKNOWN_MESSAGE", "{v}");
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[test]
fn bundled_unknown_type_is_a_protocol_error() {
    let mut h = host();
    let g = h.group();
    // `testplugin` as a bundled program: its newer minor earns no leniency.
    let p = h.spawn(g, h.req(&["testplugin"])).unwrap();
    let exit = h.exit(p);
    assert!(
        matches!(exit.reason, ExitReason::ProtocolMismatch { .. }),
        "{exit:?}"
    );
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.garbage", json!({ "what": "unknownType" }));
    assert_eq!(
        h.exit(p).reason,
        ExitReason::ProtocolError("unknownType".into())
    );
}

#[test]
fn no_hello_times_out() {
    let mut h = host_with(|o| o.timing.hello_ms = 300);
    let g = h.group();
    let p = h.spawn(g, h.req(&["--test-sleep", "20000"])).unwrap();
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::NoHello);
    assert!(
        exit.lifetime_ms >= 300 && exit.lifetime_ms < 5_000,
        "{exit:?}"
    );
}

#[test]
fn hung_is_detected() {
    let mut h = host_with(|o| o.timing.hung_ms = 300);
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.hang", json!({}));
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Hung);
    assert!(exit.lifetime_ms < 5_000, "{exit:?}");
}

#[test]
fn alive_keeps_a_quiet_worker_from_hanging() {
    let mut h = host_with(|o| o.timing.hung_ms = 400);
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.sleep", json!({ "ms": 1200 }));
    assert!(h.result(p)["sleptMs"].as_u64().unwrap() >= 1200);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[test]
fn stall_is_detected() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.stall", json!({}));
    h.host.watch(
        p,
        Some(Watch {
            job: "j1".into(),
            stall_ms: Some(300),
            timeout_ms: None,
        }),
    );
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Stalled);
}

#[test]
fn progress_keeps_a_job_from_stalling() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.sleep",
        json!({ "ms": 1000, "progressEveryMs": 50 }),
    );
    let w = Watch {
        job: "j1".into(),
        stall_ms: Some(300),
        timeout_ms: None,
    };
    h.host.watch(p, Some(w));
    h.result(p);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

#[test]
fn timeout_is_detected() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(
        p,
        "test.sleep",
        json!({ "ms": 20000, "progressEveryMs": 20 }),
    );
    h.host.watch(
        p,
        Some(Watch {
            job: "j1".into(),
            stall_ms: Some(10_000),
            timeout_ms: Some(300),
        }),
    );
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::TimedOut);
}

#[test]
fn result_then_immediate_exit_is_success() {
    let mut h = host();
    let runs = 1000;
    let batch = 16;
    let mut done = 0;
    while done < runs {
        let n = batch.min(runs - done);
        let procs: Vec<ProcId> = (0..n)
            .map(|_| {
                let g = h.group();
                h.worker(g)
            })
            .collect();
        for &p in &procs {
            h.hello(p);
            h.start(p, "test.exitAfterResult", json!({}));
        }
        for &p in &procs {
            match h.next_of(p) {
                HostEvent::Message {
                    msg: WorkerMsg::Result { .. },
                    ..
                } => {}
                e => panic!("run {done}: the result comes before the exit, got {e:?}"),
            }
            let exit = h.exit(p);
            assert_eq!(
                (exit.reason, exit.code),
                (ExitReason::Exited, Some(0)),
                "run {done}"
            );
            done += 1;
        }
    }
}

#[test]
fn cancel_grace_then_kill() {
    let mut h = host();
    // Answers the cancel: ends `JOB_CANCELLED` and exits 0 within the grace.
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.sleep", json!({ "ms": 60000 }));
    h.host
        .send(
            p,
            &HostMsg::Cancel {
                job: "j1".into(),
                reason: CancelReason::Cancel,
            },
        )
        .unwrap();
    match h.message(p) {
        WorkerMsg::Error { error, .. } => assert_eq!(error.code, "JOB_CANCELLED"),
        m => panic!("an error, got {m:?}"),
    }
    let exit = h.exit(p);
    h.kill(g, KillReason::Cancel);
    assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(0)));
    // Ignores it (`test.hang` reads nothing): killed after the grace.
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.hang", json!({}));
    let _ = h.host.send(
        p,
        &HostMsg::Cancel {
            job: "j1".into(),
            reason: CancelReason::Cancel,
        },
    );
    std::thread::sleep(Duration::from_millis(300));
    h.kill(g, KillReason::Cancel);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::Killed(KillReason::Cancel));
    assert!(killed(&exit), "{exit:?}");
}

#[test]
fn writer_queue_full_kills_as_hung() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.start(p, "test.hang", json!({}));
    std::thread::sleep(Duration::from_millis(100));
    let big = HostMsg::Reply {
        id: 1,
        value: Some(Value::String("x".repeat(4096))),
        error: None,
    };
    let full = (0..300)
        .map(|_| h.host.send(p, &big))
        .position(|r| r == Err(SendError::QueueFull));
    assert!(
        full.is_some(),
        "300 messages fill the queue of a process that does not read"
    );
    assert_eq!(h.exit(p).reason, ExitReason::Hung);
    assert_eq!(h.host.send(p, &big), Err(SendError::Gone));
}

#[test]
fn stderr_tail_is_kept_and_capped() {
    let mut h = host();
    let g = h.group();
    // An unknown family named by 20,000 characters: one long usage line on stderr, exit 2.
    let name = "x".repeat(20_000);
    let p = h.spawn(g, h.req(&[&name])).unwrap();
    let mut lines = Vec::new();
    let exit = loop {
        match h.next(|e| proc_of(e) == Some(p)) {
            HostEvent::Stderr { line, .. } => lines.push(line),
            HostEvent::Exit { info, .. } => break info,
            HostEvent::Spawned { .. } => {}
            e => panic!("{e:?}"),
        }
    };
    assert_eq!((exit.reason, exit.code), (ExitReason::Exited, Some(2)));
    assert!(
        exit.stderr_tail.len() <= 8 << 10,
        "{}",
        exit.stderr_tail.len()
    );
    assert!(exit.stderr_tail.contains("usage: fin-workers"));
    assert!(lines.iter().all(|l| l.len() <= 4 << 10));
    assert!(lines[0].starts_with("fin-workers: unknown family"));
    assert!(lines.iter().any(|l| l.starts_with("usage: fin-workers")));
}

#[test]
fn exit_reports_code_and_signal() {
    let mut h = host();
    #[cfg(unix)]
    let cases = [("exit", Some(3), None), ("segv", None, Some("segv"))];
    // Windows: the NTSTATUS of the exception (STATUS_ACCESS_VIOLATION), never a signal.
    #[cfg(windows)]
    let cases = [("exit", Some(3), None), ("segv", Some(0xC000_0005), None)];
    for (how, code, signal) in cases {
        let g = h.group();
        let p = h.worker(g);
        h.hello(p);
        h.start(
            p,
            "test.crash",
            json!({ "items": ["a"], "poison": ["a"], "how": how }),
        );
        let exit = h.exit(p);
        assert_eq!(exit.reason, ExitReason::Exited, "{how}");
        assert_eq!((exit.code, exit.signal), (code, signal), "{how}");
    }
}

#[test]
fn spawner_prepare_can_rewrite_args() {
    let mut h = host_with(|o| o.spawner = rewrite(|cmd| cmd.args.push("--warm".into())));
    let g = h.group();
    let p = h.worker(g);
    assert!(h.hello(p).warm, "the rewritten argument reached the worker");
    h.start(p, "test.echo", json!({ "value": 1, "report": ["args"] }));
    let args: Vec<String> = serde_json::from_value(h.result(p)["args"].clone()).unwrap();
    assert_eq!(&args[1..], ["test", "--warm"]);
    assert_eq!(h.message(p), WorkerMsg::Ready {});
    assert_eq!(h.host.caps().sandbox, "rewrite");
}

#[test]
fn unsandboxed_caps() {
    let h = host();
    let caps = h.host.caps();
    assert_eq!(caps.sandbox, "none");
    assert!(!caps.sandboxed);
    assert!(!caps.watchdog);
    #[cfg(unix)]
    {
        assert_eq!(caps.kill_tree, KillTree::ProcessGroup);
        assert!(!caps.leaf_limits);
        assert!(!caps.cgroup_memory);
    }
    #[cfg(windows)]
    {
        assert_eq!(caps.kill_tree, KillTree::JobObject);
        assert!(caps.leaf_limits);
        assert!(caps.cgroup_memory);
    }
}

#[test]
fn self_sandbox_fails_closed() {
    let mut h = host_with(|o| {
        o.spawner = rewrite(|cmd| {
            cmd.self_sandbox = Some(spawner::SelfSandbox {
                profile: "worker".into(),
                args: Value::Null,
            })
        })
    });
    let g = h.group();
    let p = h.worker(g);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::SpawnFailed("selfSandbox".into()));
    assert_eq!(exit.code, Some(i64::from(fin_jobs::EXIT_SELF_SANDBOX)));
    // `fin_sandbox::self_apply` is registered: a request it cannot read fails before `hello`.
    assert!(
        exit.stderr_tail.contains("unreadable worker input"),
        "{}",
        exit.stderr_tail
    );
}

#[test]
fn self_sandbox_mismatch_is_protocol_error() {
    let mut h = host_with(|o| {
        o.spawner = rewrite(|cmd| {
            cmd.env
                .push(("FIN_TEST_FAKE_SELF_SANDBOX".into(), "bogus".into()));
        })
    });
    let g = h.group();
    let p = h.worker(g);
    assert_eq!(
        h.exit(p).reason,
        ExitReason::ProtocolError("selfSandbox".into())
    );
}

#[test]
fn spawn_into_a_closed_group_fails() {
    let mut h = host();
    let g = h.group();
    let p = h.worker(g);
    h.hello(p);
    h.host.close_group(g);
    assert_eq!(h.exit(p).reason, ExitReason::Killed(KillReason::JobEnded));
    let err = h.spawn(g, h.req(&["test"])).unwrap_err();
    assert!(matches!(err, HostError::GroupClosed(_)), "{err}");
}

#[test]
fn shutdown_closes_stdin_then_kills() {
    let mut h = host();
    let quiet = h.group();
    let p1 = h.worker(quiet);
    h.hello(p1);
    let stuck = h.group();
    let p2 = h.worker(stuck);
    h.hello(p2);
    h.start(p2, "test.hang", json!({}));
    // Let `test.hang` stop reading first, or it may see the stdin EOF and exit 70.
    std::thread::sleep(Duration::from_millis(300));
    let (tx, rx) = mpsc::channel();
    h.host.shutdown(
        300,
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    let e1 = h.exit(p1);
    assert_eq!(
        (e1.reason, e1.code),
        (ExitReason::Killed(KillReason::Shutdown), Some(70))
    );
    let e2 = h.exit(p2);
    assert_eq!(e2.reason, ExitReason::Killed(KillReason::Shutdown));
    assert!(killed(&e2), "{e2:?}");
    rx.recv_timeout(WAIT).expect("shutdown answers");
    assert!(matches!(
        h.spawn(quiet, h.req(&["test"])),
        Err(HostError::ShutDown)
    ));
}

/// A member's leftover (a grandchild in the group, not in a session of its own) dies when the
/// group is released, even when every process the host started has already exited: on
/// Windows the group Job is terminated, a cgroup is killed, a process group gets
/// `kill(-pgid)` while its leader is still unreaped.
#[test]
fn close_group_ends_what_a_member_left() {
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
    // The worker ends on its own: the host kills nothing.
    kill_pid(pids.worker);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
    h.group_empty(g);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!gone(pids.grandchild), "left behind until the release");
    h.host.close_group(g);
    assert!(
        wait_gone(pids.grandchild),
        "the grandchild {} outlived its group's release",
        pids.grandchild
    );
}

/// A worker's `result` may arrive before `send(start)` returns: the hook holds `send` until
/// the reader has handled the answer, so a start named only after it is queued would read
/// the result as `otherJob`, every time.
#[test]
fn an_answer_before_send_returns_is_on_its_job() {
    use std::sync::{Arc, Condvar, Mutex};
    let answered = Arc::new((Mutex::new(false), Condvar::new()));
    let (seen, wait) = (answered.clone(), answered.clone());
    let mut h = host_observed(
        |o| {
            o.after_send_for_test = Some(Arc::new(move |_, msg| {
                if !matches!(msg, HostMsg::Start(_)) {
                    return;
                }
                let (m, cv) = &*wait;
                let g = m.lock().unwrap();
                let (g, t) = cv
                    .wait_timeout_while(g, WAIT, |answered| !*answered)
                    .unwrap();
                assert!(*g && !t.timed_out(), "the worker answered while send held");
            }));
        },
        move |e| {
            if matches!(
                e,
                HostEvent::Message {
                    msg: WorkerMsg::Result { .. },
                    ..
                } | HostEvent::Exit { .. }
            ) {
                let (m, cv) = &*seen;
                *m.lock().unwrap() = true;
                cv.notify_all();
            }
        },
    );
    let (p, v) = h.run("test.echo", json!({ "value": 7 }));
    assert_eq!(v["value"], 7);
    h.host.close_stdin(p);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
}

/// A spawn whose waiter thread does not start: the spawn thread kills the process, waits for
/// it and still reports its `Exit` (through its handle on Windows, its pid on Unix).
#[test]
fn a_missing_waiter_thread_still_reports_the_exit() {
    let mut h = host_with(|o| o.fail_waiter_thread_for_test = true);
    let g = h.group();
    let p = h.worker(g);
    let exit = h.exit(p);
    assert_eq!(exit.reason, ExitReason::SpawnFailed("thread".into()));
    assert!(killed(&exit), "{exit:?}");
    h.group_empty(g);
    h.host.close_group(g);
}
