//! `SandboxSpawner` (plan 7C Task 9) on Windows: a `worker` request is placed in its family's
//! AppContainer through the security-capabilities attribute, on a private desktop, with the
//! container UI limits and one active process, and keeps its self-sandbox request (which
//! checks the token).
#![cfg(windows)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{
    JobLimits, PathGrant, PlatformCommand, Profile, Program, Qos, Role, SpawnRequest, Spawner,
};
use fin_jobs::FileMode;
use fin_sandbox::spawner::{Decision, SandboxSpawner};
use fin_sandbox::windows::capability::{self, capability_sid};
use fin_sandbox::windows::job::UI_LIMITS;
use fin_sandbox::windows::{appcontainer, sid};
use windows_sys::Win32::Security::SECURITY_CAPABILITIES;
use windows_sys::Win32::System::Threading::PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES;

fn g(p: &Path, mode: FileMode) -> PathGrant {
    PathGrant {
        path: p.to_path_buf(),
        mode,
    }
}

#[test]
fn a_worker_is_placed_in_its_family_container() {
    let d = tempfile::tempdir().unwrap();
    let sd = d.path().to_path_buf();
    let job = sd.join("jobs").join("j1");
    for p in ["in", "out", "tmp"] {
        std::fs::create_dir_all(job.join(p)).unwrap();
    }
    let req = SpawnRequest {
        role: Role::Worker,
        profile: Profile::Worker,
        family: "test".into(),
        program: Program::Path(PathBuf::from("fin-workers.exe")),
        args: vec![],
        env: vec![(OsString::from("TMPDIR"), job.join("tmp").into_os_string())],
        cwd: job.clone(),
        grants: vec![
            g(&job.join("tmp"), FileMode::ReadWrite),
            g(&job.join("in"), FileMode::Read),
            g(&job.join("out"), FileMode::Write),
        ],
        qos: Qos::Normal,
        label: "t".into(),
        limits: Default::default(),
    };
    let s = SandboxSpawner::with(
        Some(sd.clone()),
        Some(sd.join("home")),
        Decision::Apply,
        Decision::Pass,
    );
    let mut c = PlatformCommand {
        program: "fin-workers.exe".into(),
        args: vec![],
        env: req.env.clone(),
        cwd: req.cwd.clone(),
        job_limits: JobLimits::default(),
        self_sandbox: None,
        attributes: Vec::new(),
        token: None,
        desktop: None,
        dacl: None,
    };
    s.prepare(&req, &mut c).unwrap();
    let container = capability::worker_container("test").unwrap();
    assert_eq!(c.job_limits.active_processes, Some(1));
    assert_eq!(c.job_limits.ui.map(|u| u.0), Some(UI_LIMITS));
    assert_eq!(
        c.self_sandbox.as_ref().map(|s| s.profile.as_str()),
        Some("worker")
    );
    let desk = c
        .desktop
        .as_ref()
        .expect("a private desktop")
        .to_string_lossy()
        .into_owned();
    assert!(desk.contains("\\fin-"), "{desk}");
    let user = fin_sandbox::windows::sid::current_user().unwrap().string();
    let own = capability_sid(&capability::job("j1").unwrap());
    assert_eq!(
        c.dacl.as_deref(),
        Some(fin_jobhost::process_dacl::sddl(&user, &own).as_str())
    );
    assert_eq!(c.attributes.len(), 1);
    let a = &c.attributes[0];
    assert_eq!(
        a.attribute,
        PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize
    );
    // SAFETY: the spawner's buffer starts with a SECURITY_CAPABILITIES whose pointers aim
    // inside it, and lives while `c` does.
    let (ac, caps) = unsafe {
        let sc = &*a.value.as_ptr().cast::<SECURITY_CAPABILITIES>();
        let caps: Vec<String> = (0..sc.CapabilityCount as usize)
            .map(|i| sid::string_of((*sc.Capabilities.add(i)).Sid).unwrap())
            .collect();
        (sid::string_of(sc.AppContainerSid).unwrap(), caps)
    };
    assert_eq!(ac, appcontainer::sid(&container).unwrap().string());
    // One capability, its job's; no network, no toolchain.
    assert_eq!(caps, [capability_sid(&capability::job("j1").unwrap())]);
    // Its TEMP is its job's own (review: a family's container is shared across projects).
    let temp = job
        .join("tmp")
        .join("appdata")
        .join("Packages")
        .join(&container)
        .join("AC")
        .join("Temp");
    let env = |k: &str| {
        c.env
            .iter()
            .find(|(n, _)| n.to_string_lossy().eq_ignore_ascii_case(k))
            .map(|(_, v)| PathBuf::from(v))
    };
    assert_eq!(env("TEMP"), Some(temp.clone()));
    assert_eq!(env("TMPDIR"), Some(temp.clone()));
    assert!(temp.is_dir());
    drop(c);
    appcontainer::delete(&container).unwrap();
}
