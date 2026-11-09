//! The job host's Windows spawn path under `SandboxSpawner` (plan 7C Tasks 9 and 10, the
//! Windows side of Task 12) with a real worker (`fin-workers test`) and a real plugin
//! (`fin-workers testplugin`): the profiles forced ready by the test hook
//! (`FIN_TEST_SANDBOX_READY=worker,plugin`, `READY` is empty until Task 16), each request's ACL
//! table (`SandboxSpawner::acl_grants`) granted as `finisherd` grants it through the broker,
//! and the process started by the host in its AppContainer.
//!
//! Proven by configuration and allowed paths only (owner decision 2026-11-08, plan Decision
//! 27): the process's token holds exactly its container and capabilities (read back from its
//! pid); the spawner hands the host the container's leaf Job limits, a private desktop and the
//! process DACL (read back from the command after `prepare`); the worker's self-sandbox check
//! accepts its token, it reads and writes what it was granted, and what it was not granted is
//! computed with `AccessCheck` on a copy of its token, never opened.
//!
//! The tests share `fin-workers.exe`, whose DACL each grants and revokes: they run one at a
//! time (`SERIAL`). Each grant and container outlives the host (declared before it).
#![cfg(windows)]
// Tests drive the host and inspect job directories by path; none is a library reader.
#![allow(clippy::disallowed_methods)]

mod host_support;

use std::ffi::OsString;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::ptr::null_mut;
use std::sync::{Arc, Mutex};

