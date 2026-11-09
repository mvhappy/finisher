//! `SandboxSpawner` (plan 7C Decisions 3–5, Task 7): the job host's [`Spawner`] that applies
//! the `worker` and `plugin` profiles. Each request's grants become the profile's input
//! ([`grants`]); the policy is built and validated in `prepare`, on the host's spawn thread.
//!
//! - `worker`: the self-sandbox request (`FIN_SELF_SANDBOX {profile: "worker", args:
//!   WorkerInput}`), applied by `fin-workers` before `hello`, and one active process.
//! - `plugin` (Linux): the Landlock ruleset is built in `prepare`; the child's `pre_exec`
//!   steps are `PR_SET_NO_NEW_PRIVS`, `landlock_restrict_self` on the prepared descriptor and
//!   `RLIMIT_NOFILE`, raw calls that allocate nothing. The seccomp filter is deferred (Task 4).
//!   macOS: the program is rewritten to `sandbox-exec -p <profile>` (Task 8).
//! - Windows ([`windows`]): both profiles are created by the host in their AppContainer (the
//!   security-capabilities attribute and a private desktop); the worker keeps its self-sandbox
//!   request, which checks the token.
//!
//! Which profiles apply is decided once, at construction ([`decide`], Decision 5): a profile
//! not ready on this OS passes through as [`fin_jobhost::Unsandboxed`] did.

pub mod grants;
#[cfg(windows)]
pub mod windows;

use std::path::{Path, PathBuf};

use fin_jobhost::spawner::{
    PlatformCommand, Profile as HostProfile, ProfileCaps, ProfileState, SelfSandbox, SpawnError,
    SpawnRequest, Spawner,
};

use crate::policy::{Os, Profile};
use crate::status::{Mechanism, mechanism, ready};

/// The largest `FIN_SELF_SANDBOX` value: below Linux's 128 KiB per environment string.
pub const MAX_SELF_SANDBOX: usize = 120 * 1024;

/// What the spawner does for one profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Confine every process of the profile.
    Apply,
    /// Run as before 7C (not ready on this OS, or Landlock off: F2).
    Pass,
    /// Refuse every spawn: the profile is ready, the host lacks `need`.
    Refuse(String),
}

impl Decision {
    fn state(&self) -> ProfileState {
        match self {
            Decision::Apply => ProfileState::Enforced,
            Decision::Pass => ProfileState::Disabled,
            Decision::Refuse(_) => ProfileState::Unavailable,
        }
    }
}

/// Decision 5: a profile not in `ready` for this OS passes through; a ready one is applied
/// where the mechanism is enforced, passes where it is disabled, and is refused where it is
/// unavailable.
pub fn decide(profile: Profile, m: &Mechanism, ready: &[(Profile, Os)]) -> Decision {
    if !ready.contains(&(profile, Os::current())) {
        return Decision::Pass;
    }
    match m {
        Mechanism::Enforced { .. } => Decision::Apply,
        Mechanism::Disabled { .. } => Decision::Pass,
        Mechanism::Unavailable { need } => Decision::Refuse(need.clone()),
    }
}

/// Whether the `plugin` profile is applied on this host: what [`SandboxSpawner::new`] decides
/// and `caps().profiles.plugin` reports (`fin-registry`'s `thirdPartyCode {confined}`).
pub fn plugin_confined() -> bool {
    decide(Profile::Plugin, &mechanism(), &ready()) == Decision::Apply
}

/// The job host's sandbox spawner.
#[derive(Debug, Clone)]
pub struct SandboxSpawner {
    server_data: Option<PathBuf>,
    home: Option<PathBuf>,
    worker: Decision,
    plugin: Decision,
    #[cfg(windows)]
    desktops: std::sync::Arc<windows::Desktops>,
}

impl SandboxSpawner {
    /// Decides each profile from this host's mechanism and `READY`; `server_data` places the
    /// grants (without it, an applied profile refuses every spawn).
    pub fn new(server_data: Option<PathBuf>) -> Self {
        let m = mechanism();
        let ready = ready();
        Self::with(
            server_data,
            home_dir(),
            decide(Profile::Worker, &m, &ready),
            decide(Profile::Plugin, &m, &ready),
        )
    }

    /// Fixed decisions (tests).
    pub fn with(
        server_data: Option<PathBuf>,
        home: Option<PathBuf>,
        worker: Decision,
        plugin: Decision,
    ) -> Self {
        Self {
            server_data,
            home,
            worker,
            plugin,
            #[cfg(windows)]
            desktops: Default::default(),
        }
    }

    fn server_data(&self) -> Result<&Path, SpawnError> {
        self.server_data
            .as_deref()
            .ok_or_else(|| SpawnError("the sandbox spawner has no server data directory".into()))
    }
}

