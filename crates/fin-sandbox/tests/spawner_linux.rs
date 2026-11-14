//! `SandboxSpawner` (plan 7C Task 7) on Linux: what `prepare` makes of a request, and the
//! `plugin` profile's child-side steps applied for real. The steps run in a forked child as
//! the job host runs them (`pre_exec`, after the host's own), here around `fin-sandbox-probe`,
//! under an allocator that fails the child if the steps allocate. A denial passes only on an
//! observed `EACCES` after an unconfined control succeeded.
#![cfg(target_os = "linux")]

mod linux_support;

use std::alloc::{GlobalAlloc, Layout, System};
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use fin_jobhost::spawner::{
    JobLimits, PathGrant, PlatformCommand, Profile, ProfileState, Program, Qos, Role, SpawnRequest,
    Spawner,
};
use fin_jobs::FileMode;
use fin_sandbox::spawner::{Decision, SandboxSpawner};
use fin_sandbox::{WorkerInput, WorkerScope};
use linux_support::{Outcome, PROBE, landlock_or_skip, listener, write};

/// Fails the process (exit 99, from a raw write) if anything allocates while it is armed.
struct Guard;
static ARMED: AtomicBool = AtomicBool::new(false);

// SAFETY: forwards to the system allocator; only adds a check.
unsafe impl GlobalAlloc for Guard {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.load(Ordering::SeqCst) {
            // SAFETY: raw calls; the child ends here.
            unsafe {
                libc::write(2, b"allocated in pre_exec\n".as_ptr().cast(), 22);
                libc::_exit(99);
            }
        }
        // SAFETY: as `System`.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: as `System`.
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Guard = Guard;

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
            limits: Default::default(),
        }
    }

    fn plugin_req(&self) -> SpawnRequest {
        let mut system = fin_sandbox::data::toolchain_base_resolved(fin_sandbox::Os::Linux);
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
        pre_exec: Vec::new(),
    }
}

#[test]
fn a_worker_gets_its_self_sandbox_from_the_grants() {
    let t = Tree::new();
    let fonts = t.sd.join("fonts");
    std::fs::create_dir_all(&fonts).unwrap();
    let source = t.sd.parent().unwrap().join("clip.mp4");
    let req = t.req(
        Profile::Worker,
        "media",
        vec![
            g(&t.job.join("in/x.json"), FileMode::Read),
            g(&t.job.join("out"), FileMode::Write),
            g(&fonts, FileMode::Read),
            g(&source, FileMode::Read),
        ],
    );
    let s = t.spawner(Decision::Apply, Decision::Pass);
    let mut c = cmd(&req);
    s.prepare(&req, &mut c).unwrap();
    let ss = c.self_sandbox.expect("a self-sandbox request");
    assert_eq!(ss.profile, "worker");
    let input: WorkerInput = serde_json::from_value(ss.args).unwrap();
    assert_eq!(input.tmp, t.job.join("tmp"));
    assert_eq!(input.caches, [fonts]);
    assert_eq!(
        input.scope,
        WorkerScope::Cold {
            job_dir: t.job.clone(),
            sources: vec![source],
        }
    );
    assert!(!input.devices.is_empty(), "media gets the GPU rows");
    assert_eq!(c.job_limits.active_processes, Some(1));
    assert!(c.pre_exec.is_empty());
    assert_eq!(s.profiles().worker, ProfileState::Enforced);
    assert_eq!(s.profiles().plugin, ProfileState::Disabled);
    assert!(!s.sandboxed());
}

#[test]
fn a_profile_that_passes_changes_nothing_and_a_refused_one_fails() {
    let t = Tree::new();
    let req = t.req(Profile::Worker, "test", vec![]);
    let mut c = cmd(&req);
    t.spawner(Decision::Pass, Decision::Pass)
        .prepare(&req, &mut c)
        .unwrap();
    assert!(c.self_sandbox.is_none() && c.job_limits == JobLimits::default());
    let e = t
        .spawner(Decision::Refuse("kernel ≥ 6.2".into()), Decision::Pass)
        .prepare(&req, &mut cmd(&req))
        .unwrap_err();
    assert!(
        e.0.starts_with("SANDBOX_UNAVAILABLE") && e.0.contains("kernel ≥ 6.2"),
        "{e}"
    );
    // An applied profile without server data, or with a grant it cannot place, refuses.
    let no_sd = SandboxSpawner::with(None, None, Decision::Apply, Decision::Apply);
    assert!(no_sd.prepare(&req, &mut cmd(&req)).is_err());
    let bad = t.req(
        Profile::Worker,
        "test",
        vec![g(&t.sd.join("auth/local.key"), FileMode::Read)],
    );
    let e = t
        .spawner(Decision::Apply, Decision::Pass)
        .prepare(&bad, &mut cmd(&bad))
        .unwrap_err();
    assert!(e.0.contains("not a cache"), "{e}");
    // Too many sources for one environment string: refused clearly, not `E2BIG` at exec.
    let many: Vec<PathGrant> = (0..6000)
        .map(|i| {
            g(
                &t.sd
                    .parent()
                    .unwrap()
                    .join(format!("src/a-long-name-{i:05}.mp4")),
                FileMode::Read,
            )
        })
        .collect();
    let big = t.req(Profile::Worker, "test", many);
    let e = t
        .spawner(Decision::Apply, Decision::Pass)
        .prepare(&big, &mut cmd(&big))
        .unwrap_err();
    assert!(e.0.contains("bytes, over"), "{e}");
    // Nothing is ready on any OS yet (Task 16 fills `READY`): the default spawner passes both.
    let default = SandboxSpawner::new(Some(t.sd.clone()));
    if std::env::var_os(fin_sandbox::status::TEST_READY_ENV).is_none() {
        assert_eq!(default.profiles(), fin_jobhost::ProfileCaps::DISABLED);
    }
}

