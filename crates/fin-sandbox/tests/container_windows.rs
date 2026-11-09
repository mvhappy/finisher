//! The Windows `agent` and `hook` containers through the launcher (plan 7C Task 9, spec
//! §8.11 Windows). **Owner decision (2026-11-08): no escape attempts.** Each case proves the
//! sandbox by its configuration and by what a container may use:
//! - the container's token holds exactly the container and capability SIDs of the plan;
//! - the broker's table (`windows::grants`) puts the expected ACEs under an owner-only DACL,
//!   and a revoke takes them away;
//! - the job is set as configured (kill-on-close, the limits, no breakaway) and the process
//!   runs on a private desktop with crash UI off;
//! - a container reads and writes what it was granted and connects to its project's pipe;
//! - what it must not reach (another session's folder, server data, another project's pipe)
//!   is computed with `AccessCheck` on its own token, never attempted.
//!
//! Run non-elevated, at medium integrity (an elevated token hides mistakes).
#![cfg(windows)]

mod windows_support;

use std::path::PathBuf;

use fin_core::events::payloads::agent::{AgentMode, Harness};
use fin_sandbox::launch::LaunchRequest;
use fin_sandbox::windows::capability::{self, INTERNET_CLIENT, TOOLCHAIN, capability_sid};
use fin_sandbox::windows::grants::{self, AclGrant, MODIFY, Trustee};
use fin_sandbox::windows::sid::{Sid, current_user};
use fin_sandbox::windows::{acl, appcontainer};
use fin_sandbox::{AgentInput, HookInput};
use windows_support::{
    FILE_READ_DATA, FILE_WRITE_DATA, Tree, Waiting, file_access, object_access, pipe_server, probe,
    run, spawn,
};

/// A project, a second project, server data with two sessions, a home and a toolchain.
struct Setup {
    t: Tree,
    /// Container profiles this setup made, deleted on drop.
    containers: Vec<String>,
}

impl Setup {
    fn new() -> Setup {
        let t = Tree::new();
        for f in [
            "data/auth/local.key",
            "data/control.db",
            "data/agents/s1/credential.json",
            "data/agents/s1/settings/settings.json",
            "data/agents/s1/git/config",
            "data/agents/s2/credential.json",
            "data/agents/s2/tmp/note",
            "p/finisher.project.json",
            "p/sources/clip.mp4",
            "p/hooks/check.cmd",
            "q/other.txt",
            "home/.ssh/id_ed25519",
        ] {
            t.write(f, "x");
        }
        for d in [
            "p/workspace/s1",
            "p/workspace/s2",
            "p/workspace/shared",
            "data/agents/s1/tmp",
            "data/agents/s1/provider",
            "data/agents/s1/localappdata",
            "data/agents/s2/tmp",
            "data/agents/s2/provider",
            "data/agents/s2/localappdata",
            "data/hooks/h1",
            "q/workspace/s3",
            "data/agents/s3/tmp",
            "data/agents/s3/provider",
            "data/agents/s3/localappdata",
            "tools",
        ] {
            t.dir(d);
        }
        Setup {
            t,
            containers: vec![],
        }
    }

    fn agent_input(&self, session: &str, project: &str) -> AgentInput {
        let p = self.t.p(project);
        AgentInput {
            session_id: session.into(),
            mode: AgentMode::Draft,
            harness: Harness::Codex,
            project_dir: p.clone(),
            workspace_dir: p.join("workspace").join(session),
            shared_dir: Some(p.join("workspace").join("shared")),
            library_roots: vec![],
            session_dir: self.t.p("data/agents").join(session),
            sign_in_store: None,
            toolchains: vec![self.t.p("tools")],
            server_data: self.t.p("data"),
            home: self.t.p("home"),
            fin: None,
        }
    }

    /// An agent launch of `session` in `project`, its table granted first (as the broker
    /// does).
    fn agent(&mut self, session: &str, project: &str) -> LaunchRequest {
        let i = self.agent_input(session, project);
        self.apply(&grants::agent(&i).unwrap());
        self.containers
            .push(capability::project_container(&i.project_dir));
        LaunchRequest::Agent(i)
    }

