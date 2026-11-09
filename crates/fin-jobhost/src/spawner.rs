//! The `Spawner` hook (spec §6.2, Decision 9), frozen for sub-project 7: every process is
//! described by a [`SpawnRequest`], turned into a [`PlatformCommand`] by the host, and handed
//! to [`Spawner::prepare`], which may rewrite it (a wrapper program, extra environment,
//! child-side steps on Linux, attributes, a token and a desktop on Windows, leaf `job_limits`, a
//! self-sandbox request). The default is [`Unsandboxed`].

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

pub use fin_jobs::sandbox::SelfSandbox;
use fin_jobs::{FileMode, HelperKind};

/// The program of a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// A bare name resolved in the host's `binary_dir`: only [`BUNDLED`] names.
    Bundled(String),
    /// An absolute path, set only by a registered plugin family or helper resolver.
    Path(PathBuf),
}

/// The names a `Program::Bundled` may take (Decision 2). `fin-sandbox-exec` is the 7C launcher
/// (plan 7C Task 5), shipped beside the workers.
pub const BUNDLED: &[&str] = &["fin-workers", "fin-analyze", "fin-sandbox-exec"];

/// The sandbox profile a process runs under (sub-project 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Worker,
    Plugin,
}

/// What a process is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Worker,
    Helper { kind: HelperKind, id: String },
}

/// A path a process may use (for a sandbox's rules; the host itself grants nothing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathGrant {
    pub path: PathBuf,
    pub mode: FileMode,
}

/// Process priority (Decision 21): `utility` = macOS `QOS_CLASS_UTILITY`, nice 5, Windows
/// `BELOW_NORMAL_PRIORITY_CLASS`; `background` = `QOS_CLASS_BACKGROUND`, nice 10,
/// `BELOW_NORMAL_PRIORITY_CLASS`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Qos {
    #[default]
    Normal,
    Utility,
    Background,
}

/// One process to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnRequest {
    pub role: Role,
    pub profile: Profile,
    pub family: String,
    pub program: Program,
    pub args: Vec<OsString>,
    /// The whole environment (`childEnv()` and the private temp variables): nothing else is
    /// inherited.
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    pub grants: Vec<PathGrant>,
    pub qos: Qos,
    /// For logs.
    pub label: String,
}

/// Windows `JOB_OBJECT_UILIMIT_*` flags for a leaf Job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UiLimits(pub u32);

/// Limits applied to the process's leaf kill domain (Decision 17): a leaf Job or a leaf
/// cgroup. Not enforced with process groups (`caps().leaf_limits == false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct JobLimits {
    pub active_processes: Option<u32>,
    pub ui: Option<UiLimits>,
}

/// A Windows process-thread attribute (`UpdateProcThreadAttribute`): its id and value bytes
/// (sub-project 7 is its only producer). `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` and
/// `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` are the host's own and are refused.
#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinAttribute {
    pub attribute: usize,
    pub value: Box<[u8]>,
}

/// A child-side step run after the host's own (Linux `pre_exec`). It must not allocate or
/// lock: only async-signal-safe calls.
#[cfg(target_os = "linux")]
pub type PreExec = Box<dyn Fn() -> std::io::Result<()> + Send + Sync>;

/// The command the host will run, after [`Spawner::prepare`].
pub struct PlatformCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
    pub job_limits: JobLimits,
    /// Passed as `FIN_SELF_SANDBOX`; the process fails closed without it (Decision 9a).
    pub self_sandbox: Option<SelfSandbox>,
    #[cfg(target_os = "linux")]
    pub pre_exec: Vec<PreExec>,
    #[cfg(windows)]
    pub attributes: Vec<WinAttribute>,
    #[cfg(windows)]
    pub token: Option<std::os::windows::io::OwnedHandle>,
    /// `STARTUPINFO.lpDesktop` (`<window station>\<desktop>`): a sandbox's private desktop,
    /// which the spawner keeps alive. `None`: the host's own.
    #[cfg(windows)]
    pub desktop: Option<OsString>,
}