use fin_jobhost::*;
use fin_jobs::{FileGrant, FileMode, HostMsg, WorkerMsg};
use fin_sandbox::SandboxSpawner;
use fin_sandbox::windows::capability::{self, TOOLCHAIN, capability_sid};
use fin_sandbox::windows::grants::{AclGrant, Scope, Trustee};
use fin_sandbox::windows::job::UI_LIMITS;
use fin_sandbox::windows::sid::{Sid, current_user, string_of, wide};
use fin_sandbox::windows::{acl, appcontainer};
use host_support::{BIN, TestHost, bin_dir, host_with};
use serde_json::{Value, json};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::Security::{
    AccessCheck, DACL_SECURITY_INFORMATION, DuplicateTokenEx, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, GetFileSecurityW, GetTokenInformation, MapGenericMask,
    OWNER_SECURITY_INFORMATION, PRIVILEGE_SET, PSID, SecurityImpersonation, TOKEN_DUPLICATE,
    TOKEN_GROUPS, TOKEN_IMPERSONATE, TOKEN_INFORMATION_CLASS, TOKEN_QUERY, TokenAppContainerSid,
    TokenCapabilities, TokenImpersonation, TokenIsAppContainer,
};
use windows_sys::Win32::System::Threading::{
    OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

const FILE_READ_DATA: u32 = 0x1;
const FILE_WRITE_DATA: u32 = 0x2;
const FILE_MAPPING: GENERIC_MAPPING = GENERIC_MAPPING {
    GenericRead: 0x0012_0089,
    GenericWrite: 0x0012_0116,
    GenericExecute: 0x0012_00A0,
    GenericAll: 0x001F_01FF,
};

static SERIAL: Mutex<()> = Mutex::new(());

/// The forced-ready hook, the same value in every test of this binary.
fn force_ready() {
    // SAFETY: every test of this binary sets the same value before it reads it.
    unsafe { std::env::set_var("FIN_TEST_SANDBOX_READY", "worker,plugin") };
}

/// Server data with `private_dir`'s protected owner-only DACL, a key, two jobs and a plugin.
/// Paths in Windows' own form (never `canonicalize`'s `\\?\` one, which `CreateProcessW`
/// refuses as a working directory and in which `/` is no separator).
struct Server {
    _d: tempfile::TempDir,
    sd: PathBuf,
}

impl Server {
    fn new() -> Server {
        let _d = tempfile::tempdir().unwrap();
        let sd = _d.path().to_path_buf();
        let user = current_user().unwrap().string();
        acl::set_sddl(
            &sd,
            &format!("D:P(A;OICI;FA;;;{user})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)"),
        )
        .unwrap();
        for d in [
            "auth",
            "jobs/j1/in",
            "jobs/j1/out",
            "jobs/j1/tmp",
            "jobs/j2/in",
            "jobs/c1/in",
            "jobs/c1/out",
            "jobs/c1/tmp",
            "plugins/store/acme-1",
        ] {
            std::fs::create_dir_all(sd.join(d.replace('/', "\\"))).unwrap();
        }
        let s = Server { _d, sd };
        std::fs::write(s.p("auth/local.key"), b"key").unwrap();
        std::fs::write(s.p("jobs/j1/in/a.bin"), b"hello").unwrap();
        std::fs::write(s.p("jobs/j2/in/other.bin"), b"other").unwrap();
        std::fs::write(s.p("plugins/store/acme-1/plugin.json"), b"{}").unwrap();
        s
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.sd.join(rel.replace('/', "\\"))
    }

    fn req(&self, profile: Profile, args: &str, dir: &str, extra: Vec<PathGrant>) -> SpawnRequest {
        let job = self.p(dir);
        let g = |p: PathBuf, mode| PathGrant { path: p, mode };
        let mut grants = vec![
            g(job.join("tmp"), FileMode::ReadWrite),
            g(job.join("out"), FileMode::Write),
        ];
        grants.extend(extra);
        SpawnRequest {
            role: Role::Worker,
            profile,
            family: "test".into(),
            program: match profile {
                Profile::Worker => Program::Bundled("fin-workers".into()),
                Profile::Plugin => Program::Path(PathBuf::from(BIN)),
            },
            args: vec![args.into()],
            env: vec![(OsString::from("TMPDIR"), job.join("tmp").into_os_string())],
            cwd: job,
            grants,
            qos: Qos::Normal,
            label: args.into(),
        }
    }
}

/// What `prepare` left for the host (read back after the spawner's own steps).
#[derive(Debug, Clone)]
struct Prepared {
    job_limits: JobLimits,
    desktop: Option<OsString>,
    dacl: Option<String>,
}

/// The real spawner, recording what each `prepare` set.
struct Recording {
    inner: SandboxSpawner,
    seen: Arc<Mutex<Vec<Prepared>>>,
}

impl Spawner for Recording {
    fn name(&self) -> &'static str {
        self.inner.name()
    }
    fn sandboxed(&self) -> bool {
        self.inner.sandboxed()
    }
    fn profiles(&self) -> ProfileCaps {
        self.inner.profiles()
    }
    fn per_job_grants(&self) -> bool {
        self.inner.per_job_grants()
    }
    fn prepare(&self, req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError> {
        self.inner.prepare(req, cmd)?;
        self.seen.lock().unwrap().push(Prepared {
            job_limits: cmd.job_limits,
            desktop: cmd.desktop.clone(),
            dacl: cmd.dacl.clone(),
        });
        Ok(())
    }
}

/// The request's table granted as the broker grants it; every ACE revoked, and every
/// container profile it named deleted, on drop.
struct Granted {
    made: Vec<(PathBuf, Sid)>,
    containers: Vec<String>,
}

impl Granted {
    fn new(table: &[AclGrant], also: &[String]) -> Granted {
        let mut g = Granted {
            made: vec![],
            containers: also.to_vec(),
        };
        for a in table {
            // The process's own roots exist (a wrong entry must not pass unseen); a kept one
            // may not yet, as the broker skips it.
            if !a.path.exists() {
                assert_eq!(a.scope, Scope::Static, "{} is missing", a.path.display());
                continue;
            }
            let sid = match &a.trustee {
                Trustee::Sid(s) => Sid::parse(s).unwrap(),
                Trustee::Container(c) => {
                    g.containers.push(c.clone());
                    appcontainer::sid(c).unwrap()
                }
            };
            acl::grant(&a.path, &sid, a.mask, true).unwrap();
            g.made.push((a.path.clone(), sid));
        }
        g
    }
}

impl Drop for Granted {
    fn drop(&mut self) {
        for (p, sid) in &self.made {
            let _ = acl::revoke(p, sid);
        }
        for c in &self.containers {
            let _ = appcontainer::delete(c);
        }
    }
}

/// The real spawner, with the forced-ready hook.
fn spawner(s: &Server) -> SandboxSpawner {
    force_ready();
    SandboxSpawner::new(Some(s.sd.clone()))
}

/// A host over `spawner`, recording each `prepare`.
fn host(spawner: &SandboxSpawner) -> (TestHost, Arc<Mutex<Vec<Prepared>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (inner, rec) = (spawner.clone(), seen.clone());
    let h = host_with(move |o| o.spawner = Arc::new(Recording { inner, seen: rec }));
    (h, seen)
}

fn owned(h: HANDLE) -> OwnedHandle {
    assert!(!h.is_null(), "{}", std::io::Error::last_os_error());
    // SAFETY: a fresh handle we own.
    unsafe { OwnedHandle::from_raw_handle(h as _) }
}

/// An impersonation copy of process `pid`'s token (its DACL admits this user).
fn token_of(pid: u32) -> OwnedHandle {
    // SAFETY: plain handle calls with valid out-pointers; each handle is owned once.
    unsafe {
        let p = owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid));
        let mut tok: HANDLE = null_mut();
        assert_ne!(
            OpenProcessToken(
                p.as_raw_handle() as HANDLE,
                TOKEN_DUPLICATE | TOKEN_QUERY,
                &mut tok
            ),
            0,
            "OpenProcessToken: {}",
            std::io::Error::last_os_error()
        );
        let tok = owned(tok);
        let mut dup: HANDLE = null_mut();
        assert_ne!(
            DuplicateTokenEx(
                tok.as_raw_handle() as HANDLE,
                TOKEN_QUERY | TOKEN_IMPERSONATE,
                std::ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut dup,
            ),
            0,
            "DuplicateTokenEx: {}",
            std::io::Error::last_os_error()
        );
        owned(dup)
    }
}

