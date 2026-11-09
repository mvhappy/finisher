//! The Windows launcher (`fin-sandbox-exec`, plan 7C Decisions 3, 16, 22 and 25): it confines
//! the program in its project's container and stays its parent.
//!
//! 1. Crash UI off for itself and so for the container (Decision 22).
//! 2. The container profile, a private window station and desktop, and the job (the policy's
//!    limits, kill-on-close: when the launcher dies, the container's tree dies with it).
//! 3. The environment: the launcher's own minus [`crate::launch::SOCKET_ENV`] and
//!    [`crate::launch::REFUSED_ENV`], with `LOCALAPPDATA` in a write root of the request and
//!    `TEMP`, `TMP`, `TMPDIR` at the folder Windows derives from it (S9 point 7, so `TEMP` is
//!    per session, not per project), and the Node guard (Decision 25): `NODE_OPTIONS` carries
//!    `--preserve-symlinks --preserve-symlinks-main --require <guard>` and
//!    [`NODE_GUARD_ENV`] marks the container. A program named `node` gets the same flags on its
//!    command line.
//! 4. `CreateProcessW` in the container ([`super::apply::start`]), then a wait that drains the
//!    job's limit messages (one `SANDBOX_VIOLATION` line each on stderr) and ends the tree at
//!    the policy's wall time (`op: nodePipes` when no I/O happened in the last
//!    [`IDLE_FOR_NODE_PIPES`], the signature of a Node spawn stuck on piped stdio; else
//!    `wallTime`), exit [`crate::launch::EXIT_WALL_TIME`]. Otherwise the exit code is the
//!    program's.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

use super::apply::{Container, start};
use super::desktop::PrivateDesktop;
use super::job::ContainerJob;
use super::plan::ContainerPlan;
use super::raw;
use crate::launch::{EXIT_WALL_TIME, REFUSED_ENV, SOCKET_ENV, violation_line};
use crate::policy::{Profile, SandboxPolicy};

/// The marker the Node guard looks for: set only in a container's environment.
pub const NODE_GUARD_ENV: &str = "FIN_SANDBOX_NODE_GUARD";
/// The guard, shipped inside the launcher and written into the process's `LOCALAPPDATA`.
pub const NODE_GUARD: &str = include_str!("../../data/windows/node-guard.cjs");
/// Its file name there.
pub const NODE_GUARD_FILE: &str = "finisher-node-guard.cjs";
/// No I/O for this long before the wall time means a Node spawn stuck on piped stdio.
pub const IDLE_FOR_NODE_PIPES: Duration = Duration::from_secs(2);
/// The flags Node needs in a container (S9 point 8a: its JS `realpath` `lstat`s the drive root).
pub const NODE_FLAGS: [&str; 2] = ["--preserve-symlinks", "--preserve-symlinks-main"];

/// Whether `program` is Node (`node`, `node.exe`, any case, any directory).
pub fn is_node(program: &std::ffi::OsStr) -> bool {
    Path::new(program)
        .file_stem()
        .is_some_and(|s| s.eq_ignore_ascii_case("node"))
}

/// `NODE_OPTIONS` for the container: the flags and the guard before what was there.
pub fn node_options(guard: &Path, existing: Option<&std::ffi::OsStr>) -> OsString {
    // Node reads `NODE_OPTIONS` with `\` as an escape character, inside quotes too.
    let quoted = guard
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"");
    let mut s = OsString::from(format!(
        "{} {} --require \"{quoted}\"",
        NODE_FLAGS[0], NODE_FLAGS[1]
    ));
    if let Some(e) = existing.filter(|e| !e.is_empty()) {
        s.push(" ");
        s.push(e);
    }
    s
}

