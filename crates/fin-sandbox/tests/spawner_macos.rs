//! `SandboxSpawner` on macOS (plan 7C Task 8): a worker gets its self-sandbox request (applied
//! by `fin-workers` with Seatbelt, `fin-workers`' `tests/self_sandbox_macos.rs`), and a plugin's
//! program is rewritten to `sandbox-exec -p <profile> -D… -- <program>`, which this suite runs
//! for real around `fin-sandbox-probe`. A denial passes only on an observed `EPERM` after an
//! unconfined control succeeded.
#![cfg(target_os = "macos")]

mod macos_support;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use fin_jobhost::spawner::{
    JobLimits, PathGrant, PlatformCommand, Profile, Program, Qos, Role, SpawnRequest, Spawner,
};
use fin_jobs::FileMode;
use fin_sandbox::spawner::{Decision, SandboxSpawner};
use fin_sandbox::{WorkerInput, WorkerScope};
use macos_support::{Outcome, PROBE, listener, probe, s, write};

/// Server data (canonical) with a key, a jobs root and a plugin store; a job directory.
struct Tree {
    _d: tempfile::TempDir,
    sd: PathBuf,
    job: PathBuf,
}

impl Tree {
    fn new() -> Tree {
        let _d = tempfile::tempdir().unwrap();
        let sd = std::fs::canonicalize(_d.path()).unwrap();
        let job = sd.join("jobs/j1");
        write(&sd.join("auth/local.key"), "key");
        write(&sd.join("plugins/store/acme.tool@1.0.0/main"), "m");
        write(&job.join("in/x.json"), "{}");
        for d in ["out", "tmp"] {
            std::fs::create_dir_all(job.join(d)).unwrap();
        }
        Tree { _d, sd, job }
    }

    fn req(&self, profile: Profile, family: &str, grants: Vec<PathGrant>) -> SpawnRequest {
        let tmp = self.job.join("tmp");
        let mut all = vec![g(&tmp, FileMode::ReadWrite)];
        all.extend(grants);
        SpawnRequest {
            role: Role::Worker,
            profile,
            family: family.into(),
            program: Program::Path(PROBE.into()),
            args: vec![],
            env: vec![(OsString::from("TMPDIR"), tmp.into_os_string())],
            cwd: self.job.clone(),
            grants: all,
            qos: Qos::Normal,
            label: "t".into(),
        }
    }

    fn plugin_req(&self) -> SpawnRequest {
        let mut system = fin_sandbox::data::toolchain_base_resolved(fin_sandbox::Os::Macos);
        system.push(std::fs::canonicalize(PROBE).unwrap());
        let mut grants = vec![
            g(
                &self.sd.join("plugins/store/acme.tool@1.0.0"),
                FileMode::Read,
            ),
            g(&self.job.join("in"), FileMode::Read),
            g(&self.job.join("out"), FileMode::Write),
        ];
        grants.extend(system.iter().map(|p| g(p, FileMode::Read)));
        self.req(Profile::Plugin, "acme.tool", grants)
    }

    fn spawner(&self, worker: Decision, plugin: Decision) -> SandboxSpawner {
        SandboxSpawner::with(
            Some(self.sd.clone()),
            Some(self.sd.join("home")),
            worker,
            plugin,
        )
    }
}

fn g(p: &Path, mode: FileMode) -> PathGrant {
    PathGrant {
        path: p.to_path_buf(),
        mode,
    }
}

fn cmd(req: &SpawnRequest) -> PlatformCommand {
    PlatformCommand {
        program: PROBE.into(),
        args: req.args.clone(),
        env: req.env.clone(),
        cwd: req.cwd.clone(),
        job_limits: JobLimits::default(),
        self_sandbox: None,
        seatbelt_mark: None,
    }
}

#[test]
fn a_worker_gets_its_self_sandbox_from_the_grants() {
    let t = Tree::new();
    let source = t.sd.parent().unwrap().join("clip.mp4");
    let req = t.req(
        Profile::Worker,
        "media",
        vec![
            g(&t.job.join("in/x.json"), FileMode::Read),
            g(&t.job.join("out"), FileMode::Write),
            g(&source, FileMode::Read),
        ],
    );
    let s = t.spawner(Decision::Apply, Decision::Pass);
    let mut c = cmd(&req);
    s.prepare(&req, &mut c).unwrap();
    let ss = c.self_sandbox.expect("a self-sandbox request");
    assert_eq!(ss.profile, "worker");
    let input: WorkerInput = serde_json::from_value(ss.args).unwrap();
    assert_eq!(
        input.scope,
        WorkerScope::Cold {
            job_dir: t.job.clone(),
            sources: vec![source],
        }
    );
    // No device paths on macOS: Metal is reached through IOKit (the profile's own rules).
    assert!(input.devices.is_empty());
    assert_eq!(c.job_limits.active_processes, Some(1));
    assert_eq!(
        c.program,
        PathBuf::from(PROBE),
        "a worker's program is its own"
    );
}

