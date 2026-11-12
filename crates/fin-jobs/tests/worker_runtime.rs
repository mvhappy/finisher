//! The worker runtime (Decision 6, 7, 9a) over in-memory pipes with toy families, and in a
//! child process (this test binary re-executed) where it must exit the process.

mod support;

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fin_jobs::sandbox::{self, SelfApplied, SelfSandbox};
use fin_jobs::worker::{self, JobCtx};
use fin_jobs::*;
use serde_json::{Value, json};
use support::*;

#[test]
fn hello_first_then_result_then_exit_cold() {
    let mut rig = Rig::worker(echo());
    let WorkerMsg::Hello(h) = rig.recv_any() else {
        panic!("hello first")
    };
    assert_eq!(
        (h.v, h.minor, h.role, h.pid),
        (PROTOCOL, MINOR, Role::Worker, PID)
    );
    assert_eq!(h.family, "toy");
    assert_eq!(h.kinds, [KindVersion::new("toy.run", 1)]);
    assert!(!h.warm);
    assert_eq!(h.envelope.as_deref(), Some(envelope_id()));
    assert_eq!(h.self_sandbox, None);
    rig.send(&start("j1", "toy.run", json!({ "a": 1 })));
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "j1".into(),
            value: json!({ "a": 1 })
        }
    );
    assert_eq!(rig.join_open(), 0, "a cold worker exits 0 after its job");
}

#[test]
fn warm_sends_ready_and_takes_the_next_start() {
    let mut served = 0u64;
    let mut rig = Rig::worker(warm_toy(move |job, _| {
        served += 1;
        Ok(json!({ "job": job.id, "served": served }))
    }));
    let WorkerMsg::Hello(h) = rig.recv() else {
        panic!()
    };
    assert!(h.warm);
    for n in 1..=3u64 {
        let id = format!("j{n}");
        rig.send(&start(&id, "toy.run", json!({})));
        assert_eq!(
            rig.recv(),
            WorkerMsg::Result {
                job: id.clone(),
                value: json!({ "job": id, "served": n })
            }
        );
        assert_eq!(rig.recv(), WorkerMsg::Ready {});
    }
    rig.send(&HostMsg::Shutdown {});
    assert_eq!(rig.join_open(), 0);
}

#[test]
fn alive_every_interval_from_its_own_thread() {
    let mut rig = Rig::worker(toy(|_, _| {
        std::thread::sleep(Duration::from_millis(500));
        Ok(json!(null))
    }));
    rig.recv();
    rig.send(&start_with("j", json!({}), |s| s.limits.alive_ms = 40));
    let mut alive = 0;
    loop {
        match rig.recv_any() {
            WorkerMsg::Alive {} => alive += 1,
            WorkerMsg::Result { .. } => break,
            m => panic!("{m:?}"),
        }
    }
    assert!(alive >= 6, "{alive} alive messages in 500 ms at 40 ms");
    rig.join();
}

#[test]
fn cancel_sets_the_token() {
    let mut rig = Rig::worker(toy(|_, ctx| {
        let token = ctx.cancel_token();
        while !token.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(ctx.cancelled(), Some(CancelReason::Cancel));
        Err(JobError::cancelled())
    }));
    rig.recv();
    rig.send(&start("j1", "toy.run", json!({})));
    // A cancel for another job does not reach this one.
    rig.send(&HostMsg::Cancel {
        job: "other".into(),
        reason: CancelReason::Cancel,
    });
    assert!(rig.quiet_for(Duration::from_millis(150)));
    rig.send(&HostMsg::Cancel {
        job: "j1".into(),
        reason: CancelReason::Cancel,
    });
    let WorkerMsg::Error { job, error, .. } = rig.recv() else {
        panic!()
    };
    assert_eq!((job.as_str(), error.code.as_str()), ("j1", "JOB_CANCELLED"));
    assert_eq!(rig.join_open(), 0);
}