/// Runs the probe's `op` with `steps` as the host would run them in the child: armed
/// allocator, the steps, disarmed.
fn run_steps(c: PlatformCommand, op: &[&str]) -> Outcome {
    let steps = c.pre_exec;
    let mut cmd = Command::new(PROBE);
    cmd.arg("--none").args(op).env_clear();
    // SAFETY: the closure runs in the child; the steps promise async-signal-safety, which the
    // armed allocator checks for allocation.
    unsafe {
        cmd.pre_exec(move || {
            ARMED.store(true, Ordering::SeqCst);
            for s in &steps {
                s()?;
            }
            ARMED.store(false, Ordering::SeqCst);
            Ok(())
        });
    }
    let out = cmd.output().unwrap();
    assert_ne!(out.status.code(), Some(99), "the steps allocated: {out:?}");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("probe output: {out:?}"));
    Outcome::Ran(v["errno"].as_i64().map(|e| e as i32))
}

fn control(op: &[&str]) {
    let o: Vec<String> = op.iter().map(|x| x.to_string()).collect();
    assert_eq!(
        linux_support::probe(&["--none".into()], &o),
        Outcome::Ran(None),
        "control {op:?}"
    );
}

#[test]
fn a_plugin_is_confined_by_its_pre_exec_steps() {
    let Some(abi) = landlock_or_skip("a_plugin_is_confined_by_its_pre_exec_steps") else {
        return;
    };
    let t = Tree::new();
    let req = t.plugin_req();
    let s = t.spawner(Decision::Pass, Decision::Apply);
    let prepared = || {
        let mut c = cmd(&req);
        s.prepare(&req, &mut c).unwrap();
        assert!(c.self_sandbox.is_none(), "plugins are confined before exec");
        assert_eq!(c.pre_exec.len(), 1);
        // Eight processes on the leaf where limits are enforced (sp8 §3.3).
        assert_eq!(c.job_limits.active_processes, Some(8));
        c
    };
    let key = t.sd.join("auth/local.key");
    let s_ = |p: &Path| p.to_string_lossy().into_owned();
    // Allowed: its call's `in/`, its package, a write in `out/`.
    for op in [
        vec!["read".to_string(), s_(&t.job.join("in/x.json"))],
        vec![
            "read".into(),
            s_(&t.sd.join("plugins/store/acme.tool@1.0.0/main")),
        ],
        vec!["create".into(), s_(&t.job.join("out/r.txt"))],
    ] {
        let op: Vec<&str> = op.iter().map(String::as_str).collect();
        assert_eq!(run_steps(prepared(), &op), Outcome::Ran(None), "{op:?}");
    }
    // Denied after a control: server data, a write to its package (the store is read-only)
    // and, at ABI ≥ 4, any TCP connection.
    let pkg_file = s_(&t.sd.join("plugins/store/acme.tool@1.0.0/new"));
    for op in [
        vec!["read", &s_(&key)][..].to_vec(),
        vec!["create", &pkg_file],
    ] {
        control(&op);
        let _ = std::fs::remove_file(&pkg_file);
        assert_eq!(
            run_steps(prepared(), &op),
            Outcome::Ran(Some(libc::EACCES)),
            "{op:?}"
        );
    }
    if abi.0 >= 4 {
        let (_l, port) = listener();
        let port = port.to_string();
        control(&["connect-tcp", &port]);
        let got = run_steps(prepared(), &["connect-tcp", &port]);
        assert_eq!(got, Outcome::Ran(Some(libc::EACCES)));
    }
}

#[test]
fn the_plugin_steps_limit_open_files() {
    let Some(_) = landlock_or_skip("the_plugin_steps_limit_open_files") else {
        return;
    };
    let t = Tree::new();
    let req = t.plugin_req();
    let mut c = cmd(&req);
    t.spawner(Decision::Pass, Decision::Apply)
        .prepare(&req, &mut c)
        .unwrap();
    // `sh -c 'ulimit -n'` is not in the plugin's grants; the probe's own limit is read by
    // `/proc/self/limits`, which a plugin may not read either: check the step's effect from
    // inside the steps' child instead (getrlimit after the steps).
    let steps = c.pre_exec;
    let mut cmd = Command::new(PROBE);
    cmd.args(["--none", "read", "/dev/null"]).env_clear();
    let (r, w) = std::io::pipe().unwrap();
    // SAFETY: raw calls in the child.
    unsafe {
        use std::os::fd::AsRawFd;
        let w = w.as_raw_fd();
        cmd.pre_exec(move || {
            for s in &steps {
                s()?;
            }
            let mut l: libc::rlimit = std::mem::zeroed();
            libc::getrlimit(libc::RLIMIT_NOFILE, &mut l);
            let n = l.rlim_cur as u64;
            libc::write(w, (&raw const n).cast(), 8);
            Ok(())
        });
    }
    let _ = cmd.output().unwrap();
    drop(cmd);
    let mut b = [0u8; 8];
    std::io::Read::read_exact(&mut &r, &mut b).unwrap();
    assert_eq!(u64::from_ne_bytes(b), 1024);
}
