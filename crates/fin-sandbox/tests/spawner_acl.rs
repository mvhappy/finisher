//! `SandboxSpawner::acl_grants` (plan 7C Decision 18, the Windows side of Task 12): the ACL
//! broker's table for a job host request, from the same mapping `prepare` uses, on every OS
//! (the table is pure; only Windows applies it), and `per_job_grants` where the broker grants
//! each visiting job.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{PathGrant, Profile, Program, Qos, Role, SpawnRequest, Spawner};
use fin_jobs::FileMode;
use fin_sandbox::spawner::{Decision, SandboxSpawner, grants};
use fin_sandbox::windows::job_grants;

fn root() -> PathBuf {
    PathBuf::from(if cfg!(windows) { r"C:\r" } else { "/r" })
}

fn g(p: &Path, mode: FileMode) -> PathGrant {
    PathGrant {
        path: p.to_path_buf(),
        mode,
    }
}

fn cold(profile: Profile) -> (SpawnRequest, PathBuf) {
    let sd = root().join("data");
    let job = sd.join("jobs").join("j1");
    let mut grants = vec![
        g(&job.join("tmp"), FileMode::ReadWrite),
        g(&job.join("in").join("a.bin"), FileMode::Read),
        g(&job.join("out"), FileMode::Write),
    ];
    if profile == Profile::Plugin {
        grants.push(g(
            &sd.join("plugins").join("store").join("acme-1"),
            FileMode::Read,
        ));
    } else {
        grants.push(g(&root().join("p").join("sources"), FileMode::Read));
    }
    let req = SpawnRequest {
        role: Role::Worker,
        profile,
        family: "test".into(),
        program: Program::Path(root().join("bin").join("w.exe")),
        args: vec![],
        env: vec![(OsString::from("TMPDIR"), job.join("tmp").into_os_string())],
        cwd: job,
        grants,
        qos: Qos::Normal,
        label: "t".into(),
    };
    (req, sd)
}

fn spawner(worker: Decision, plugin: Decision) -> SandboxSpawner {
    let sd = root().join("data");
    SandboxSpawner::with(Some(sd), Some(root().join("home")), worker, plugin)
}

#[test]
fn an_applied_worker_profile_answers_the_jobs_table() {
    let (req, sd) = cold(Profile::Worker);
    let exe = root().join("bin").join("w.exe");
    let got = spawner(Decision::Apply, Decision::Pass)
        .acl_grants(&req, &exe)
        .unwrap();
    let input = grants::worker_input(&req, &sd).unwrap();
    assert_eq!(got, job_grants::worker(&input, "test", &exe).unwrap());
    assert!(!got.is_empty());
    // The job's own capability, never the jobs root or server data.
    for a in &got {
        assert_ne!(a.path, sd.join("jobs"));
        assert_ne!(a.path, sd);
    }
}

#[test]
fn an_applied_plugin_profile_answers_the_calls_table() {
    let (req, sd) = cold(Profile::Plugin);
    let exe = root().join("bin").join("w.exe");
    let got = spawner(Decision::Pass, Decision::Apply)
        .acl_grants(&req, &exe)
        .unwrap();
    let input = grants::plugin_input(&req, &sd, &root().join("home")).unwrap();
    assert_eq!(got, job_grants::plugin(&input, &exe).unwrap());
    assert!(got.iter().any(|a| a.path == input.store_dir));
}

#[test]
fn a_profile_that_passes_grants_nothing_and_a_refused_one_refuses() {
    let exe = root().join("bin").join("w.exe");
    let (w, _) = cold(Profile::Worker);
    let (p, _) = cold(Profile::Plugin);
    let pass = spawner(Decision::Pass, Decision::Pass);
    assert_eq!(pass.acl_grants(&w, &exe).unwrap(), vec![]);
    assert_eq!(pass.acl_grants(&p, &exe).unwrap(), vec![]);
    let refused = spawner(Decision::Refuse("x".into()), Decision::Refuse("y".into()));
    let e = refused.acl_grants(&w, &exe).unwrap_err().0;
    assert!(e.starts_with("SANDBOX_UNAVAILABLE"), "{e}");
    // A request `prepare` would refuse is refused alike (a write grant outside the job).
    let (mut bad, _) = cold(Profile::Worker);
    bad.grants.push(g(&root().join("p"), FileMode::Write));
    let e = spawner(Decision::Apply, Decision::Pass)
        .acl_grants(&bad, &exe)
        .unwrap_err()
        .0;
    assert!(e.starts_with("SANDBOX_UNAVAILABLE"), "{e}");
}

#[test]
fn per_job_grants_follow_the_applied_worker_profile_where_they_are_made() {
    let applied = spawner(Decision::Apply, Decision::Pass).per_job_grants();
    // Linux: Landlock at start; Windows: the broker per visiting job; macOS: not yet.
    assert_eq!(applied, cfg!(any(target_os = "linux", windows)));
    assert!(!spawner(Decision::Pass, Decision::Apply).per_job_grants());
}