#[test]
fn start_and_cancel_in_one_write() {
    // The `cancel` right behind the `start` reaches the job even though both lines arrive in
    // one read, before the run loop has picked the job up.
    let mut rig = Rig::worker(toy(|_, ctx| {
        let token = ctx.cancel_token();
        for _ in 0..1000 {
            if token.is_cancelled() {
                return Err(JobError::cancelled());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(json!("never cancelled"))
    }));
    rig.recv();
    rig.send_together(&[
        start("j1", "toy.run", json!({})),
        HostMsg::Cancel {
            job: "j1".into(),
            reason: CancelReason::Cancel,
        },
    ]);
    let WorkerMsg::Error { error, .. } = rig.recv() else {
        panic!("an error")
    };
    assert_eq!(error.code, "JOB_CANCELLED");
    rig.join();
}

#[test]
fn cancel_upgrades_pause() {
    let t = worker::CancelToken::new();
    assert_eq!(t.reason(), None);
    t.cancel(CancelReason::Pause);
    assert_eq!(t.reason(), Some(CancelReason::Pause));
    t.cancel(CancelReason::Cancel);
    assert_eq!(
        t.reason(),
        Some(CancelReason::Cancel),
        "a cancel upgrades a pause"
    );
    t.cancel(CancelReason::Pause);
    assert_eq!(
        t.reason(),
        Some(CancelReason::Cancel),
        "a pause never downgrades a cancel"
    );
    // Through the runtime: a pause, then a cancel, ends as a cancel.
    let mut rig = Rig::worker(toy(|_, ctx| {
        while ctx.cancelled() != Some(CancelReason::Cancel) {
            std::thread::sleep(Duration::from_millis(5));
        }
        Err(JobError::cancelled())
    }));
    rig.recv();
    rig.send(&start("j1", "toy.run", json!({})));
    for reason in [CancelReason::Pause, CancelReason::Cancel] {
        rig.send(&HostMsg::Cancel {
            job: "j1".into(),
            reason,
        });
    }
    assert!(matches!(rig.recv(), WorkerMsg::Error { error, .. } if error.code == "JOB_CANCELLED"));
    rig.join();
}

#[test]
fn bad_host_line_exits_65() {
    for bad in [
        &b"{nope\n"[..],
        b"{\"v\":1,\"t\":\"shutdown\",\"seq\":2}\n",
        b"{\"v\":1,\"t\":\"bogus\",\"seq\":1}\n",
    ] {
        let mut rig = Rig::worker(echo());
        rig.recv();
        rig.send_raw(bad);
        assert_eq!(
            rig.join_open(),
            EXIT_HOST_PROTOCOL,
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn plugin_mode_hello_and_unknown_host_types() {
    let mut rig = Rig::plugin(warm_toy(|job, _| Ok(job.params.clone())), 3);
    let WorkerMsg::Hello(h) = rig.recv_any() else {
        panic!("hello first")
    };
    assert_eq!((h.v, h.minor, h.warm), (PROTOCOL, 3, true));
    assert_eq!(h.envelope, None, "a plugin sends no envelope id");
    // seq 1 and 2: types a newer host may send, with and without an id; dropped.
    rig.send_raw(b"{\"v\":1,\"t\":\"future\",\"seq\":1}\n");
    rig.send_raw(b"{\"v\":1,\"t\":\"later\",\"seq\":2,\"id\":9}\n");
    rig.skip_seq(2);
    rig.send(&start("j1", "toy.run", json!({ "a": 1 })));
    assert!(matches!(rig.recv(), WorkerMsg::Result { job, .. } if job == "j1"));
    assert_eq!(rig.recv(), WorkerMsg::Ready {});
    // Every other rule still holds: a seq gap is fatal.
    rig.send_raw(b"{\"v\":1,\"t\":\"future\",\"seq\":9}\n");
    assert_eq!(rig.join_open(), EXIT_HOST_PROTOCOL);
}

#[test]
fn plugin_mode_keeps_a_malformed_known_type_fatal() {
    let mut rig = Rig::plugin(warm_toy(|job, _| Ok(job.params.clone())), 3);
    rig.recv_any();
    // A `start` without its job: a known type of a bad shape.
    rig.send_raw(b"{\"v\":1,\"t\":\"start\",\"seq\":1}\n");
    assert_eq!(rig.join_open(), EXIT_HOST_PROTOCOL);
}

#[test]
fn pause_reason_reaches_the_family() {
    let mut rig = Rig::worker(toy(|_, ctx| {
        while ctx.cancelled().is_none() {
            std::thread::sleep(Duration::from_millis(5));
        }
        if ctx.cancelled() == Some(CancelReason::Pause) {
            ctx.checkpoint(json!({ "step": 2 }))?;
        }
        Err(JobError::cancelled().with_details(json!({ "reason": ctx.cancelled() })))
    }));
    rig.recv();
    rig.send(&start("j1", "toy.run", json!({})));
    rig.send(&HostMsg::Cancel {
        job: "j1".into(),
        reason: CancelReason::Pause,
    });
    assert_eq!(
        rig.recv(),
        WorkerMsg::Checkpoint {
            job: "j1".into(),
            data: json!({ "step": 2 })
        }
    );
    let WorkerMsg::Error { error, .. } = rig.recv() else {
        panic!()
    };
    assert_eq!(error.details, json!({ "reason": "pause" }));
    rig.join();
}

#[test]
fn panic_is_answered_as_worker_crashed_and_exits_101() {
    let mut rig = Rig::worker(warm_toy(|_, _| panic!("boom")));
    rig.recv();
    rig.send(&start("j1", "toy.run", json!({})));
    let WorkerMsg::Error { job, error, .. } = rig.recv() else {
        panic!()
    };
    assert_eq!(job, "j1");
    assert_eq!(error.code, "WORKER_CRASHED");
    assert_eq!(error.retry, Retry::Backoff);
    assert!(error.message.contains("boom"), "{}", error.message);
    // A warm worker is not reused after a panic: no `ready`, exit 101.
    assert_eq!(rig.join_open(), 101);
}

fn diag_family(ctx: &mut JobCtx<'_>) -> Result<Value, JobError> {
    ctx.diag("toy.diag", json!({ "built": true }));
    Ok(json!(ctx.diagnostics()))
}

#[test]
fn diag_is_a_no_op_when_off() {
    let mut rig = Rig::worker(warm_toy(|_, ctx| diag_family(ctx)));
    rig.recv();
    rig.send(&start("off", "toy.run", json!({})));
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "off".into(),
            value: json!(false)
        }
    );
    assert_eq!(rig.recv(), WorkerMsg::Ready {});
    rig.send(&start_with("on", json!({}), |s| {
        s.job.id = "on".into();
        s.diagnostics = true;
    }));
    assert_eq!(
        rig.recv(),
        WorkerMsg::Diag {
            job: Some("on".into()),
            kind: "toy.diag".into(),
            payload: json!({ "built": true })
        }
    );
    assert!(matches!(rig.recv(), WorkerMsg::Result { .. }));
    rig.join();
}

#[test]
fn reporter_works_from_another_thread() {
    let mut rig = Rig::worker(toy(|_, ctx| {
        let r = ctx.reporter();
        std::thread::spawn(move || {
            r.begin("a");
            r.progress(Progress::new(1.0, ProgressUnit::Items).of(2.0));
            r.item("a", Some(json!(10)));
            r.item_failed("b", JobError::new("BAD_ITEM", "no"));
        })
        .join()
        .unwrap();
        Ok(json!("done"))
    }));
    rig.recv();
    rig.send(&start("j9", "toy.run", json!({})));
    assert_eq!(
        rig.recv(),
        WorkerMsg::Begin {
            job: "j9".into(),
            item: "a".into()
        }
    );
    let WorkerMsg::Progress(p) = rig.recv() else {
        panic!()
    };
    assert_eq!((p.job.as_str(), p.done, p.total), ("j9", 1.0, Some(2.0)));
    assert!(matches!(rig.recv(), WorkerMsg::Item { item, value: Some(_), .. } if item == "a"));
    assert!(matches!(rig.recv(), WorkerMsg::Item { item, error: Some(_), .. } if item == "b"));
    assert!(matches!(rig.recv(), WorkerMsg::Result { .. }));
    rig.join();
}

#[test]
fn ctx_exposes_files_dirs_limits_and_budget() {
    let mut rig = Rig::worker(toy(|job, ctx| {
        assert_eq!(job.attempt, 1);
        assert_eq!(ctx.files().len(), 1);
        assert_eq!(ctx.file("in.bin").map(|f| f.mode), Some(FileMode::Read));
        assert!(ctx.file("nope").is_none());
        assert_eq!(ctx.dirs().tmp, std::path::PathBuf::from("/job/tmp"));
        assert_eq!(ctx.limits().read_bytes_per_sec, Some(1000));
        let b = ctx.read_budget().expect("a budget");
        b.take(10);
        assert!(ctx.helpers().is_empty());
        let big = json!("x".repeat(codec::MAX_CHECKPOINT));
        Ok(json!(ctx.checkpoint(big).unwrap_err().code))
    }));
    rig.recv();
    rig.send(&start_with("j", json!({}), |s| {
        s.limits.read_bytes_per_sec = Some(1000);
        s.files.push(FileGrant {
            name: "in.bin".into(),
            path: "/job/in.bin".into(),
            mode: FileMode::Read,
            bytes: None,
        });
    }));
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "j".into(),
            value: json!("CHECKPOINT_TOO_LARGE")
        }
    );
    rig.join();
}

static APPLIED: AtomicBool = AtomicBool::new(false);

fn test_sandbox(s: &SelfSandbox) -> Result<SelfApplied, String> {
    if s.profile == "fail" {
        return Err("refused".into());
    }
    if s.profile == "off" {
        // The mechanism is disabled on this host (F2): nothing applied, the process runs.
        return Ok(SelfApplied::Nothing);
    }
    // Slow, so a `hello` sent before the sandbox would be seen while it is not yet applied.
    std::thread::sleep(Duration::from_millis(200));
    APPLIED.store(true, Ordering::SeqCst);
    Ok(SelfApplied::Applied)
}

#[test]
fn self_sandbox_entry_runs_before_hello() {
    sandbox::register(test_sandbox);
    let mut rig = Rig::worker_sandboxed(echo(), r#"{"profile":"worker","args":{"deny":["exec"]}}"#);
    let WorkerMsg::Hello(h) = rig.recv_any() else {
        panic!()
    };
    assert!(
        APPLIED.load(Ordering::SeqCst),
        "hello came before the sandbox"
    );
    assert_eq!(h.self_sandbox.as_deref(), Some("worker"));
    rig.join();
    // Applied nothing (F2): `hello` says so, so the host never takes it for confined.
    let mut rig = Rig::worker_sandboxed(echo(), r#"{"profile":"off"}"#);
    let WorkerMsg::Hello(h) = rig.recv_any() else {
        panic!()
    };
    assert_eq!(h.self_sandbox.as_deref(), Some(sandbox::NONE));
    rig.join();
    // A function that fails, or an unreadable request, exits 78 before `hello`.
    for request in [r#"{"profile":"fail"}"#, "not json"] {
        let rig = Rig::worker_sandboxed(echo(), request);
        assert_eq!(rig.join_open(), EXIT_SELF_SANDBOX, "{request}");
    }
}

/// Re-executes this test binary as a worker over real stdio (`FIN_JOBS_CHILD` picks it).
fn child(mode: &str, sandbox_request: Option<&str>) -> std::process::Child {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "child_worker",
        "--exact",
        "--nocapture",
        "--test-threads",
        "1",
    ])
    .env("FIN_JOBS_CHILD", mode)
    .env_remove(sandbox::ENV)
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::null());
    if let Some(r) = sandbox_request {
        cmd.env(sandbox::ENV, r);
    }
    cmd.spawn().unwrap()
}

