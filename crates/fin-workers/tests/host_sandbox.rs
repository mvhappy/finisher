//! The job host with `SandboxSpawner` (plan 7C Task 7) over the real `fin-workers` binary: a
//! worker spawned from a cold job's grants applies the `worker` profile itself (Landlock;
//! seccomp is deferred, plan Task 4, so the `setsid` + `fork` refusal of the plan's Step 1 is
//! not run) and says so in `hello`, reads what it was granted, and is refused what it was not.
#![cfg(target_os = "linux")]
#![allow(clippy::disallowed_methods)]

mod host_support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_jobhost::*;
use fin_jobs::{FileGrant, FileMode, HostMsg};
use fin_sandbox::linux::landlock::{Support, query};
use fin_sandbox::spawner::{Decision, SandboxSpawner};
use host_support::{TestHost, host_with};
use serde_json::json;

fn landlock(test: &str) -> bool {
    match query() {
        Support::Abi(_) => true,
        other => {
            assert_ne!(
                std::env::var("FIN_REQUIRE_LANDLOCK").as_deref(),
                Ok("1"),
                "{test}: FIN_REQUIRE_LANDLOCK=1 but Landlock answers {other:?}"
            );
            eprintln!("{test}: skipped, Landlock is not available ({other:?})");
            false
        }
    }
}

/// Server data (canonical) with a key and one job directory.
struct Server {
    _d: tempfile::TempDir,
    sd: PathBuf,
    job: PathBuf,
}

impl Server {
    fn new() -> Server {
        let _d = tempfile::tempdir().unwrap();
        let sd = std::fs::canonicalize(_d.path()).unwrap();
        let job = sd.join("jobs/j1");
        for d in ["in", "out", "tmp"] {
            std::fs::create_dir_all(job.join(d)).unwrap();
        }
        std::fs::create_dir_all(sd.join("auth")).unwrap();
        std::fs::write(sd.join("auth/local.key"), b"key").unwrap();
        std::fs::write(job.join("in/a.bin"), b"hello").unwrap();
        Server { _d, sd, job }
    }

    fn host(&self, worker: Decision) -> TestHost {
        let sd = self.sd.clone();
        host_with(move |o| {
            o.spawner = Arc::new(SandboxSpawner::with(Some(sd), None, worker, Decision::Pass));
        })
    }

    /// `fin-workers test` for the job, granted `tmp/`, `in/a.bin` and `out/`.
    fn req(&self) -> SpawnRequest {
        let tmp = self.job.join("tmp");
        let g = |p: PathBuf, mode| PathGrant { path: p, mode };
        SpawnRequest {
            role: Role::Worker,
            profile: Profile::Worker,
            family: "test".into(),
            program: Program::Bundled("fin-workers".into()),
            args: vec!["test".into()],
            env: vec![(OsString::from("TMPDIR"), tmp.clone().into_os_string())],
            cwd: self.job.clone(),
            grants: vec![
                g(tmp, FileMode::ReadWrite),
                g(self.job.join("in/a.bin"), FileMode::Read),
                g(self.job.join("out"), FileMode::Write),
            ],
            qos: Qos::Normal,
            label: "test".into(),
            limits: Default::default(),
        }
    }

    /// `test.files` over `files` in the job's directories.
    fn files(
        &self,
        h: &mut TestHost,
        p: ProcId,
        files: Vec<(&str, &Path, FileMode)>,
    ) -> serde_json::Value {
        let HostMsg::Start(mut start) = h.start_msg("j1", "test.files", json!({})) else {
            unreachable!()
        };
        start.dirs.job = self.job.clone();
        start.dirs.tmp = self.job.join("tmp");
        start.files = files
            .into_iter()
            .map(|(name, path, mode)| FileGrant {
                name: name.into(),
                path: path.to_path_buf(),
                mode,
                bytes: None,
            })
            .collect();
        h.host.send(p, &HostMsg::Start(start)).unwrap();
        loop {
            match h.message(p) {
                fin_jobs::WorkerMsg::Result { value, .. } => return value,
                fin_jobs::WorkerMsg::Error { error, .. } => {
                    return json!({ "error": { "code": error.code, "message": error.message } });
                }
                _ => {}
            }
        }
    }
}

#[test]
fn a_worker_applies_the_profile_from_its_grants() {
    if !landlock("a_worker_applies_the_profile_from_its_grants") {
        return;
    }
    let s = Server::new();
    let mut h = s.host(Decision::Apply);
    let caps = h.host.caps();
    assert_eq!(caps.sandbox, "fin-sandbox");
    assert_eq!(caps.profiles.worker, ProfileState::Enforced);
    assert!(!caps.sandboxed, "plugin is not applied");
    let g = h.group();
    let p = h.spawn(g, s.req()).expect("spawns");
    assert_eq!(h.hello(p).self_sandbox.as_deref(), Some("worker"));
    let ok = s.files(
        &mut h,
        p,
        vec![
            ("in/a.bin", &s.job.join("in/a.bin"), FileMode::Read),
            ("out/", &s.job.join("out"), FileMode::Write),
        ],
    );
    assert_eq!(ok["files"][0]["bytes"], 5, "{ok}");
}

#[test]
fn a_worker_is_refused_what_its_grants_do_not_hold() {
    if !landlock("a_worker_is_refused_what_its_grants_do_not_hold") {
        return;
    }
    let s = Server::new();
    // Control: under a spawner that passes, the same worker reads the key.
    let key = s.sd.join("auth/local.key");
    let read_key = |h: &mut TestHost| {
        let g = h.group();
        let p = h.spawn(g, s.req()).expect("spawns");
        let hello = h.hello(p);
        (hello, s.files(h, p, vec![("x", &key, FileMode::Read)]))
    };
    let (hello, plain) = read_key(&mut s.host(Decision::Pass));
    assert_eq!(hello.self_sandbox, None);
    assert_eq!(plain["files"][0]["bytes"], 3, "control: {plain}");
    let (_, denied) = read_key(&mut s.host(Decision::Apply));
    assert_eq!(denied["error"]["code"], "IO_ERROR", "{denied}");
    assert!(
        denied["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Permission denied"),
        "{denied}"
    );
}

#[test]
fn a_request_the_profile_cannot_place_fails_the_spawn() {
    let s = Server::new();
    let h = s.host(Decision::Apply);
    let mut req = s.req();
    req.grants.push(PathGrant {
        path: s.sd.join("auth"),
        mode: FileMode::Read,
    });
    let g = h.group();
    let e = h.spawn(g, req).expect_err("refused");
    assert_eq!(e.code(), "SANDBOX_UNAVAILABLE", "{e:?}");
    // A ready profile on a host without the mechanism refuses every spawn.
    let h = s.host(Decision::Refuse("kernel ≥ 6.2".into()));
    assert_eq!(h.host.caps().profiles.worker, ProfileState::Unavailable);
    let g = h.group();
    assert_eq!(
        h.spawn(g, s.req()).unwrap_err().code(),
        "SANDBOX_UNAVAILABLE"
    );
}
