//! `fin-workers` with the `test` families, driven through the binary by a minimal host over
//! pipes (5C-1 plan, Task 2).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use fin_jobs::codec::{self, Decoded};
use fin_jobs::*;
use serde_json::{Value, json};

const BIN: &str = env!("CARGO_BIN_EXE_fin-workers");
const WAIT: Duration = Duration::from_secs(10);

/// A worker process and a host's end of its pipes. Lines are read raw (no cap) so the
/// garbage kinds can be inspected; `recv` decodes them in order.
struct Proc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Vec<u8>>,
    host_seq: u64,
    worker_seq: u64,
    lenient: bool,
}

fn spawn(args: &[&str]) -> Proc {
    let mut child = Command::new(BIN)
        .args(args)
        .env_clear()
        .env("FIN_TEST_MARKER", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match r.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if tx.send(line).is_err() {
                        return;
                    }
                }
            }
        }
    });
    Proc {
        stdin: child.stdin.take(),
        child,
        lines,
        host_seq: 0,
        worker_seq: 0,
        lenient: false,
    }
}

impl Proc {
    fn raw(&mut self) -> Vec<u8> {
        let line = self.lines.recv_timeout(WAIT).expect("a line within 10 s");
        self.worker_seq += 1;
        line
    }

    fn recv_any(&mut self) -> Decoded<WorkerMsg> {
        let line = self.raw();
        codec::decode_worker(&line, self.worker_seq, self.lenient)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&line)))
    }

    /// The next decoded message other than `alive`.
    fn recv(&mut self) -> WorkerMsg {
        loop {
            match self.recv_any() {
                Decoded::Msg(WorkerMsg::Alive {}) => continue,
                Decoded::Msg(m) => return m,
                Decoded::Unknown { t, .. } => panic!("unknown message {t}"),
            }
        }
    }

    fn hello(&mut self) -> Hello {
        match self.recv() {
            WorkerMsg::Hello(h) => h,
            m => panic!("hello first, got {m:?}"),
        }
    }

    fn send(&mut self, msg: &HostMsg) {
        self.host_seq += 1;
        let line = codec::encode(self.host_seq, msg).unwrap();
        self.stdin.as_mut().unwrap().write_all(&line).unwrap();
    }

    fn result(&mut self) -> Value {
        match self.recv() {
            WorkerMsg::Result { value, .. } => value,
            m => panic!("a result, got {m:?}"),
        }
    }

    fn wait(&mut self) -> ExitStatus {
        let until = Instant::now() + WAIT;
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return s;
            }
            assert!(Instant::now() < until, "the process did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn dirs(job: &Path) -> Dirs {
    Dirs {
        job: job.to_path_buf(),
        tmp: job.join("tmp"),
    }
}

fn start(id: &str, kind: &str, params: Value) -> Start {
    Start {
        job: StartJob {
            id: id.into(),
            kind: kind.into(),
            kind_version: 1,
            params,
            checkpoint: None,
            attempt: 1,
            extra: None,
        },
        limits: Limits::default(),
        files: vec![],
        helpers: vec![],
        dirs: dirs(Path::new("/nonexistent-job")),
        diagnostics: false,
    }
}

/// Spawns `test`, checks the hello, and starts one job.
fn run(kind: &str, params: Value) -> Proc {
    run_start(start("j1", kind, params))
}

fn run_start(s: Start) -> Proc {
    let mut p = spawn(&["test"]);
    p.hello();
    p.send(&HostMsg::Start(s));
    p
}

const TEST_KINDS: [&str; 17] = [
    "test.sleep",
    "test.echo",
    "test.progressFlood",
    "test.warm",
    "test.crash",
    "test.hang",
    "test.stall",
    "test.spawn",
    "test.files",
    "test.checkpoint",
    "test.oom",
    "test.call",
    "test.diag",
    "test.garbage",
    "test.exitAfterResult",
    "test.helper",
    "test.fail",
];