    fn hook(&mut self, timeout_ms: Option<u64>) -> LaunchRequest {
        let i = HookInput {
            project_dir: self.t.p("p"),
            hook_file: self.t.p("p/hooks/check.cmd"),
            toolchains: vec![self.t.p("tools")],
            tmp: self.t.p("data/hooks/h1"),
            server_data: self.t.p("data"),
            home: self.t.p("home"),
            fin: None,
            timeout_ms,
        };
        self.apply(&grants::hook(&i).unwrap());
        self.containers
            .push(capability::project_container(&i.project_dir));
        LaunchRequest::Hook(i)
    }

    fn apply(&self, table: &[AclGrant]) {
        // As the broker: a root missing at launch is not granted.
        for g in table.iter().filter(|g| g.path.exists()) {
            let sid = match &g.trustee {
                Trustee::Sid(s) => Sid::parse(s).unwrap(),
                Trustee::Container(c) => appcontainer::sid(c).unwrap(),
            };
            acl::grant(&g.path, &sid, g.mask, true).unwrap();
        }
    }

    fn p(&self, rel: &str) -> PathBuf {
        self.t.p(rel)
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        for c in &self.containers {
            let _ = appcontainer::delete(c);
        }
    }
}

fn s(p: &std::path::Path) -> String {
    p.to_string_lossy().into_owned()
}

#[test]
fn container_profile_create_reuse_delete() {
    let name = format!("finisher-p-test{}", std::process::id());
    let a = appcontainer::ensure(&name).unwrap();
    let b = appcontainer::ensure(&name).unwrap();
    assert_eq!(a.string(), b.string(), "reused");
    assert_eq!(
        appcontainer::sid(&name).unwrap().string(),
        a.string(),
        "derived"
    );
    assert!(a.string().starts_with("S-1-15-2-"), "{}", a.string());
    appcontainer::delete(&name).unwrap();
    appcontainer::delete(&name).unwrap();
    assert!(
        appcontainer::ensure("other-name").is_err(),
        "only Finisher names"
    );
}

#[test]
fn capability_sids_match_the_windows_derivation() {
    use windows_sys::Win32::Foundation::{HLOCAL, LocalFree};
    use windows_sys::Win32::Security::{DeriveCapabilitySidsFromName, PSID};
    for name in ["finisher-session-s1", "finisher-session-S1", TOOLCHAIN] {
        let (mut groups, mut ng, mut caps, mut nc) = (
            std::ptr::null_mut::<PSID>(),
            0u32,
            std::ptr::null_mut::<PSID>(),
            0u32,
        );
        let w = fin_sandbox::windows::sid::wide(name);
        // SAFETY: a NUL-terminated name and valid out-pointers; every SID and array returned
        // is LocalFree'd once.
        let got = unsafe {
            assert_ne!(
                DeriveCapabilitySidsFromName(w.as_ptr(), &mut groups, &mut ng, &mut caps, &mut nc),
                0
            );
            assert_eq!(nc, 1);
            let s = fin_sandbox::windows::sid::string_of(*caps).unwrap();
            for i in 0..ng as usize {
                LocalFree(*groups.add(i) as HLOCAL);
            }
            LocalFree(*caps as HLOCAL);
            LocalFree(groups as HLOCAL);
            LocalFree(caps as HLOCAL);
            s
        };
        assert_eq!(capability_sid(name), got, "{name}");
    }
}

#[test]
fn agent_token_is_its_project_container_with_exactly_its_capabilities() {
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let v = probe(&req, &["token"]);
    let t = &v["token"];
    assert_eq!(t["appContainer"], true, "{v}");
    let c = appcontainer::sid(&capability::project_container(&st.p("p"))).unwrap();
    assert_eq!(t["container"], c.string());
    let want = [
        capability_sid("finisher-session-s1"),
        capability_sid(TOOLCHAIN),
        INTERNET_CLIENT.to_string(),
    ];
    let got: Vec<String> = serde_json::from_value(t["capabilities"].clone()).unwrap();
    assert_eq!(got, want);
}