/// The program's command line: Node gets its flags and the guard right after its name.
pub fn argv_for(argv: &[OsString], guard: &Path) -> Vec<OsString> {
    if !is_node(&argv[0]) {
        return argv.to_vec();
    }
    let mut out = vec![argv[0].clone()];
    out.extend(NODE_FLAGS.iter().map(OsString::from));
    out.push("--require".into());
    out.push(guard.as_os_str().to_owned());
    out.extend(argv[1..].iter().cloned());
    out
}

/// The container's environment from the launcher's `base` (pure, for tests).
pub fn environment(
    base: impl IntoIterator<Item = (OsString, OsString)>,
    plan: &ContainerPlan,
    guard: &Path,
) -> Vec<(OsString, OsString)> {
    let dropped = |k: &OsString| {
        let k = k.to_string_lossy();
        SOCKET_ENV
            .iter()
            .chain(REFUSED_ENV)
            .any(|v| k.eq_ignore_ascii_case(v))
            || [
                "LOCALAPPDATA",
                "TEMP",
                "TMP",
                "TMPDIR",
                "NODE_OPTIONS",
                NODE_GUARD_ENV,
            ]
            .iter()
            .any(|v| k.eq_ignore_ascii_case(v))
    };
    let mut existing_node = None;
    let mut env: Vec<(OsString, OsString)> = Vec::new();
    for (k, v) in base {
        if k.to_string_lossy().eq_ignore_ascii_case("NODE_OPTIONS") {
            existing_node = Some(v.clone());
        }
        if !dropped(&k) {
            env.push((k, v));
        }
    }
    if let (Some(lad), Some(temp)) = (&plan.local_app_data, plan.temp()) {
        env.push(("LOCALAPPDATA".into(), lad.into()));
        for k in ["TEMP", "TMP", "TMPDIR"] {
            env.push((k.into(), temp.clone().into()));
        }
    }
    env.push((NODE_GUARD_ENV.into(), "1".into()));
    env.push((
        "NODE_OPTIONS".into(),
        node_options(guard, existing_node.as_deref()),
    ));
    env
}

/// Makes `LOCALAPPDATA`, the temp Windows derives from it and the guard file.
fn prepare(plan: &ContainerPlan) -> Result<PathBuf, String> {
    let lad = plan
        .local_app_data
        .as_ref()
        .ok_or("a launch plan names LOCALAPPDATA")?;
    let temp = plan.temp().ok_or("a launch plan names a temp")?;
    std::fs::create_dir_all(&temp).map_err(|e| format!("{}: {e}", temp.display()))?;
    let guard = lad.join(NODE_GUARD_FILE);
    std::fs::write(&guard, NODE_GUARD).map_err(|e| format!("{}: {e}", guard.display()))?;
    Ok(guard)
}

/// Why [`run`] ended without the program's exit code.
pub enum Failure {
    /// The container could not be made or the program started: `SANDBOX_UNAVAILABLE`, or
    /// [`std::io::ErrorKind::NotFound`] and the like for the program.
    Setup(String),
    Start(std::io::Error),
}