#[test]
fn hello_lists_the_test_kinds() {
    use fin_core::events::names_jobs::{JOB_KINDS, WORKER_FAMILIES};
    let mut p = spawn(&["test"]);
    let h = p.hello();
    assert_eq!(
        (h.v, h.minor, h.role, h.family.as_str()),
        (1, 0, Role::Worker, "test")
    );
    let kinds: Vec<&str> = h.kinds.iter().map(|k| k.kind.as_ref()).collect();
    assert_eq!(kinds, TEST_KINDS);
    assert!(h.kinds.iter().all(|k| k.version == 1));
    assert!(!h.warm);
    assert_eq!(h.envelope.as_deref(), Some(envelope_id()));
    assert_eq!(h.pid, p.child.id());
    let mut warm = spawn(&["test", "--warm"]);
    assert!(warm.hello().warm);
    // The drift test: every family and kind the binary serves is in fin-core's tables, and
    // every test kind has its params type.
    for family in ["test", "testbad", "testplugin"] {
        let mut p = spawn(&[family]);
        p.lenient = true;
        let h = p.hello();
        assert_eq!(h.family, family);
        assert!(WORKER_FAMILIES.get(family).is_some(), "{family}");
        for k in &h.kinds {
            assert!(JOB_KINDS.get(&k.kind).is_some(), "{}", k.kind);
            assert!(
                kinds::find(&[test_kinds::PARAMS], &k.kind, k.version).is_some(),
                "{}",
                k.kind
            );
        }
    }
}

#[test]
fn sleep_echo_and_flood() {
    let mut p = run("test.sleep", json!({ "ms": 60, "progressEveryMs": 20 }));
    let mut progress = 0;
    let value = loop {
        match p.recv() {
            WorkerMsg::Progress(pr) => {
                assert_eq!((pr.job.as_str(), pr.unit), ("j1", ProgressUnit::Seconds));
                progress += 1;
            }
            WorkerMsg::Result { value, .. } => break value,
            m => panic!("{m:?}"),
        }
    };
    assert_eq!(progress, 3);
    assert!(value["sleptMs"].as_u64().unwrap() >= 60);
    assert!(p.wait().success());

    let mut p = run(
        "test.echo",
        json!({ "value": [1, "a"], "report": ["env", "args", "cwd", "fds"] }),
    );
    let v = p.result();
    assert_eq!(v["value"], json!([1, "a"]));
    assert_eq!(v["pid"], p.child.id());
    assert_eq!(
        v["env"],
        json!({ "FIN_TEST_MARKER": "1" }),
        "a cleared environment"
    );
    assert_eq!(v["args"][1], "test");
    #[cfg(unix)]
    assert_eq!(v["fds"], json!([0, 1, 2]), "only stdio is open");

    let mut p = run("test.progressFlood", json!({ "n": 2000 }));
    for i in 1..=2000u64 {
        let WorkerMsg::Progress(pr) = p.recv() else {
            panic!()
        };
        assert_eq!(pr.done, i as f64);
    }
    assert_eq!(p.result(), json!({ "sent": 2000 }));
}

#[test]
fn warm_serves_back_to_back_and_shuts_down() {
    let mut p = spawn(&["test", "--warm"]);
    p.hello();
    for n in 1..=3u64 {
        p.send(&HostMsg::Start(start(
            &format!("j{n}"),
            "test.warm",
            json!({ "value": n }),
        )));
        let v = p.result();
        assert_eq!(v, json!({ "value": n, "pid": p.child.id(), "served": n }));
        assert_eq!(p.recv(), WorkerMsg::Ready {});
    }
    p.send(&HostMsg::Shutdown {});
    assert!(p.wait().success());
}

#[test]
fn crash_without_poison_reports_every_item() {
    let mut p = run(
        "test.crash",
        json!({ "items": ["a", "b", "c"], "how": "abort" }),
    );
    for item in ["a", "b", "c"] {
        assert_eq!(
            p.recv(),
            WorkerMsg::Begin {
                job: "j1".into(),
                item: item.into()
            }
        );
        assert!(matches!(p.recv(), WorkerMsg::Item { item: i, error: None, .. } if i == item));
    }
    assert_eq!(p.result(), json!({ "done": ["a", "b", "c"] }));
    assert!(p.wait().success());
}

/// The exit a crash kind produces.
fn crash_exit(how: &str, extra: Value) -> (Vec<String>, ExitStatus) {
    let mut params = json!({ "items": ["a", "b", "c"], "poison": ["b"], "how": how });
    params
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let mut p = run("test.crash", params);
    let mut begun = Vec::new();
    // Until stdout closes (the process died); every line it wrote is read first.
    while let Ok(line) = p.lines.recv_timeout(WAIT) {
        let v: Value = serde_json::from_slice(&line).unwrap();
        if v["t"] == "begin" {
            begun.push(v["item"].as_str().unwrap().to_owned());
        }
    }
    (begun, p.wait())
}