#[test]
fn hook_token_has_no_network_capability() {
    let mut st = Setup::new();
    let req = st.hook(None);
    let v = probe(&req, &["token"]);
    let got: Vec<String> = serde_json::from_value(v["token"]["capabilities"].clone()).unwrap();
    let want = [
        capability_sid(TOOLCHAIN),
        capability_sid(&capability::hook(&st.p("data/hooks/h1"))),
    ];
    assert_eq!(got, want);
}

#[test]
fn granted_folders_are_usable() {
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let ws = st.p("p/workspace/s1/draft.txt");
    assert_eq!(probe(&req, &["write", &s(&ws), "d1"])["ok"], true);
    assert_eq!(probe(&req, &["read", &s(&ws)])["ok"], "d1");
    let tmp = st.p("data/agents/s1/tmp/made");
    assert_eq!(probe(&req, &["mkdir", &s(&tmp)])["ok"], true);
    assert_eq!(
        probe(&req, &["read", &s(&st.p("p/finisher.project.json"))])["ok"],
        "x"
    );
    assert_eq!(
        probe(
            &req,
            &["read", &s(&st.p("data/agents/s1/settings/settings.json"))]
        )["ok"],
        "x"
    );
    assert!(probe(&req, &["list", &s(&st.p("p/sources"))])["ok"].as_u64() == Some(1));
    // TEMP is the session's own (S9 point 7), and writable.
    let temp = probe(&req, &["env", "TEMP"])["value"]
        .as_str()
        .unwrap()
        .to_owned();
    let c = capability::project_container(&st.p("p"));
    let want = st
        .p("data/agents/s1/localappdata/Packages")
        .join(&c)
        .join("AC")
        .join("Temp");
    assert_eq!(PathBuf::from(&temp), want);
    let f = PathBuf::from(&temp).join("t.txt");
    assert_eq!(probe(&req, &["write", &s(&f), "t"])["ok"], true);
}

#[test]
fn grants_add_the_capability_ace_under_the_owner_only_dacl_and_revoke_removes_it() {
    let st = Setup::new();
    let dir = st.p("data/agents/s1/provider");
    let before = acl::sddl(&dir).unwrap();
    let sid = Sid::parse(&capability_sid("finisher-session-s9")).unwrap();
    acl::grant(&dir, &sid, MODIFY, true).unwrap();
    let after = acl::sddl(&dir).unwrap();
    let ace = format!("(A;OICI;0x1301bf;;;{})", sid.string());
    assert!(after.contains(&ace), "{after}");
    // The owner-only entries are still there (inherited from the protected root), and nobody
    // else is: the user, SYSTEM, Administrators and the capability.
    let user = current_user().unwrap().string();
    for who in [user.as_str(), "SY", "BA"] {
        assert!(after.contains(&format!(";;;{who})")), "{who}: {after}");
    }
    let root = acl::sddl(&st.t.t).unwrap();
    assert!(root.contains("D:P"), "protected root: {root}");
    assert!(
        !after.contains(";;;WD)") && !after.contains(";;;BU)") && !after.contains(";;;AC)"),
        "{after}"
    );
    assert_eq!(acl::revoke(&dir, &sid).unwrap(), 1);
    assert_eq!(acl::sddl(&dir).unwrap(), before);
    assert_eq!(acl::revoke(&dir, &sid).unwrap(), 0);
}