/// Runs `argv` in the container of `plan` under `policy`; the exit code to leave with.
pub fn run(
    profile: Profile,
    policy: &SandboxPolicy,
    plan: &ContainerPlan,
    argv: &[OsString],
) -> Result<i32, Failure> {
    super::wer::crash_ui_off();
    let guard = prepare(plan).map_err(Failure::Setup)?;
    let container =
        Container::of(plan).map_err(|e| Failure::Setup(format!("the container: {e}")))?;
    let desktop = PrivateDesktop::new(&container.sid.string())
        .map_err(|e| Failure::Setup(format!("the private desktop: {e}")))?;
    let job =
        ContainerJob::new(&policy.limits).map_err(|e| Failure::Setup(format!("the job: {e}")))?;
    let env = environment(std::env::vars_os(), plan, &guard);
    let argv = argv_for(argv, &guard);
    let started = start(&container, &job, &desktop, &argv, &env, None).map_err(Failure::Start)?;
    let deadline = policy
        .limits
        .wall_ms
        .map(|ms| Instant::now() + Duration::from_millis(ms));
    let mut reported = Vec::new();
    let mut report = |job: &ContainerJob| {
        for v in job.violations() {
            if !reported.contains(&v) {
                reported.push(v);
                eprintln!("{}", violation_line(profile, v.op()));
            }
        }
    };
    let (mut last_io, mut io_changed) = (job.io_operations(), Instant::now());
    loop {
        // SAFETY: our process handle, with SYNCHRONIZE access.
        let w = unsafe { WaitForSingleObject(raw(&started.process), 200) };
        report(&job);
        if w == WAIT_OBJECT_0 {
            let mut code = 0u32;
            // SAFETY: as above; a valid out-pointer.
            unsafe { GetExitCodeProcess(raw(&started.process), &mut code) };
            return Ok(code as i32);
        }
        if w != WAIT_TIMEOUT {
            return Err(Failure::Setup(format!(
                "waiting for the program: {}",
                std::io::Error::last_os_error()
            )));
        }
        let io = job.io_operations();
        if io != last_io {
            (last_io, io_changed) = (io, Instant::now());
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            let op = if io_changed.elapsed() >= IDLE_FOR_NODE_PIPES {
                "nodePipes"
            } else {
                "wallTime"
            };
            job.terminate(EXIT_WALL_TIME as u32);
            eprintln!("{}", violation_line(profile, op));
            return Ok(EXIT_WALL_TIME);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::plan::Capability;

    fn plan() -> ContainerPlan {
        ContainerPlan {
            container: "finisher-p-x".into(),
            capabilities: vec![Capability {
                name: None,
                sid: "S-1-15-3-1".into(),
            }],
            local_app_data: Some(PathBuf::from(r"C:\s\tmp\appdata")),
        }
    }

    #[test]
    fn environment_is_private_and_guarded() {
        let base = [
            ("Path", r"C:\bin"),
            ("TEMP", r"C:\shared"),
            ("node_options", "--max-old-space-size=100"),
            ("SSH_AUTH_SOCK", "x"),
            ("FIN_SELF_SANDBOX", "{}"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = environment(base, &plan(), Path::new(r"C:\s\tmp\appdata\g.cjs"));
        let get = |k: &str| {
            env.iter()
                .filter(|(n, _)| n.to_string_lossy().eq_ignore_ascii_case(k))
                .map(|(_, v)| v.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        };
        let temp = r"C:\s\tmp\appdata\Packages\finisher-p-x\AC\Temp";
        assert_eq!(get("TEMP"), [temp]);
        assert_eq!(get("TMPDIR"), [temp]);
        assert_eq!(get("LOCALAPPDATA"), [r"C:\s\tmp\appdata"]);
        assert_eq!(get("Path"), [r"C:\bin"]);
        assert!(get("SSH_AUTH_SOCK").is_empty() && get("FIN_SELF_SANDBOX").is_empty());
        assert_eq!(get(NODE_GUARD_ENV), ["1"]);
        assert_eq!(
            get("NODE_OPTIONS"),
            [
                r#"--preserve-symlinks --preserve-symlinks-main --require "C:\\s\\tmp\\appdata\\g.cjs" --max-old-space-size=100"#
            ]
        );
    }

    #[test]
    fn node_gets_its_flags_on_the_command_line() {
        let g = Path::new(r"C:\g.cjs");
        let a = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            argv_for(&a(&[r"C:\n\NODE.EXE", "s.js"]), g),
            a(&[
                r"C:\n\NODE.EXE",
                NODE_FLAGS[0],
                NODE_FLAGS[1],
                "--require",
                r"C:\g.cjs",
                "s.js"
            ])
        );
        assert_eq!(argv_for(&a(&["cmd", "/c"]), g), a(&["cmd", "/c"]));
        assert!(!is_node("nodemon.exe".as_ref()));
    }
}