#[cfg(unix)]
fn signal_of(s: ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(&s)
}

#[test]
fn crash_kinds_die_as_asked() {
    let (begun, s) = crash_exit("exit", json!({}));
    assert_eq!(begun, ["a", "b"], "the poison item is in flight");
    assert_eq!(s.code(), Some(3));
    let (begun, _) = crash_exit("exit", json!({ "between": true }));
    assert_eq!(begun, ["a"], "between: before the poison's begin");
    let (begun, _) = crash_exit("exit", json!({ "poison": [], "crashOnSecond": true }));
    assert_eq!(begun, ["a", "b"], "crashOnSecond: inside the second item");
    #[cfg(unix)]
    {
        assert_eq!(
            signal_of(crash_exit("abort", json!({})).1),
            Some(libc::SIGABRT)
        );
        assert_eq!(
            signal_of(crash_exit("segv", json!({})).1),
            Some(libc::SIGSEGV)
        );
        assert_eq!(
            signal_of(crash_exit("bus", json!({})).1),
            Some(libc::SIGBUS)
        );
    }
    #[cfg(windows)]
    {
        let code = |how| crash_exit(how, json!({})).1.code().map(|c| c as u32);
        assert_eq!(code("segv"), Some(0xC000_0005));
        assert_eq!(code("bus"), Some(0xC000_0006));
        assert_eq!(code("fastfail"), Some(0xC000_0409));
        assert_eq!(code("abort"), Some(0xC000_0409));
    }
}

#[test]
fn garbage_kinds_emit_what_they_say() {
    for what in ["json", "seq", "long", "otherJob", "unknownType"] {
        let mut p = run("test.garbage", json!({ "what": what }));
        let line = loop {
            let line = p.raw();
            let is_alive = serde_json::from_slice::<Value>(&line).is_ok_and(|v| v["t"] == "alive");
            if !is_alive || what == "seq" {
                break line;
            }
        };
        match what {
            "json" => assert!(serde_json::from_slice::<Value>(&line).is_err()),
            "seq" => {
                let v: Value = serde_json::from_slice(&line).unwrap();
                assert_eq!(v["seq"].as_u64(), Some(p.worker_seq + 1), "one seq skipped");
            }
            "long" => assert!(line.len() > codec::MAX_LINE),
            "otherJob" => assert!(matches!(
                codec::decode_worker(&line, p.worker_seq, false),
                Ok(Decoded::Msg(WorkerMsg::Progress(pr))) if pr.job == "other-job"
            )),
            _ => assert_eq!(
                codec::decode_worker(&line, p.worker_seq, false),
                Err(codec::CodecError::UnknownType("bogus".into()))
            ),
        }
        p.kill();
    }
}

#[test]
fn stall_ends_at_cancel_and_hang_ignores_everything() {
    let mut p = run("test.stall", json!({}));
    std::thread::sleep(Duration::from_millis(100));
    p.send(&HostMsg::Cancel {
        job: "j1".into(),
        reason: CancelReason::Cancel,
    });
    let WorkerMsg::Error { error, .. } = p.recv() else {
        panic!()
    };
    assert_eq!(error.code, "JOB_CANCELLED");

    let mut s = start("j1", "test.hang", json!({}));
    s.limits.alive_ms = 30;
    let mut p = run_start(s);
    std::thread::sleep(Duration::from_millis(300));
    // It no longer reads stdin: 4 MiB of host messages fill the pipe and the writes block.
    let mut stdin = p.stdin.take().unwrap();
    let (done_tx, done) = mpsc::channel();
    std::thread::spawn(move || {
        let pad = "x".repeat(16 << 10);
        for seq in 2..258u64 {
            let msg = HostMsg::Cancel {
                job: pad.clone(),
                reason: CancelReason::Cancel,
            };
            if stdin.write_all(&codec::encode(seq, &msg).unwrap()).is_err() {
                break;
            }
        }
        let _ = done_tx.send(());
    });
    assert!(
        done.recv_timeout(Duration::from_millis(500)).is_err(),
        "the host's writes block"
    );
    assert!(
        p.child.try_wait().unwrap().is_none(),
        "test.hang survives a full stdin"
    );
    let mut lines = 0;
    while p.lines.try_recv().is_ok() {
        lines += 1;
    }
    assert!(lines <= 1, "at most one alive before the hang ({lines})");
    p.kill();
}