#[test]
fn another_sessions_folder_and_server_data_grant_the_container_nothing() {
    let mut st = Setup::new();
    let a = st.agent("s1", "p");
    let b = st.agent("s2", "p");
    let wa = Waiting::start(&a);
    let wb = Waiting::start(&b);
    let (ta, tb) = (wa.token_handle(), wb.token_handle());
    // Controls: each session's own folders are open to it.
    let rw = FILE_READ_DATA | FILE_WRITE_DATA;
    assert_eq!(file_access(&ta, &st.p("data/agents/s1/tmp")) & rw, rw);
    assert_eq!(file_access(&tb, &st.p("data/agents/s2/tmp")) & rw, rw);
    assert_ne!(file_access(&ta, &st.p("p/sources")) & FILE_READ_DATA, 0);
    for p in [
        "data/agents/s2",
        "data/agents/s2/tmp",
        "data/agents/s2/tmp/note",
        "data/agents/s2/credential.json",
        "data/agents/s2/localappdata",
        "p/workspace/s2",
    ] {
        let got = file_access(&ta, &st.p(p));
        // Other sessions' workspaces stay readable (owner decision, Decision 6): no write.
        if p == "p/workspace/s2" {
            assert_eq!(got & FILE_WRITE_DATA, 0, "{p}: {got:#x}");
        } else {
            assert_eq!(got, 0, "{p}: {got:#x}");
        }
    }
    assert_eq!(file_access(&tb, &st.p("data/agents/s1/tmp")), 0);
    for p in [
        "data",
        "data/auth/local.key",
        "data/control.db",
        "data/agents",
        "q/other.txt",
        "home/.ssh/id_ed25519",
    ] {
        assert_eq!(file_access(&ta, &st.p(p)), 0, "{p}");
    }
    // Protected: the session's settings are read-only to it.
    assert_eq!(
        file_access(&ta, &st.p("data/agents/s1/settings")) & FILE_WRITE_DATA,
        0
    );
}

#[test]
fn own_project_pipe_connects_and_another_projects_grants_nothing() {
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let user = current_user().unwrap().string();
    let sddl_for =
        |c: &str| format!("D:P(A;;GA;;;{user})(A;;GA;;;SY)(A;;0x12008b;;;{c})S:(ML;;NW;;;LW)");
    let mine = appcontainer::sid(&capability::project_container(&st.p("p")))
        .unwrap()
        .string();
    let q = capability::project_container(&st.p("q"));
    st.containers.push(q.clone());
    let theirs = appcontainer::ensure(&q).unwrap().string();
    let name = format!(r"\\.\pipe\fin-sandbox-test-{}", std::process::id());
    let server = pipe_server(&name, &sddl_for(&mine));
    let h = server.try_clone().unwrap();
    let reader = std::thread::spawn(move || {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
        // SAFETY: our pipe handle, a blocking connect.
        unsafe { ConnectNamedPipe(h.as_raw_handle() as _, std::ptr::null_mut()) };
        let mut f = std::fs::File::from(h);
        let mut buf = [0u8; 3];
        std::io::Read::read_exact(&mut f, &mut buf).map(|()| buf)
    });
    assert_eq!(probe(&req, &["pipe", &name])["ok"], true);
    assert_eq!(&reader.join().unwrap().unwrap(), b"hi\n");
    // Another project's pipe: the same container is granted nothing that `fin` asks for.
    let w = Waiting::start(&req);
    let tok = w.token_handle();
    let other = pipe_server(&format!("{name}-q"), &sddl_for(&theirs));
    assert_eq!(
        object_access(&tok, &other) & (FILE_READ_DATA | FILE_WRITE_DATA),
        0
    );
    let own = pipe_server(&format!("{name}-p"), &sddl_for(&mine));
    let want = FILE_READ_DATA | FILE_WRITE_DATA;
    assert_eq!(object_access(&tok, &own) & want, want, "control");
}

#[test]
fn job_is_kill_on_close_with_ui_limits_and_no_breakaway() {
    use fin_sandbox::windows::job::{BREAKAWAY, UI_LIMITS};
    const KILL_ON_CLOSE: u32 = 0x2000;
    const DIE_ON_UNHANDLED: u32 = 0x400;
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let v = probe(&req, &["job"]);
    let flags = v["flags"].as_u64().unwrap() as u32;
    assert_eq!(
        flags & (KILL_ON_CLOSE | DIE_ON_UNHANDLED),
        KILL_ON_CLOSE | DIE_ON_UNHANDLED,
        "{v}"
    );
    assert_eq!(flags & BREAKAWAY, 0, "{v}");
    assert_eq!(v["ui"].as_u64(), Some(u64::from(UI_LIMITS)), "{v}");
}