impl Spawner for SandboxSpawner {
    fn name(&self) -> &'static str {
        "fin-sandbox"
    }

    fn sandboxed(&self) -> bool {
        self.worker == Decision::Apply && self.plugin == Decision::Apply
    }

    fn profiles(&self) -> ProfileCaps {
        ProfileCaps {
            worker: self.worker.state(),
            plugin: self.plugin.state(),
        }
    }

    /// Where the `worker` profile is applied by a restriction made once at start that names
    /// paths (Landlock): a warm worker granted its own `jobs/` sees only the job visiting it
    /// (Decision 19). macOS follows once Task 8's Seatbelt worker profile exists (granting
    /// `jobs/`, never the worker's directory); Windows needs the broker's per-job ACL grants
    /// (Task 10). Both stay false until then, so warm kinds run cold there.
    fn per_job_grants(&self) -> bool {
        self.worker == Decision::Apply && cfg!(target_os = "linux")
    }

    fn prepare(&self, req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError> {
        let (profile, decision) = match req.profile {
            HostProfile::Worker => (Profile::Worker, &self.worker),
            HostProfile::Plugin => (Profile::Plugin, &self.plugin),
        };
        match decision {
            Decision::Pass => return Ok(()),
            Decision::Refuse(need) => {
                return Err(SpawnError(format!(
                    "SANDBOX_UNAVAILABLE: the {} profile needs {need}",
                    profile_name(profile)
                )));
            }
            Decision::Apply => {}
        }
        let fail = |e: String| {
            SpawnError(format!(
                "SANDBOX_UNAVAILABLE: the {} profile: {e}",
                profile_name(profile)
            ))
        };
        let server_data = self.server_data()?;
        match profile {
            Profile::Worker => {
                let input = grants::worker_input(req, server_data).map_err(fail)?;
                crate::profile::worker(&input).map_err(|e| fail(e.to_string()))?;
                #[cfg(windows)]
                let family = req.family.clone();
                let req = SelfSandbox {
                    profile: "worker".into(),
                    args: serde_json::to_value(&input).map_err(|e| fail(e.to_string()))?,
                };
                // It travels as one environment string, which Linux caps (`MAX_ARG_STRLEN`):
                // refuse clearly rather than fail `execve` with `E2BIG`.
                let len = serde_json::to_string(&req).map_or(usize::MAX, |j| j.len());
                if len > MAX_SELF_SANDBOX {
                    return Err(fail(format!(
                        "the worker input is {len} bytes, over {MAX_SELF_SANDBOX} (too many sources granted one by one)"
                    )));
                }
                #[cfg(windows)]
                {
                    let plan = crate::windows::plan::worker(&input, &family).map_err(fail)?;
                    windows::contain(&plan, &self.desktops, cmd).map_err(fail)?;
                }
                cmd.self_sandbox = Some(req);
                cmd.job_limits.active_processes = Some(1);
                Ok(())
            }
            _ => {
                let home = self.home.as_deref().ok_or_else(|| fail("no HOME".into()))?;
                let input = grants::plugin_input(req, server_data, home).map_err(fail)?;
                let policy = crate::profile::plugin(&input).map_err(|e| fail(e.to_string()))?;
                // Processes: the leaf's limit where it is enforced (`caps().leafLimits`);
                // memory stays the group's limit of 5C (Decision 4a).
                cmd.job_limits.active_processes = policy.limits.processes;
                #[cfg(windows)]
                {
                    let plan = crate::windows::plan::plugin(&input).map_err(fail)?;
                    windows::contain(&plan, &self.desktops, cmd).map_err(fail)
                }
                #[cfg(not(windows))]
                plugin_steps(&policy, cmd).map_err(fail)
            }
        }
    }
}

fn profile_name(p: Profile) -> &'static str {
    match p {
        Profile::Agent => "agent",
        Profile::Hook => "hook",
        Profile::Worker => "worker",
        Profile::Plugin => "plugin",
    }
}