#[test]
fn child_worker() {
    match std::env::var("FIN_JOBS_CHILD").as_deref() {
        Ok("sleeper") => fin_jobs::worker::run(toy(|_, _| {
            std::thread::sleep(Duration::from_secs(60));
            Ok(json!(null))
        })),
        Ok("echo") => fin_jobs::worker::run(echo()),
        _ => {}
    }
}

/// Reads the child's stdout up to its `hello` (the test harness prints its own lines first).
fn wait_hello(c: &mut std::process::Child) -> BufReader<std::process::ChildStdout> {
    let mut r = BufReader::new(c.stdout.take().unwrap());
    let mut line = String::new();
    loop {
        line.clear();
        assert!(r.read_line(&mut line).unwrap() > 0, "no hello");
        if line.contains(r#""t":"hello""#) {
            return r;
        }
    }
}

fn wait_exit(c: &mut std::process::Child, within: Duration) -> i32 {
    let until = Instant::now() + within;
    loop {
        if let Some(s) = c.try_wait().unwrap() {
            return s.code().expect("an exit code");
        }
        assert!(Instant::now() < until, "the child did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stdin_eof_exits_70() {
    // Idle, and in the middle of a job.
    let mut c = child("echo", None);
    let _out = wait_hello(&mut c);
    drop(c.stdin.take());
    assert_eq!(wait_exit(&mut c, Duration::from_secs(5)), EXIT_HOST_GONE);

    let mut c = child("sleeper", None);
    let _out = wait_hello(&mut c);
    let mut stdin = c.stdin.take().unwrap();
    let line = codec::encode(1, &start("j", "toy.run", json!({}))).unwrap();
    std::io::Write::write_all(&mut stdin, &line).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    drop(stdin);
    assert_eq!(wait_exit(&mut c, Duration::from_secs(5)), EXIT_HOST_GONE);
    assert!(
        t.elapsed() < Duration::from_secs(2),
        "exited at once, not after the job"
    );
}

#[test]
fn self_sandbox_requested_but_unregistered_exits_78() {
    let mut c = child("echo", Some(r#"{"profile":"worker"}"#));
    let mut out = String::new();
    c.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert_eq!(wait_exit(&mut c, Duration::from_secs(5)), EXIT_SELF_SANDBOX);
    assert!(!out.contains(r#""t":"hello""#), "no hello: {out}");
}

#[cfg(windows)]
#[test]
fn windows_crash_ui_is_off() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
    };
    let rig = Rig::worker(echo());
    rig.join();
    // SAFETY: reads a process-wide flag.
    let mode = unsafe { GetErrorMode() };
    let want = SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX;
    assert_eq!(mode & want, want, "error mode {mode:#x}");
}