#[test]
fn job_limits_are_set_as_configured() {
    use fin_sandbox::policy::Limits;
    use fin_sandbox::windows::job::{BREAKAWAY, ContainerJob};
    let job = ContainerJob::new(&Limits {
        processes: Some(1),
        memory: Some(64 << 20),
        ..Limits::default()
    })
    .unwrap();
    let l = job.limits().unwrap();
    assert_eq!(l.active_processes, Some(1));
    assert_eq!(l.job_memory, Some(64 << 20));
    assert_eq!(l.flags & BREAKAWAY, 0);
    let none = ContainerJob::new(&Limits::default())
        .unwrap()
        .limits()
        .unwrap();
    assert_eq!((none.active_processes, none.job_memory), (None, None));
}

#[test]
fn the_tree_ends_when_the_launcher_dies() {
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let mut w = Waiting::start(&req);
    let pid = w.pid();
    assert!(!windows_support::ends_within(pid, 300), "running");
    w.child.kill().unwrap();
    assert!(
        windows_support::ends_within(pid, 5000),
        "the job ended the container process"
    );
}

#[test]
fn runs_on_a_private_desktop_with_crash_ui_off() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
    };
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    let d = probe(&req, &["desktop"]);
    let station = d["station"].as_str().unwrap_or_default();
    assert!(
        !station.is_empty() && !station.eq_ignore_ascii_case("WinSta0"),
        "{d}"
    );
    let desk = d["desktop"].as_str().unwrap_or_default();
    assert!(
        desk.starts_with("fin-") && !desk.eq_ignore_ascii_case("Default"),
        "{d}"
    );
    let mode = probe(&req, &["errormode"])["errorMode"].as_u64().unwrap() as u32;
    let want = SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX;
    assert_eq!(mode & want, want, "inherited error mode {mode:#x}");
}

#[test]
fn wall_time_ends_the_tree_and_reports_it() {
    let mut st = Setup::new();
    let req = st.hook(Some(2500));
    let t0 = std::time::Instant::now();
    let mut child = spawn(&req, &[windows_support::PROBE, "wait"], &[]);
    // Its stdin stays open: the probe would end at its close.
    let stdin = child.stdin.take();
    let out = child.wait_with_output().unwrap();
    drop(stdin);
    assert_eq!(out.status.code(), Some(fin_sandbox::launch::EXIT_WALL_TIME));
    assert!(t0.elapsed().as_secs() < 10, "{:?}", t0.elapsed());
    let err = String::from_utf8_lossy(&out.stderr);
    // No I/O at the end: reported as a Node piped-stdio stall (the backstop, Decision 25).
    assert!(
        err.contains(r#""code":"SANDBOX_VIOLATION""#) && err.contains(r#""op":"nodePipes""#),
        "{err}"
    );
}

#[test]
fn a_crash_in_the_container_shows_no_window_and_keeps_no_report() {
    let mut st = Setup::new();
    let req = st.agent("s1", "p");
    for (how, code) in [("av", 0xC000_0005u32), ("fastfail", 0xC000_0409)] {
        let watch = windows_support::wer_watch("fin-sandbox-probe", 8);
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let r = run(&req, &[windows_support::PROBE, "crash", how]);
        assert_eq!(r.code.map(|c| c as u32), Some(code), "{how}: {r:?}");
        let seen = watch.wait_with_output().unwrap();
        let seen = String::from_utf8_lossy(&seen.stdout);
        assert!(seen.trim().is_empty(), "{how}: {seen}");
    }
}

#[test]
fn concurrent_launches_of_different_projects_all_start() {
    // Every launcher of the user shares one unnamed window station: each launch needs a
    // desktop of its own that admits its container.
    let mut st = Setup::new();
    let reqs = [st.agent("s1", "p"), st.agent("s3", "q")];
    std::thread::scope(|sc| {
        let runs: Vec<_> = (0..8)
            .map(|n| {
                let req = &reqs[n % 2];
                sc.spawn(move || run(req, &[windows_support::PROBE, "token"]))
            })
            .collect();
        for r in runs {
            let r = r.join().unwrap();
            assert_eq!(r.code, Some(0), "{r:?}");
            assert_eq!(r.json()["token"]["appContainer"], true, "{r:?}");
        }
    });
}