/// Runs the prepared command with the probe's `op` appended, as the host would start it.
fn run(c: &PlatformCommand, op: &[&str]) -> Outcome {
    let out = Command::new(&c.program)
        .args(&c.args)
        .args(["--none"])
        .args(op)
        .env_clear()
        .current_dir(&c.cwd)
        .output()
        .unwrap();
    let v: Option<serde_json::Value> = serde_json::from_slice(&out.stdout).ok();
    match v {
        Some(v) if out.status.success() => Outcome::Ran(v["errno"].as_i64().map(|e| e as i32)),
        _ => Outcome::Other {
            code: out.status.code(),
            stdout: format!("{out:?}"),
        },
    }
}

#[test]
fn a_plugin_runs_under_sandbox_exec() {
    let t = Tree::new();
    let req = t.plugin_req();
    let mut c = cmd(&req);
    t.spawner(Decision::Pass, Decision::Apply)
        .prepare(&req, &mut c)
        .unwrap();
    assert!(c.self_sandbox.is_none(), "plugins are confined before exec");
    assert_eq!(c.program, PathBuf::from("/usr/bin/sandbox-exec"));
    assert_eq!(c.args[0], "-p");
    let dashdash = c.args.iter().position(|a| a == "--").unwrap();
    assert_eq!(
        c.args[dashdash + 1],
        PROBE,
        "the plugin's own program follows"
    );
    // Eight processes on the leaf where limits are enforced (sp8 §3.3).
    assert_eq!(c.job_limits.active_processes, Some(8));
    // Allowed: its call's `in/`, its package, a write in `out/`.
    for op in [
        ["read", &s(&t.job.join("in/x.json"))],
        ["read", &s(&t.sd.join("plugins/store/acme.tool@1.0.0/main"))],
        ["create", &s(&t.job.join("out/r.txt"))],
    ] {
        assert_eq!(run(&c, &op), Outcome::Ran(None), "{op:?}");
    }
    // Denied after a control: server data, a write to its package (the store is read-only),
    // any TCP connection and the Apple Events service.
    let pkg_file = s(&t.sd.join("plugins/store/acme.tool@1.0.0/new"));
    let key = s(&t.sd.join("auth/local.key"));
    let (_l, port) = listener();
    let port = port.to_string();
    for op in [
        vec!["read", key.as_str()],
        vec!["create", pkg_file.as_str()],
        vec!["connect-tcp", port.as_str()],
        vec!["mach-lookup", "com.apple.coreservices.appleevents"],
    ] {
        let o: Vec<String> = op.iter().map(|x| x.to_string()).collect();
        assert_eq!(
            probe(&["--none".into()], &o),
            Outcome::Ran(None),
            "control {op:?}"
        );
        let _ = std::fs::remove_file(&pkg_file);
        let got = run(&c, &op);
        assert!(got.denied(), "{op:?}: {got:?}");
    }
}

/// Exists and is not a zombie.
fn alive(pid: i32) -> bool {
    // SAFETY: plain data the call fills; the size is the struct's.
    let mut info: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as libc::c_int;
    // SAFETY: a valid buffer of `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            (&raw mut info).cast(),
            size,
        )
    };
    n == size && info.pbsi_status != libc::SZOMB
}

/// Polls `f` for up to 10 s.
fn within(mut f: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while !f() {
        if Instant::now() > until {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}

#[test]
fn a_plugins_setsid_child_goes_with_its_group() {
    use fin_jobhost::{EventSink, GroupSpec, Host, HostOptions, KillReason};
    let t = Tree::new();
    let mut req = t.plugin_req();
    // A plugin that leaves a child in a session of its own behind (out of its process group,
    // which the kill domain kills), which says its pid in `out/g.pid`.
    let pid_file = t.job.join("out/g.pid");
    req.args = vec![
        "--none".into(),
        "setsid-child".into(),
        pid_file.clone().into(),
    ];
    let mut opts = HostOptions::new(std::env::temp_dir());
    opts.watchdog = false;
    opts.spawner = Arc::new(t.spawner(Decision::Pass, Decision::Apply));
    let (sink, _events) = EventSink::channel(1024);
    let host = Host::start(opts, sink).unwrap();
    let group = host.open_group(GroupSpec {
        label: "t".into(),
        memory_bytes: None,
        qos: Qos::Normal,
    });
    let (tx, rx) = mpsc::channel();
    host.spawn(
        group,
        req,
        Box::new(move |r| {
            let _ = tx.send(r.map_err(|e| e.to_string()));
        }),
    );
    rx.recv_timeout(Duration::from_secs(10))
        .unwrap()
        .expect("the plugin started");
    assert!(within(|| pid_file.exists()), "the child never started");
    let g: i32 = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    // The child writes its pid only once `setsid` succeeded. The host may already have
    // ended the group when the plugin exited (its close sweeps the mark too); the kill and
    // the close below end it otherwise.
    let (tx, rx) = mpsc::channel();
    host.kill_group(
        group,
        KillReason::Test,
        Box::new(move || {
            let _ = tx.send(());
        }),
    );
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
    host.close_group(group);
    let left = !within(|| !alive(g));
    if left {
        // SAFETY: the process this test started.
        unsafe { libc::kill(g, libc::SIGKILL) };
    }
    assert!(!left, "the setsid child {g} outlived its group's kill");
}