/// Adds the `plugin` profile's child-side steps (Linux).
#[cfg(target_os = "linux")]
fn plugin_steps(policy: &crate::SandboxPolicy, cmd: &mut PlatformCommand) -> Result<(), String> {
    use std::os::fd::{AsRawFd, OwnedFd};

    let abi = crate::linux::landlock::query()
        .abi()
        .map_err(|e| e.to_string())?;
    let rs = crate::linux::landlock::ruleset(policy, abi).map_err(|e| e.to_string())?;
    let fd: Option<OwnedFd> = rs.into();
    let fd = fd.ok_or("the Landlock ruleset has no descriptor")?;
    let nofile = policy.limits.open_files.map(libc::rlim_t::from);
    cmd.pre_exec.push(Box::new(move || {
        // SAFETY: raw, async-signal-safe calls on data prepared before the fork; nothing is
        // allocated. The descriptor is close-on-exec, so it ends at `exec`.
        unsafe {
            if let Some(n) = nofile {
                let mut r: libc::rlimit = std::mem::zeroed();
                if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let n = n.min(r.rlim_max);
                let r = libc::rlimit {
                    rlim_cur: n,
                    rlim_max: n,
                };
                if libc::setrlimit(libc::RLIMIT_NOFILE, &r) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::syscall(libc::SYS_landlock_restrict_self, fd.as_raw_fd(), 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }));
    Ok(())
}

/// The host process's home directory (`HOME`; on Windows `USERPROFILE`).
fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var).map(PathBuf::from)
}

/// macOS: the program is rewritten to `sandbox-exec -p <profile> -D… -- <program> <args…>`
/// (the host starts children with `posix_spawn`, so there is no child-side step). The profile
/// carries a fresh Seatbelt mark, which the host's group kill sweeps (Decision 3, macOS
/// amendment). `posix_spawn` has no resource-limit attribute either, so where the policy has
/// an open-files limit `/bin/sh` sets it (soft and hard, never above the host's own hard
/// limit) and `exec`s `sandbox-exec`: `/bin/sh -c 'ulimit -n "$0" && exec "$@"' <n>
/// /usr/bin/sandbox-exec …` (invoked as `sh`, it reads no startup file).
#[cfg(target_os = "macos")]
fn plugin_steps(policy: &crate::SandboxPolicy, cmd: &mut PlatformCommand) -> Result<(), String> {
    let mark = fin_jobhost::seatbelt_mark::new_mark().map_err(|e| format!("getentropy: {e}"))?;
    let (program, args) =
        crate::macos::apply::plugin_command(policy, &cmd.program, &cmd.args, &mark)
            .map_err(|e| e.to_string())?;
    // Test builds: the wrapper also opens the session's guard file as descriptor 9 (every
    // sweep of a test build requires it, `fin_jobhost::seatbelt_mark`) and always runs.
    #[cfg(feature = "test-hooks")]
    let guard = {
        fin_jobhost::seatbelt_mark::make_guard(&mark).map_err(|e| format!("test guard: {e}"))?;
        Some(fin_jobhost::seatbelt_mark::guard_path(&mark))
    };
    #[cfg(not(feature = "test-hooks"))]
    let guard: Option<PathBuf> = None;
    cmd.seatbelt_mark = Some(mark);
    let limit = match policy.limits.open_files {
        Some(n) => {
            // SAFETY: plain data, written by getrlimit before use.
            let mut r: libc::rlimit = unsafe { std::mem::zeroed() };
            // SAFETY: a valid out pointer.
            if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) } != 0 {
                return Err(format!("getrlimit: {}", std::io::Error::last_os_error()));
            }
            Some(libc::rlim_t::from(n).min(r.rlim_max))
        }
        None => None,
    };
    if limit.is_none() && guard.is_none() {
        cmd.program = program;
        cmd.args = args;
        return Ok(());
    }
    // `$0` is the limit (or `-`), then the guard where there is one, then the command.
    let mut script = String::new();
    if guard.is_some() {
        script.push_str("exec 9<\"$1\" && shift && ");
    }
    if limit.is_some() {
        script.push_str("ulimit -n \"$0\" && ");
    }
    script.push_str("exec \"$@\"");
    let mut wrapped: Vec<std::ffi::OsString> = vec![
        "-c".into(),
        script.into(),
        limit.map_or("-".into(), |n| n.to_string()).into(),
    ];
    wrapped.extend(guard.map(PathBuf::into_os_string));
    wrapped.push(program.into_os_string());
    wrapped.extend(args);
    cmd.program = PathBuf::from(SHELL);
    cmd.args = wrapped;
    Ok(())
}

/// The shell that sets a macOS plugin's open-files limit.
#[cfg(target_os = "macos")]
const SHELL: &str = "/bin/sh";

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn plugin_steps(_policy: &crate::SandboxPolicy, _cmd: &mut PlatformCommand) -> Result<(), String> {
    Err(format!(
        "the plugin profile is not implemented on {:?} yet",
        Os::current()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::DisabledReason;

    #[test]
    fn decide_follows_decision_5() {
        let on = Mechanism::Enforced { detail: "x".into() };
        let off = Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        };
        let none = Mechanism::Unavailable { need: "k".into() };
        let ready = [(Profile::Worker, Os::current())];
        assert_eq!(decide(Profile::Worker, &on, &ready), Decision::Apply);
        assert_eq!(decide(Profile::Worker, &off, &ready), Decision::Pass);
        assert_eq!(
            decide(Profile::Worker, &none, &ready),
            Decision::Refuse("k".into())
        );
        // Not ready: as before 7C, whatever the mechanism.
        for m in [&on, &off, &none] {
            assert_eq!(decide(Profile::Plugin, m, &ready), Decision::Pass);
        }
    }

    #[test]
    fn sandboxed_only_when_every_profile_is_applied() {
        let s = |w, p| SandboxSpawner::with(None, None, w, p);
        assert!(s(Decision::Apply, Decision::Apply).sandboxed());
        assert!(!s(Decision::Apply, Decision::Pass).sandboxed());
        assert!(!s(Decision::Pass, Decision::Apply).sandboxed());
        let caps = s(Decision::Apply, Decision::Refuse("k".into())).profiles();
        assert_eq!(caps.worker, ProfileState::Enforced);
        assert_eq!(caps.plugin, ProfileState::Unavailable);
        assert_eq!(s(Decision::Pass, Decision::Pass).name(), "fin-sandbox");
    }
}