#[test]
fn files_checkpoint_oom_call_diag_and_exit() {
    let dir = tempfile::tempdir().unwrap();
    let job = dir.path();
    std::fs::write(job.join("in.bin"), b"hello").unwrap();
    std::fs::write(job.join("out.bin"), vec![0; 64]).unwrap();
    std::fs::create_dir(job.join("out")).unwrap();
    let grant = |name: &str, mode, bytes| FileGrant {
        name: name.into(),
        path: job.join(name.trim_end_matches('/')),
        mode,
        bytes,
    };
    let mut s = start("j1", "test.files", json!({}));
    s.dirs = dirs(job);
    s.files = vec![
        grant("in.bin", FileMode::Read, None),
        grant("out.bin", FileMode::Write, Some(64)),
        grant("out/", FileMode::Write, None),
    ];
    let mut p = run_start(s.clone());
    let v = p.result();
    assert_eq!(v["outEmpty"], true);
    assert_eq!(v["files"][0]["bytes"], 5);
    assert_eq!(v["files"][1]["bytes"], 64);
    let written = std::fs::read(job.join("out.bin")).unwrap();
    assert_eq!(written[..4], [0, 1, 2, 3]);
    let mut p = run_start(s);
    assert_eq!(
        p.result()["outEmpty"],
        false,
        "out/ holds what attempt 1 left"
    );

    let mut p = run("test.checkpoint", json!({ "steps": 3, "stepMs": 1 }));
    for step in 1..=3 {
        assert_eq!(
            p.recv(),
            WorkerMsg::Checkpoint {
                job: "j1".into(),
                data: json!({ "step": step })
            }
        );
        assert!(matches!(p.recv(), WorkerMsg::Progress(_)));
    }
    assert_eq!(p.result(), json!({ "steps": 3, "resumedFrom": 0 }));
    let mut s = start("j1", "test.checkpoint", json!({ "steps": 3, "crashAt": 1 }));
    s.job.checkpoint = Some(json!({ "step": 2 }));
    s.job.attempt = 2;
    let mut p = run_start(s);
    assert!(matches!(p.recv(), WorkerMsg::Checkpoint { .. }));
    assert!(matches!(p.recv(), WorkerMsg::Progress(_)));
    assert_eq!(p.result(), json!({ "steps": 3, "resumedFrom": 2 }));

    assert_eq!(
        run("test.oom", json!({ "mib": 8 })).result(),
        json!({ "mib": 8 })
    );

    let mut p = run(
        "test.call",
        json!({ "method": "test.echo", "params": { "x": 1 } }),
    );
    let WorkerMsg::Call { id, method, params } = p.recv() else {
        panic!()
    };
    assert_eq!((method.as_str(), params), ("test.echo", json!({ "x": 1 })));
    p.send(&HostMsg::Reply {
        id,
        value: Some(json!("back")),
        error: None,
    });
    assert_eq!(p.result(), json!("back"));

    let mut s = start("j1", "test.diag", json!({}));
    s.diagnostics = true;
    let mut p = run_start(s);
    assert!(matches!(p.recv(), WorkerMsg::Diag { kind, .. } if kind == "test.diag"));
    assert_eq!(p.result(), json!({ "sent": true }));

    let mut p = run("test.exitAfterResult", json!({}));
    assert_eq!(p.result(), json!({ "exit": 0 }));
    assert_eq!(p.wait().code(), Some(0));

    let mut s = start("j1", "test.helper", json!({ "helper": "ok" }));
    s.helpers = vec![HelperGrant {
        id: "test".into(),
        kind: HelperKind::Codec,
        files: vec![],
    }];
    assert_eq!(run_start(s).result(), json!({ "helpers": ["test"] }));
}

#[test]
fn bad_hello_and_plugin_minor() {
    let mut p = spawn(&["testbad"]);
    let h = p.hello();
    assert_ne!(h.envelope.as_deref(), Some(envelope_id()));
    assert_eq!(h.kinds, [KindVersion::new("test.badHello", 1)]);

    let mut p = spawn(&["testplugin"]);
    p.lenient = true;
    let h = p.hello();
    assert_eq!((h.minor, h.envelope.as_deref()), (1, None));
    p.send(&HostMsg::Start(start(
        "j1",
        "test.pluginMinor",
        json!({ "key": "A" }),
    )));
    let Decoded::Unknown { t, id: Some(id) } = p.recv_any() else {
        panic!("an unknown type with an id")
    };
    assert_eq!(t, "futureThing");
    let err = JobError::new("UNKNOWN_MESSAGE", "unknown message type");
    p.send(&HostMsg::Reply {
        id,
        value: None,
        error: Some(err),
    });
    let v = p.result();
    assert_eq!(v["reply"]["error"]["code"], "UNKNOWN_MESSAGE");
    assert_eq!(
        (v["key"].as_str(), v["served"].as_u64()),
        (Some("A"), Some(1))
    );
}

fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }
    #[cfg(windows)]
    {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains(&pid.to_string())
    }
}

fn kill_pid(pid: u32) {
    #[cfg(unix)]
    // SAFETY: kills a process this test started.
    unsafe {
        libc::kill(pid as i32, libc::SIGKILL);
    }
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/F", "/PID", &pid.to_string()])
        .output();
}

fn read_pids(path: &Path) -> SpawnPidsView {
    let until = Instant::now() + WAIT;
    loop {
        let raw = std::fs::read(path).unwrap_or_default();
        let text: Vec<u8> = raw.into_iter().take_while(|&b| b != 0).collect();
        if let Ok(p) = serde_json::from_slice::<Value>(&text) {
            return SpawnPidsView {
                grandchild: p["grandchild"].as_u64().unwrap() as u32,
                worker: p["worker"].as_u64().unwrap() as u32,
            };
        }
        assert!(Instant::now() < until, "no pids in {}", path.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct SpawnPidsView {
    worker: u32,
    grandchild: u32,
}

#[test]
fn spawn_writes_pids_of_a_live_grandchild() {
    let dir = tempfile::tempdir().unwrap();
    let out: PathBuf = dir.path().join("pids.json");
    std::fs::write(&out, vec![0; 256]).unwrap();
    let mut s = start(
        "j1",
        "test.spawn",
        json!({ "detach": "none", "out": "pids.json", "grandchildMs": 5000 }),
    );
    s.dirs = dirs(dir.path());
    s.files = vec![FileGrant {
        name: "pids.json".into(),
        path: out.clone(),
        mode: FileMode::Write,
        bytes: Some(256),
    }];
    let mut p = run_start(s);
    let pids = read_pids(&out);
    assert_eq!(pids.worker, p.child.id());
    assert!(alive(pids.grandchild), "the grandchild sleeps");
    p.kill();
    kill_pid(pids.grandchild);
}

fn attach(job: &Path, files: Vec<FileGrant>) -> HostMsg {
    HostMsg::Attach(Attach {
        job: "j1".into(),
        files,
        dirs: dirs(job),
    })
}

#[test]
fn helper_ok_attaches_and_exits_on_shutdown() {
    let mut p = spawn(&["--helper", "test:ok"]);
    let h = p.hello();
    assert_eq!(
        (h.role, h.kind, h.id.as_deref()),
        (Role::Helper, Some(HelperKind::Codec), Some("test"))
    );
    assert_eq!(h.envelope.as_deref(), Some(envelope_id()));
    p.send(&attach(Path::new("/nonexistent-job"), vec![]));
    std::thread::sleep(Duration::from_millis(100));
    assert!(p.child.try_wait().unwrap().is_none(), "waits for shutdown");
    p.send(&HostMsg::Shutdown {});
    assert_eq!(p.wait().code(), Some(0));

    let mut p = spawn(&["--helper", "test:call"]);
    p.hello();
    p.send(&attach(Path::new("/nonexistent-job"), vec![]));
    let WorkerMsg::Call { id, method, .. } = p.recv() else {
        panic!()
    };
    assert_eq!(method, "test.echo");
    p.send(&HostMsg::Reply {
        id,
        value: Some(json!({})),
        error: None,
    });
    p.send(&HostMsg::Shutdown {});
    assert_eq!(p.wait().code(), Some(0));
}

#[test]
fn helper_exit_early_exits_nonzero() {
    let mut p = spawn(&["--helper", "test:exitEarly"]);
    p.hello();
    p.send(&attach(Path::new("/nonexistent-job"), vec![]));
    assert_eq!(p.wait().code(), Some(3));
}

#[test]
fn version_prints_the_envelope_id() {
    let out = Command::new(BIN).arg("--version").output().unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), envelope_id());
}

#[test]
fn unknown_family_exits_2() {
    for args in [
        &["nope"][..],
        &[],
        &["--helper", "nope:x"],
        &["--helper", "test:nope"],
        &["test", "--cold"],
    ] {
        let out = Command::new(BIN)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
        assert!(out.stdout.is_empty(), "{args:?}");
    }
}