impl fmt::Debug for PlatformCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PlatformCommand")
            .field("program", &self.program)
            .field("args", &self.args)
            .field("env", &self.env)
            .field("cwd", &self.cwd)
            .field("job_limits", &self.job_limits)
            .field("self_sandbox", &self.self_sandbox)
            .finish_non_exhaustive()
    }
}

/// Why a spawner refused a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpawnError(pub String);

impl fmt::Display for SpawnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SpawnError {}

/// What a spawner does for one profile on this host (plan 7C Decision 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileState {
    /// Every process of the profile is confined.
    Enforced,
    /// Processes run unconfined, with the warnings their sub-project shows (the profile is not
    /// ready on this OS, or the mechanism is off: F2).
    Disabled,
    /// The profile is ready but the host lacks the mechanism: its spawns are refused.
    Unavailable,
}

/// [`ProfileState`] per profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileCaps {
    pub worker: ProfileState,
    pub plugin: ProfileState,
}

impl ProfileCaps {
    pub const DISABLED: ProfileCaps = ProfileCaps {
        worker: ProfileState::Disabled,
        plugin: ProfileState::Disabled,
    };
}

/// The spawn hook. `prepare` runs on the host's spawn thread for every process.
pub trait Spawner: Send + Sync {
    fn name(&self) -> &'static str;
    /// True only when every profile is enforced.
    fn sandboxed(&self) -> bool;
    fn prepare(&self, req: &SpawnRequest, cmd: &mut PlatformCommand) -> Result<(), SpawnError>;
    /// What each profile gets (`caps().profiles`).
    fn profiles(&self) -> ProfileCaps {
        ProfileCaps::DISABLED
    }
    /// A warm worker of the `worker` profile is confined to its own directory, where each job
    /// it serves visits, and its project's read roots (plan 7C Task 12, Decision 19). Plugin
    /// warm workers are not covered: they run cold under an applied `plugin` profile.
    fn per_job_grants(&self) -> bool {
        false
    }
}

/// No sandbox: name `none`, `sandboxed` false, `prepare` does nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unsandboxed;

impl Spawner for Unsandboxed {
    fn name(&self) -> &'static str {
        "none"
    }

    fn sandboxed(&self) -> bool {
        false
    }

    fn prepare(&self, _req: &SpawnRequest, _cmd: &mut PlatformCommand) -> Result<(), SpawnError> {
        Ok(())
    }
}

/// The program's path: a bundled name in `binary_dir` (only [`BUNDLED`] names, the file must
/// exist), or an absolute path.
pub(crate) fn resolve(binary_dir: &Path, program: &Program) -> Result<PathBuf, String> {
    match program {
        Program::Bundled(name) => {
            if !BUNDLED.contains(&name.as_str()) {
                return Err(format!("{name:?} is not a bundled program"));
            }
            let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
            let path = binary_dir.join(file);
            if !path.is_file() {
                return Err(format!("{} does not exist", path.display()));
            }
            Ok(path)
        }
        Program::Path(path) => {
            if !path.is_absolute() {
                return Err(format!("{} is not absolute", path.display()));
            }
            Ok(path.clone())
        }
    }
}

/// The command for `req` before the spawner sees it.
pub(crate) fn command(binary_dir: &Path, req: &SpawnRequest) -> Result<PlatformCommand, String> {
    Ok(PlatformCommand {
        program: resolve(binary_dir, &req.program)?,
        args: req.args.clone(),
        env: req.env.clone(),
        cwd: req.cwd.clone(),
        job_limits: JobLimits::default(),
        self_sandbox: None,
        #[cfg(target_os = "linux")]
        pre_exec: Vec::new(),
        #[cfg(windows)]
        attributes: Vec::new(),
        #[cfg(windows)]
        token: None,
        #[cfg(windows)]
        desktop: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_names_are_limited() {
        let dir = std::env::temp_dir();
        assert!(resolve(&dir, &Program::Bundled("sh".into())).is_err());
        assert!(resolve(&dir, &Program::Bundled("../fin-workers".into())).is_err());
        assert!(resolve(&dir, &Program::Path("relative/bin".into())).is_err());
    }
}
