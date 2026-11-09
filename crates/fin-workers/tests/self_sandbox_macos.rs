//! The `worker` profile applied by `fin-workers` itself on macOS (plan 7C Task 8): a
//! `FIN_SELF_SANDBOX` request is applied through `fin_sandbox::self_apply` (Seatbelt,
//! `sandbox_init`) before `hello`, and the real families run under it. Each denial is an
//! observed `EPERM` (a job error quoting the OS's "Operation not permitted"), never an absence
//! of output, and each has an unsandboxed control.
#![cfg(target_os = "macos")]
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::{Path, PathBuf};

use fin_jobs::sandbox::{ENV, SelfSandbox};
use fin_jobs::{EXIT_SELF_SANDBOX, FileGrant, FileMode, HostMsg, Start};
use fin_media::jobs::LookResult;
use fin_media::testing::{Rect, read_id_rgba};
use fin_sandbox::{WorkerInput, WorkerScope};
use media_support::{Fixture, gpu_or_skip, read_png, worker_env};
use serde_json::json;
use support::{Job, Proc, Run, no_calls, spawn_with};

/// Seatbelt is present on every supported Mac; the suite never skips.
fn seatbelt(_test: &str) -> bool {
    assert!(
        std::path::Path::new("/usr/bin/sandbox-exec").is_file(),
        "Seatbelt is missing"
    );
    true
}

/// Server data with a jobs root, canonical (the policy refuses anything else).
struct Server {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Server {
    fn new() -> Server {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("jobs")).unwrap();
        std::fs::create_dir_all(root.join("auth")).unwrap();
        std::fs::write(root.join("auth/local.key"), b"key").unwrap();
        Server { _dir: dir, root }
    }

