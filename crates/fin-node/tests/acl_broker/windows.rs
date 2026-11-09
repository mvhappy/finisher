//! Windows: the broker over the real DACL, and the pipe's per-project container SIDs.

use std::ptr::null_mut;
use std::sync::mpsc;

use fin_sandbox::windows::{acl, appcontainer, capability};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_KERNEL_OBJECT};
use windows_sys::Win32::Security::{
    AccessCheck, DACL_SECURITY_INFORMATION, DuplicateTokenEx, GENERIC_MAPPING,
    GROUP_SECURITY_INFORMATION, LABEL_SECURITY_INFORMATION, MapGenericMask,
    OWNER_SECURITY_INFORMATION, PRIVILEGE_SET, PSECURITY_DESCRIPTOR, SecurityImpersonation,
    TOKEN_DUPLICATE, TOKEN_IMPERSONATE, TOKEN_QUERY, TokenImpersonation,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_ALL_ACCESS, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows_sys::Win32::System::Threading::OpenProcessToken;

use super::support::{AppContainer, CHILD_CONNECTED, CLIENT_ACCESS, READ_CONTROL, open};
use super::*;
use crate::acl_broker::apply::Windows;
use crate::pipe_win::{Event, Options as PipeOptions, PipeCore};

fn real(t: &Tree) -> Broker<Windows> {
    Broker::open(&t.data, Windows, Options::default(), &|_| true).expect("the broker opens")
}

fn session_sid(id: &str) -> String {
    capability::capability_sid(&capability::session(id).unwrap())
}

#[test]
fn a_real_grant_then_revoke_leaves_the_dacl_as_before() {
    let t = Tree::new();
    let b = real(&t);
    let before = acl::sddl(&t.tmp()).unwrap();
    let sid = session_sid("s1");
    b.grant(&grant(&t.tmp(), &sid, MODIFY, "session:s1"))
        .unwrap();
    let granted = acl::sddl(&t.tmp()).unwrap();
    assert!(granted.contains(&sid), "{granted}");
    std::fs::write(t.tmp().join("f"), "x").unwrap();
    assert!(
        acl::sddl(&t.tmp().join("f")).unwrap().contains(&sid),
        "inherited by a new file"
    );
    std::fs::remove_file(t.tmp().join("f")).unwrap();
    assert_eq!(b.revoke("session:s1", None).unwrap(), 1);
    assert_eq!(acl::sddl(&t.tmp()).unwrap(), before);
}

#[test]
fn two_concurrent_real_grants_on_one_directory_are_both_present() {
    let t = Tree::new();
    let b = Arc::new(real(&t));
    let (a, c) = (session_sid("s1"), session_sid("s2"));
    let job = t.data.join("jobs/j1");
    // A subtree, so each grant propagates for a while.
    for i in 0..300 {
        std::fs::write(job.join(format!("f{i}")), "x").unwrap();
    }
    let barrier = Arc::new(Barrier::new(2));
    let hs: Vec<_> = [(a.clone(), "job:a"), (c.clone(), "job:b")]
        .into_iter()
        .map(|(sid, owner)| {
            let (b, w, p) = (b.clone(), barrier.clone(), job.clone());
            std::thread::spawn(move || {
                w.wait();
                b.grant(&grant(&p, &sid, MODIFY, owner)).unwrap();
            })
        })
        .collect();
    hs.into_iter().for_each(|h| h.join().unwrap());
    let sddl = acl::sddl(&job).unwrap();
    assert!(sddl.contains(&a) && sddl.contains(&c), "{sddl}");
    assert_eq!(b.sweep(&HashSet::new()).revoked, 2);
}

/// Review B1: the toolchain roots admit every AppContainer already (`ALL APPLICATION
/// PACKAGES`) and this user may not re-ACL them, so the grant is done without a change.
#[test]
fn a_root_that_admits_every_container_is_granted_without_a_change() {
    let t = Tree::new();
    let b = real(&t);
    let toolchain = capability::capability_sid(capability::TOOLCHAIN);
    let roots = fin_sandbox::data::toolchain_base_present();
    assert!(!roots.is_empty());
    for root in roots {
        let p = PathBuf::from(&root);
        let before = acl::sddl(&p).unwrap();
        b.grant(&grant(&p, &toolchain, READ_EXECUTE, STATIC_OWNER))
            .unwrap_or_else(|e| panic!("{root}: {e}"));
        assert_eq!(acl::sddl(&p).unwrap(), before, "{root}: unchanged");
    }
}

/// A root that does not admit containers and whose DACL this user may not change (`OWNER
/// RIGHTS` limits the owner to read): the grant fails, naming it, and nothing is recorded.
#[test]
fn a_root_this_user_cannot_re_acl_fails_naming_it() {
    let t = Tree::new();
    let b = real(&t);
    let locked = t.project.join("workspace/s1/locked");
    std::fs::create_dir_all(&locked).unwrap();
    let user = fin_sandbox::windows::sid::current_user().unwrap().string();
    acl::set_sddl(&locked, &format!("D:P(A;;FR;;;{user})(A;;FR;;;OW)")).unwrap();
    let e = b
        .grant(&grant(&locked, &session_sid("s1"), READ, "session:s1"))
        .unwrap_err();
    assert_eq!(e.code(), "IO_ERROR", "{e}");
    assert!(e.to_string().contains("locked"), "{e}");
    assert!(b.grants().is_empty());
}

/// Review 6: a share spelling of a local path (`\\localhost\C$\…`) is refused, never
/// checked by its share form.
#[test]
fn a_share_path_is_refused() {
    let t = Tree::new();
    let b = real(&t);
    let local = t.tmp().to_string_lossy().into_owned();
    let local = local.trim_start_matches(r"\\?\");
    let (drive, rest) = local.split_once(':').unwrap();
    let share = PathBuf::from(format!(r"\\localhost\{drive}${rest}"));
    let e = b
        .grant(&grant(&share, &session_sid("s1"), MODIFY, "session:s1"))
        .unwrap_err();
    assert!(
        matches!(e.code(), "PATH_NOT_ALLOWED" | "NOT_FOUND"),
        "{}: {e}",
        share.display()
    );
    assert!(b.grants().is_empty());
}

/// The pipe's owner, group, DACL and label (`AccessCheck` needs owner and group).
struct Descriptor(PSECURITY_DESCRIPTOR);

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: LocalAlloc'ed by `GetSecurityInfo`.
        unsafe { LocalFree(self.0) };
    }
}