fn info(tok: &OwnedHandle, class: TOKEN_INFORMATION_CLASS) -> Vec<u64> {
    let h = tok.as_raw_handle() as HANDLE;
    let mut len = 0u32;
    // SAFETY: a size query.
    unsafe { GetTokenInformation(h, class, null_mut(), 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: a buffer of at least `len` bytes.
    let ok = unsafe { GetTokenInformation(h, class, buf.as_mut_ptr().cast(), len, &mut len) };
    assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
    buf
}

/// The token's container SID and capability SIDs (`None` when it is not an AppContainer).
fn container_of(tok: &OwnedHandle) -> Option<(String, Vec<String>)> {
    // SAFETY: each call wrote the structure named by its class into its buffer.
    unsafe {
        let is = info(tok, TokenIsAppContainer);
        if *is.as_ptr().cast::<u32>() == 0 {
            return None;
        }
        let ac = info(tok, TokenAppContainerSid);
        let container = string_of(*ac.as_ptr().cast::<PSID>()).unwrap();
        let caps = info(tok, TokenCapabilities);
        let g = caps.as_ptr().cast::<TOKEN_GROUPS>();
        let caps = std::slice::from_raw_parts((*g).Groups.as_ptr(), (*g).GroupCount as usize)
            .iter()
            .map(|a| string_of(a.Sid).unwrap())
            .collect();
        Some((container, caps))
    }
}

/// What `tok` is granted on `path` (`MAXIMUM_ALLOWED`, computed by `AccessCheck`; 0: none).
fn access(tok: &OwnedHandle, path: &Path) -> u32 {
    let si = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let p = wide(path);
    let mut len = 0u32;
    // SAFETY: a size query.
    unsafe { GetFileSecurityW(p.as_ptr(), si, null_mut(), 0, &mut len) };
    let mut sd = vec![0u64; (len as usize).div_ceil(8).max(1)];
    // SAFETY: a buffer of `len` bytes.
    let ok = unsafe { GetFileSecurityW(p.as_ptr(), si, sd.as_mut_ptr().cast(), len, &mut len) };
    assert_ne!(
        ok,
        0,
        "{}: {}",
        path.display(),
        std::io::Error::last_os_error()
    );
    let mut desired = 0x0200_0000u32;
    // SAFETY: valid pointers.
    unsafe { MapGenericMask(&mut desired, &FILE_MAPPING) };
    let mut privs = vec![0u64; 32];
    let mut privs_len = (privs.len() * 8) as u32;
    let (mut granted, mut status) = (0u32, 0i32);
    // SAFETY: the descriptor read above, our token, buffers of the sizes given.
    let ok = unsafe {
        AccessCheck(
            sd.as_mut_ptr().cast(),
            tok.as_raw_handle() as HANDLE,
            desired,
            &FILE_MAPPING,
            privs.as_mut_ptr().cast::<PRIVILEGE_SET>(),
            &mut privs_len,
            &mut granted,
            &mut status,
        )
    };
    assert_ne!(ok, 0, "AccessCheck: {}", std::io::Error::last_os_error());
    if status == 0 { 0 } else { granted }
}

/// Runs `kind` with `files` mapped on process `p`; its result (or `{error}`).
fn run(
    h: &mut TestHost,
    p: ProcId,
    dir: &Path,
    kind: &str,
    files: Vec<(&str, PathBuf, FileMode)>,
) -> Value {
    let HostMsg::Start(mut start) = h.start_msg("j1", kind, json!({})) else {
        unreachable!()
    };
    start.dirs.job = dir.to_path_buf();
    start.dirs.tmp = dir.join("tmp");
    start.files = files
        .into_iter()
        .map(|(name, path, mode)| FileGrant {
            name: name.into(),
            path,
            mode,
            bytes: None,
        })
        .collect();
    h.host.send(p, &HostMsg::Start(start)).unwrap();
    loop {
        match h.message(p) {
            WorkerMsg::Result { value, .. } => return value,
            WorkerMsg::Error { error, .. } => {
                return json!({ "error": { "code": error.code, "message": error.message } });
            }
            _ => {}
        }
    }
}

fn expect_prepared(p: &Prepared, own: &str, processes: u32) {
    assert_eq!(p.job_limits.active_processes, Some(processes), "{p:?}");
    assert_eq!(p.job_limits.ui.map(|u| u.0), Some(UI_LIMITS), "{p:?}");
    let desk = p
        .desktop
        .as_ref()
        .expect("a private desktop")
        .to_string_lossy()
        .into_owned();
    assert!(desk.contains("\\fin-"), "{desk}");
    let user = current_user().unwrap().string();
    assert_eq!(
        p.dacl.as_deref(),
        Some(fin_jobhost::process_dacl::sddl(&user, own).as_str())
    );
}

#[test]
fn a_real_worker_runs_in_its_family_container_with_its_jobs_grants() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Server::new();
    let spawner = spawner(&s);
    let job = s.p("jobs/j1");
    let a_bin = s.p("jobs/j1/in/a.bin");
    let req = s.req(
        Profile::Worker,
        "test",
        "jobs/j1",
        vec![PathGrant {
            path: a_bin.clone(),
            mode: FileMode::Read,
        }],
    );
    let exe = bin_dir().join("fin-workers.exe");
    let table = spawner.acl_grants(&req, &exe).unwrap();
    let family = capability::worker_container("test").unwrap();
    let _granted = Granted::new(&table, std::slice::from_ref(&family));
    let (mut h, seen) = host(&spawner);
    let caps = h.host.caps();
    assert_eq!(
        caps.profiles.worker,
        ProfileState::Enforced,
        "the forced-ready hook"
    );
    assert!(caps.per_job_grants);
    let g = h.group();
    let p = h.spawn(g, req).expect("spawns");
    let hello = h.hello(p);
    // The worker's own check of its token passed (it fails closed otherwise).
    assert_eq!(hello.self_sandbox.as_deref(), Some("worker"));
    // Its token: the family's container and its job's capability alone (no network, no
    // toolchain).
    let tok = token_of(hello.pid);
    let own = capability_sid(&capability::job("j1").unwrap());
    assert_eq!(
        container_of(&tok),
        Some((
            appcontainer::sid(&family).unwrap().string(),
            vec![own.clone()]
        ))
    );
    expect_prepared(&seen.lock().unwrap()[0], &own, 1);
    // What it was granted, it uses.
    let ok = run(
        &mut h,
        p,
        &job,
        "test.files",
        vec![
            ("in/a.bin", a_bin.clone(), FileMode::Read),
            ("out/", s.p("jobs/j1/out"), FileMode::Write),
        ],
    );
    assert_eq!(ok["files"][0]["bytes"], 5, "{ok}");
    for (path, want) in [
        (a_bin, FILE_READ_DATA),
        (s.p("jobs/j1/out"), FILE_WRITE_DATA),
    ] {
        assert_eq!(access(&tok, &path) & want, want, "{}", path.display());
    }
    // What it was not granted, it cannot reach: computed on its token, never attempted.
    for path in [
        s.sd.clone(),
        s.p("auth/local.key"),
        s.p("jobs"),
        s.p("jobs/j2/in/other.bin"),
        s.p("jobs/c1/in"),
    ] {
        assert_eq!(
            access(&tok, &path) & (FILE_READ_DATA | FILE_WRITE_DATA),
            0,
            "{}",
            path.display()
        );
    }
    // A cold worker exits after its job; the host goes before the grants do.
    drop(tok);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
    drop(h);
}

#[test]
fn a_real_plugin_runs_in_its_plugin_container_with_its_calls_grants() {
    let _serial = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    let s = Server::new();
    let spawner = spawner(&s);
    let store = s.p("plugins/store/acme-1");
    let req = s.req(
        Profile::Plugin,
        "testplugin",
        "jobs/c1",
        vec![PathGrant {
            path: store.clone(),
            mode: FileMode::Read,
        }],
    );
    let table = spawner.acl_grants(&req, Path::new(BIN)).unwrap();
    let container = capability::plugin_container(&store, None);
    let _granted = Granted::new(&table, std::slice::from_ref(&container));
    let (mut h, seen) = host(&spawner);
    assert_eq!(h.host.caps().profiles.plugin, ProfileState::Enforced);
    let g = h.group();
    let p = h.spawn(g, req).expect("spawns");
    let hello = h.hello(p);
    assert_eq!((hello.minor, hello.envelope.as_deref()), (1, None));
    let tok = token_of(hello.pid);
    let call = capability_sid(&capability::call("c1").unwrap());
    let (ac, mut caps) = container_of(&tok).expect("an AppContainer token");
    caps.sort();
    let mut want = vec![capability_sid(TOOLCHAIN), call.clone()];
    want.sort();
    assert_eq!(
        (ac, caps),
        (appcontainer::sid(&container).unwrap().string(), want)
    );
    // The plugin profile's process limit (`profile::plugin`'s `PROCESSES`).
    expect_prepared(&seen.lock().unwrap()[0], &call, 8);
    // It talks the plugin protocol from inside its container.
    let v = run(&mut h, p, &s.p("jobs/c1"), "test.pluginMinor", vec![]);
    assert_eq!(v["reply"]["error"]["code"], "UNKNOWN_MESSAGE", "{v}");
    // Its package and its call's directories, and nothing of server data beyond them.
    assert_eq!(
        access(&tok, &store.join("plugin.json")) & FILE_READ_DATA,
        FILE_READ_DATA
    );
    assert_eq!(
        access(&tok, &s.p("jobs/c1/out")) & FILE_WRITE_DATA,
        FILE_WRITE_DATA
    );
    for path in [s.p("auth/local.key"), s.p("jobs/j1/in/a.bin"), s.p("jobs")] {
        assert_eq!(
            access(&tok, &path) & (FILE_READ_DATA | FILE_WRITE_DATA),
            0,
            "{}",
            path.display()
        );
    }
    drop(tok);
    assert_eq!(h.exit(p).reason, ExitReason::Exited);
    drop(h);
}