    /// A job directory `<root>/jobs/<random>/` with `in/`, `out/` and `tmp/`.
    fn job(&self) -> Job {
        let dir = tempfile::Builder::new()
            .prefix("job-")
            .tempdir_in(self.root.join("jobs"))
            .unwrap();
        for d in ["in", "out", "tmp"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        Job { dir }
    }

    /// The worker profile's input for one cold job reading `sources`.
    fn input(&self, job: &Job, sources: Vec<PathBuf>) -> WorkerInput {
        WorkerInput {
            tmp: job.path("tmp"),
            caches: vec![],
            devices: vec![],
            server_data: self.root.clone(),
            scope: WorkerScope::Cold {
                job_dir: job.dir.path().to_path_buf(),
                sources,
            },
        }
    }
}

fn request(input: &WorkerInput) -> (&'static str, String) {
    let req = SelfSandbox {
        profile: "worker".into(),
        args: serde_json::to_value(input).unwrap(),
    };
    (ENV, serde_json::to_string(&req).unwrap())
}

/// `fin-workers <family>` under `input`, past its `hello` (which must name the profile).
fn sandboxed(family: &str, input: &WorkerInput, extra: &[(&'static str, String)]) -> Proc {
    let mut env = vec![request(input)];
    env.extend_from_slice(extra);
    let mut p = spawn_with(&[family], &env);
    let h = p.hello();
    assert_eq!(h.self_sandbox.as_deref(), Some("worker"));
    p
}

fn grant(name: &str, path: &Path, mode: FileMode) -> FileGrant {
    FileGrant {
        name: name.into(),
        path: path.to_path_buf(),
        mode,
        bytes: None,
    }
}

fn run_files(server: &Server, job: &Job, files: Vec<FileGrant>) -> Run {
    let mut p = sandboxed("test", &server.input(job, vec![]), &[]);
    let mut start: Start = job.start("test.files", json!({}), &[]);
    start.files = files;
    p.send(&HostMsg::Start(start));
    p.drive(no_calls)
}

fn assert_denied(run: &Run, what: &str) {
    let e = run.error();
    assert_eq!(e.code, "IO_ERROR", "{what}: {e:?}");
    assert!(
        e.message.contains("Operation not permitted"),
        "{what}: {e:?}"
    );
}

/// The same job without the sandbox (the control: the denial is the sandbox's).
fn run_files_unsandboxed(job: &Job, files: Vec<FileGrant>) -> Run {
    let mut p = support::spawn_with(&["test"], &[]);
    p.hello();
    let mut start: Start = job.start("test.files", json!({}), &[]);
    start.files = files;
    p.send(&HostMsg::Start(start));
    p.drive(no_calls)
}

#[test]
fn self_apply_fails_closed() {
    let server = Server::new();
    let job = server.job();
    let mut relative = server.input(&job, vec![]);
    relative.tmp = PathBuf::from("tmp");
    // A root that is a link cannot be applied (Seatbelt would grant its target).
    let link = job.path("tmp-link");
    std::os::unix::fs::symlink(job.path("tmp"), &link).unwrap();
    let mut linked = server.input(&job, vec![]);
    linked.tmp = link;
    let cases: Vec<(String, &str)> = vec![
        ("{not json".into(), "unreadable FIN_SELF_SANDBOX"),
        (
            json!({ "profile": "worker", "args": null }).to_string(),
            "unreadable worker input",
        ),
        (request(&relative).1, "not absolute"),
        (
            json!({ "profile": "plugin", "args": {} }).to_string(),
            "not applied by the process itself",
        ),
    ];
    // A link is refused, not followed to its target (Seatbelt matches resolved paths).
    let link_reason = "symbolic link";
    let cases = cases
        .into_iter()
        .chain([(request(&linked).1, link_reason)])
        .collect::<Vec<_>>();
    for (raw, want) in cases {
        let mut cmd = std::process::Command::new(support::BIN);
        let out = cmd
            .arg("test")
            .env_clear()
            .env("FIN_TEST_MARKER", "1")
            .env(ENV, &raw)
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(EXIT_SELF_SANDBOX), "{raw}");
        assert!(
            out.stdout.is_empty(),
            "nothing is sent before the sandbox: {raw}"
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains(want), "{raw}: {stderr}");
    }
}

#[test]
fn a_sandboxed_worker_reads_only_its_grants() {
    if !seatbelt("a_sandboxed_worker_reads_only_its_grants") {
        return;
    }
    let server = Server::new();
    let job = server.job();
    std::fs::write(job.path("in/a.bin"), b"hello").unwrap();
    let ok = run_files(
        &server,
        &job,
        vec![
            grant("in/a.bin", &job.path("in/a.bin"), FileMode::Read),
            grant("out/", &job.path("out"), FileMode::Write),
        ],
    );
    assert_eq!(ok.result()["files"][0]["bytes"], 5);
    assert!(job.path("out/attempt.txt").is_file());
    // The host grants a path the worker profile does not: the kernel refuses it.
    let key = server.root.join("auth/local.key");
    let other = server.job();
    std::fs::write(other.path("in/b.bin"), b"other").unwrap();
    for (what, path) in [
        ("auth/local.key", key),
        ("another job", other.path("in/b.bin")),
    ] {
        let run = run_files(&server, &job, vec![grant("x", &path, FileMode::Read)]);
        assert_denied(&run, what);
        let control = run_files_unsandboxed(&job, vec![grant("x", &path, FileMode::Read)]);
        assert!(
            control.result()["files"][0]["bytes"].as_u64() > Some(0),
            "{what}"
        );
    }
}

#[test]
fn a_sandboxed_worker_writes_only_out_and_tmp() {
    if !seatbelt("a_sandboxed_worker_writes_only_out_and_tmp") {
        return;
    }
    let server = Server::new();
    let job = server.job();
    std::fs::write(job.path("in/a.bin"), b"hello").unwrap();
    std::fs::write(job.path("tmp/t.bin"), vec![0; 8]).unwrap();
    std::fs::write(job.path("j.bin"), vec![0; 8]).unwrap();
    let ok = run_files(
        &server,
        &job,
        vec![grant("tmp/t.bin", &job.path("tmp/t.bin"), FileMode::Write)],
    );
    assert_eq!(ok.result()["files"][0]["bytes"], 8);
    let other = server.job();
    for (what, g) in [
        (
            "in/ is read-only",
            grant("in/a.bin", &job.path("in/a.bin"), FileMode::Write),
        ),
        (
            "the job directory",
            grant("j.bin", &job.path("j.bin"), FileMode::Write),
        ),
        (
            "another job's out/",
            grant("out/", &other.path("out"), FileMode::Write),
        ),
    ] {
        assert_denied(&run_files(&server, &job, vec![g.clone()]), what);
        assert!(!other.path("out/attempt.txt").exists());
        // The control: unsandboxed, the same write works.
        run_files_unsandboxed(&job, vec![g]).result();
    }
}

#[test]
fn a_sandboxed_worker_cannot_exec() {
    if !seatbelt("a_sandboxed_worker_cannot_exec") {
        return;
    }
    let server = Server::new();
    let job = server.job();
    std::fs::write(job.path("out/pids.json"), vec![0; 64]).unwrap();
    let mut p = sandboxed("test", &server.input(&job, vec![]), &[]);
    let start = job.start(
        "test.spawn",
        json!({ "detach": "setsid", "out": "out/pids.json", "grandchildMs": 5000 }),
        &["out/pids.json"],
    );
    p.send(&HostMsg::Start(start));
    let run = p.drive(no_calls);
    // `fin-workers --test-spawn-mid` is its own binary: still not executable.
    assert_denied(&run, "exec of fin-workers");
}

#[test]
fn media_look_runs_under_the_worker_profile() {
    if !seatbelt("media_look_runs_under_the_worker_profile")
        || !gpu_or_skip("media_look_runs_under_the_worker_profile")
    {
        return;
    }
    let fx = Fixture::new();
    let plain = fx.job(None);
    let server = Server::new();
    let job = server.job();
    for f in ["in/snapshot.json", "in/manifest.json"] {
        std::fs::copy(plain.path(f), job.path(f)).unwrap();
    }
    let sources = vec![
        std::fs::canonicalize(&fx.av1).unwrap(),
        std::fs::canonicalize(&fx.h264).unwrap(),
    ];
    let input = server.input(&job, sources);
    let mut p = sandboxed("media", &input, &worker_env());
    let times = [media_support::r("0"), media_support::r("1")];
    let start = job.start(
        "media.look",
        serde_json::to_value(fx.params(&times)).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    p.send(&HostMsg::Start(start));
    let run = p.drive(no_calls);
    let res: LookResult = serde_json::from_value(run.result().clone()).unwrap();
    // The sources were opened (granted per file) and decoded: each frame is the right one.
    assert!(res.skipped.is_empty(), "{:?}", res.skipped);
    assert_eq!(res.frames.len(), 2);
    for (frame, t) in res.frames.iter().zip(times) {
        let (size, rgba) = read_png(&job.path(&format!("out/{}", frame.file)));
        let whole = Rect {
            x: 0.0,
            y: 0.0,
            width: size.width as f64,
            height: size.height as f64,
        };
        assert_eq!(
            read_id_rgba(&rgba, size.width, size.height, whole),
            Some(fx.expected_frame(t)),
            "frame at {t}"
        );
    }
    // sp6 §15: the capabilities event names the sandbox in force.
    let mut p = sandboxed("media", &input, &worker_env());
    let mut start = job.start("media.capabilities", json!({}), &[]);
    start.diagnostics = true;
    p.send(&HostMsg::Start(start));
    let run = p.drive(no_calls);
    let caps = run.diags("media.capabilities");
    assert_eq!(caps.len(), 1, "one capabilities event");
    assert_eq!(caps[0]["sandbox"], "worker", "{}", caps[0]);
}

#[test]
fn library_probe_runs_under_the_worker_profile() {
    use fin_library::params::{FileLine, LibraryProbeParams};
    if !seatbelt("library_probe_runs_under_the_worker_profile") {
        return;
    }
    let lib = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(lib.path()).unwrap();
    let lines: Vec<FileLine> = (0..2)
        .map(|i| {
            let p = root.join(format!("p{i}.png"));
            fin_probe::fixtures::write_png(&p, 8, 4);
            FileLine {
                id: i + 1,
                rel: support::rel(&[&format!("p{i}.png")]),
                expect: support::expect_of(&p),
            }
        })
        .collect();
    let server = Server::new();
    let probe = |granted: Vec<PathBuf>| {
        let job = server.job();
        let text: Vec<serde_json::Value> = lines
            .iter()
            .map(|l| serde_json::to_value(l).unwrap())
            .collect();
        job.write_in("in/files.ndjson", &text);
        let params = LibraryProbeParams {
            library: "lib1".into(),
            root: root.to_str().unwrap().into(),
            hydrate: false,
            read_bps: None,
            volume: support::identity_of(&root),
            extensions: None,
            files: "in/files.ndjson".into(),
            items: vec!["1".into(), "2".into()],
        };
        let mut p = sandboxed("library", &server.input(&job, granted), &[]);
        p.send(&HostMsg::Start(job.start(
            "library.probe",
            serde_json::to_value(&params).unwrap(),
            &["in/files.ndjson"],
        )));
        p.drive(|_, _| Ok(json!({})))
    };
    // The library root granted: both files probe as present.
    assert_eq!(
        probe(vec![root.clone()]).result()["states"],
        json!({ "present": 2 })
    );
    // Not granted: opening the root is refused by the kernel, and the job says so.
    let run = probe(vec![]);
    let e = run.error();
    assert_eq!(e.code, "LIBRARY_OFFLINE", "{e:?}");
    assert_eq!(e.details["state"]["code"], "PERMISSION_DENIED", "{e:?}");
}

/// The stubbed statuses (plan 7C Task 3) on macOS: a disabled mechanism applies nothing and
/// says so in `hello` (`selfSandbox: "none"`); where the host lacks Seatbelt the worker exits
/// 78 before `hello`, naming the need.
#[test]
fn landlock_disabled_runs_unconfined_and_unavailable_refuses() {
    use fin_sandbox::status::TEST_ENV;
    let server = Server::new();
    let job = server.job();
    let key = server.root.join("auth/local.key");
    let env = [
        request(&server.input(&job, vec![])),
        (TEST_ENV, "disabled".into()),
    ];
    let mut p = spawn_with(&["test"], &env);
    assert_eq!(
        p.hello().self_sandbox.as_deref(),
        Some(fin_jobs::sandbox::NONE)
    );
    // Unconfined indeed: a path outside the profile opens.
    let mut start: Start = job.start("test.files", json!({}), &[]);
    start.files = vec![grant("x", &key, FileMode::Read)];
    p.send(&HostMsg::Start(start));
    assert_eq!(p.drive(no_calls).result()["files"][0]["bytes"], 3);

    let out = std::process::Command::new(support::BIN)
        .arg("test")
        .env_clear()
        .env("FIN_TEST_MARKER", "1")
        .env(ENV, request(&server.input(&job, vec![])).1)
        .env(TEST_ENV, "unavailable")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(EXIT_SELF_SANDBOX));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Seatbelt"), "{stderr}");
}