fn pipe_descriptor(f: &std::fs::File) -> Descriptor {
    use std::os::windows::io::AsRawHandle;
    let mut sd: PSECURITY_DESCRIPTOR = null_mut();
    let si = OWNER_SECURITY_INFORMATION
        | GROUP_SECURITY_INFORMATION
        | DACL_SECURITY_INFORMATION
        | LABEL_SECURITY_INFORMATION;
    // SAFETY: a live handle with READ_CONTROL; `sd` receives a LocalAlloc'ed descriptor.
    let rc = unsafe {
        GetSecurityInfo(
            f.as_raw_handle() as HANDLE,
            SE_KERNEL_OBJECT,
            si,
            null_mut(),
            null_mut(),
            null_mut(),
            null_mut(),
            &mut sd,
        )
    };
    assert_eq!(
        rc,
        0,
        "GetSecurityInfo: {}",
        io::Error::from_raw_os_error(rc as i32)
    );
    Descriptor(sd)
}

/// What `fin`'s access (`GENERIC_READ | FILE_WRITE_DATA`) gets from `sd` for the token of
/// `process` (impersonated): `Some(granted)`, or `None` when refused. Computed, nothing opened.
fn access_for(process: HANDLE, sd: &Descriptor) -> Option<u32> {
    // SAFETY: a live process handle; every handle opened here is closed below; the buffers
    // outlive the calls.
    unsafe {
        let mut token: HANDLE = null_mut();
        assert_ne!(
            OpenProcessToken(process, TOKEN_QUERY | TOKEN_DUPLICATE, &mut token),
            0,
            "OpenProcessToken: {}",
            io::Error::last_os_error()
        );
        let mut imp: HANDLE = null_mut();
        assert_ne!(
            DuplicateTokenEx(
                token,
                TOKEN_QUERY | TOKEN_IMPERSONATE | TOKEN_DUPLICATE,
                std::ptr::null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut imp,
            ),
            0,
            "DuplicateTokenEx: {}",
            io::Error::last_os_error()
        );
        let mapping = GENERIC_MAPPING {
            GenericRead: FILE_GENERIC_READ,
            GenericWrite: FILE_GENERIC_WRITE,
            GenericExecute: FILE_GENERIC_EXECUTE,
            GenericAll: FILE_ALL_ACCESS,
        };
        let mut desired = CLIENT_ACCESS;
        MapGenericMask(&mut desired, &mapping);
        let mut privileges = vec![0u64; 16];
        let mut len = (privileges.len() * 8) as u32;
        let (mut granted, mut status) = (0u32, 0);
        let ok = AccessCheck(
            sd.0,
            imp,
            desired,
            &mapping,
            privileges.as_mut_ptr().cast::<PRIVILEGE_SET>(),
            &mut len,
            &mut granted,
            &mut status,
        );
        let err = io::Error::last_os_error();
        CloseHandle(imp);
        CloseHandle(token);
        assert_ne!(ok, 0, "AccessCheck: {err}");
        (status != 0).then_some(granted)
    }
}

#[test]
fn pipe_allow_admits_the_projects_container_and_no_other() {
    let t = Tree::new();
    let other = t.root.join("q");
    std::fs::create_dir_all(&other).unwrap();
    let (nx, ny) = (
        capability::project_container(&t.project),
        capability::project_container(&other),
    );
    let (Some(x), Some(y)) = (AppContainer::named(&nx), AppContainer::named(&ny)) else {
        eprintln!("skipped: no AppContainer profile could be created on this host");
        return;
    };
    // The SID `sandboxContainerSid` answers is the profile's.
    assert_eq!(appcontainer::sid(&nx).unwrap().string(), x.sid);
    let name = format!(r"\\.\pipe\finisher-acl-test-{}", std::process::id());
    let core = PipeCore::new(&name, PipeOptions::default()).expect("the pipe server starts");
    let (tx, rx) = mpsc::channel();
    core.set_handler(Box::new(move |e| {
        let _ = tx.send(e);
    }));
    core.allow(&x.sid).unwrap();
    // Project X's container connects and is reported as itself.
    assert_eq!(x.probe(&name), CHILD_CONNECTED);
    match rx.recv_timeout(Duration::from_secs(10)).expect("an event") {
        Event::Connected(p) => assert_eq!(p.app_container.as_deref(), Some(x.sid.as_str())),
        Event::Refused(r) => panic!("refused: {r:?}"),
    }
    // Project Y's: computed with `AccessCheck` on its token against the pipe's descriptor.
    let ours = open(&name, READ_CONTROL).expect("this user opens the pipe");
    let sd = pipe_descriptor(&ours);
    let (wx, wy) = (
        x.start("waiting_child", &[("FIN_ACL_WAIT", "1")]),
        y.start("waiting_child", &[("FIN_ACL_WAIT", "1")]),
    );
    assert!(
        access_for(wx.process, &sd).is_some(),
        "the control: X's token is granted"
    );
    assert_eq!(
        access_for(wy.process, &sd),
        None,
        "Y's container gets no access"
    );
    drop((wx, wy, ours));
    core.shutdown();
}
