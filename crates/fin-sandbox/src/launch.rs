//! The launcher (plan 7C Decision 3, Task 5): `fin-sandbox-exec --policy-fd <n> -- <argv…>`
//! confines a program `finisherd` starts itself (an agent harness, a hook command): on Linux it
//! forks it and stays its parent, the session's reaper ([`crate::linux::reaper`], Decision 3 as
//! amended); elsewhere it becomes it.
//!
//! The request arrives on the inherited descriptor `<n>` (never on argv, which every process of
//! the user can read, and never in the environment): a [`LaunchRequest`], the profile and its
//! typed input, from which [`LaunchRequest::policy`] builds the policy, so nothing outside this
//! crate assembles one (Decision 1). The launcher reads the descriptor to its end and closes it,
//! applies the policy (Linux: Landlock, then `RLIMIT_NOFILE` from the policy's limits, in the
//! forked child; the seccomp filter is deferred, plan Task 4; macOS: Seatbelt through
//! `sandbox_init`, which the program inherits across `exec`, then `RLIMIT_NOFILE`) and `exec`s
//! the program. Before Landlock, where it
//! is enforced, the launcher hides Unix sockets in a user and mount namespace (Decision 26,
//! [`crate::linux::namespace`]) when a forked probe shows it can, and in every mode it removes
//! [`SOCKET_ENV`] from the program's environment. `--probe` prints whether the namespace is
//! available (`finisherd`'s `SANDBOX_SOCKETS_EXPOSED`). On Linux the launcher
//! forwards the signals it is sent to the program's group and exits as the program did (its
//! code, or its signal raised again); `SIGHUP` ends the session instead, killing every
//! descendant. Elsewhere it `exec`s, so the exit code and signals are the program's own.
//!
//! **Windows** (plan 7C Task 9, [`crate::windows::launcher`]): `<n>` is a C runtime descriptor
//! (Node's `stdio[3]`) or else an inherited handle's value. The launcher cannot become the
//! program, so it starts it in the project's AppContainer, in a kill-on-close job, on a private
//! desktop, and stays its parent: its exit code is the program's, and the program's tree ends
//! when the launcher does. A limit the job reports adds a `SANDBOX_VIOLATION` line
//! ([`violation_line`]) on the request channel (on stderr when the channel is one-way, which a
//! parent must not trust: the program shares it); the wall time ends the tree with
//! [`EXIT_WALL_TIME`].
//!
//! Exit codes of the launcher itself: [`EXIT_USAGE`] for a malformed command line or a request
//! offered anywhere but the descriptor; [`EXIT_UNAVAILABLE`] when the request is unreadable or
//! invalid, or the mechanism is enabled but the policy cannot be applied
//! (`SANDBOX_UNAVAILABLE`); 126/127 when the program cannot be started (refused, or missing).
//! Each writes one JSON line on stderr. Where Landlock is disabled (F2,
//! `Disabled {landlockDisabled}`), the program runs unconfined: `finisherd` already reports
//! that from `sandboxStatus()`. Any other `Disabled` reason (none on Linux, macOS or Windows
//! today) is refused with exit 78.
//!
//! The descriptor is read to end of file: the parent must close its copy of the pipe's write
//! end in the child (and its own after writing), or the launcher waits forever. It stays open,
//! close-on-exec, until the `exec`: on a socketpair, each of the launcher's own failure lines is
//! also written back on it, so the parent tells the launcher's exit 78 from a program's (plan
//! 7C Task 11 review).

use std::ffi::OsString;

use serde::{Deserialize, Serialize};

use crate::inputs::{AgentInput, HookInput};
use crate::policy::{Os, Profile, SandboxPolicy};

/// A malformed command line, or a request offered on argv or in the environment.
pub const EXIT_USAGE: i32 = 64;
/// `SANDBOX_UNAVAILABLE` (the worker's self-sandbox uses the same code).
pub const EXIT_UNAVAILABLE: i32 = 78;
/// The program could not be started (`EACCES` and the like), as shells report it.
pub const EXIT_CANNOT_EXEC: i32 = 126;
/// The program does not exist.
pub const EXIT_NOT_FOUND: i32 = 127;
/// The program's tree was ended at the policy's wall time (as `timeout` reports it).
pub const EXIT_WALL_TIME: i32 = 124;
/// The largest request read from the descriptor.
pub const MAX_REQUEST: u64 = 1 << 20;
/// Environment variables that would carry a policy or a sandbox request; the launcher refuses
/// to start while one is set, so a request can only come through the descriptor.
pub const REFUSED_ENV: &[&str] = &[
    "FIN_SANDBOX_POLICY",
    "FIN_SANDBOX_REQUEST",
    "FIN_SELF_SANDBOX",
];

/// Variables that name a socket, an agent or a display (Decision 26): removed from the
/// program's environment in every mode, with or without the namespace that hides the sockets
/// themselves. `finisherd`'s own allow-list (`NEVER_PASSED`, Task 11) is the first layer.
pub const SOCKET_ENV: &[&str] = &[
    "SSH_AUTH_SOCK",
    "SSH_AGENT_PID",
    "GPG_AGENT_INFO",
    "DBUS_SESSION_BUS_ADDRESS",
    "DBUS_SYSTEM_BUS_ADDRESS",
    "DOCKER_HOST",
    "CONTAINER_HOST",
    "XDG_RUNTIME_DIR",
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XAUTHORITY",
    "PULSE_SERVER",
    "TMUX",
    "TMUX_PANE",
    "SWAYSOCK",
    "I3SOCK",
    "KITTY_LISTEN_ON",
    "WEZTERM_UNIX_SOCKET",
    "NVIM",
    "NVIM_LISTEN_ADDRESS",
    "VSCODE_IPC_HOOK_CLI",
    "VSCODE_GIT_IPC_HANDLE",
    "GNOME_KEYRING_CONTROL",
    "NOTIFY_SOCKET",
    "LISTEN_FDS",
    "LISTEN_PID",
    "LISTEN_FDNAMES",
    "AT_SPI_BUS_ADDRESS",
    "DBUS_STARTER_ADDRESS",
    "PIPEWIRE_REMOTE",
    "WAYLAND_SOCKET",
    "STY",
    "ZELLIJ",
];

/// What the descriptor carries: a profile the launcher applies and its typed input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "profile", content = "input")]
pub enum LaunchRequest {
    Agent(AgentInput),
    Hook(HookInput),
}

impl LaunchRequest {
    pub fn profile(&self) -> Profile {
        match self {
            LaunchRequest::Agent(_) => Profile::Agent,
            LaunchRequest::Hook(_) => Profile::Hook,
        }
    }

    /// The user's home directory the input names (hidden by the namespace, Decision 26).
    pub fn home(&self) -> &std::path::Path {
        match self {
            LaunchRequest::Agent(i) => &i.home,
            LaunchRequest::Hook(i) => &i.home,
        }
    }

    /// The policy, built by the profile's builder and validated.
    pub fn policy(&self) -> Result<SandboxPolicy, crate::PolicyError> {
        match self {
            LaunchRequest::Agent(i) => crate::profile::agent(i),
            LaunchRequest::Hook(i) => crate::profile::hook(i),
        }
    }
}

/// The parsed command line: a launch, or `--probe` (prints a [`crate::LauncherProbe`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Launch(Args),
    Probe,
}

/// [`parse_args`], or `--probe` alone.
pub fn parse_command(args: &[OsString]) -> Result<Command, String> {
    if args.len() == 1 && args[0] == "--probe" {
        return Ok(Command::Probe);
    }
    parse_args(args).map(Command::Launch)
}

/// The parsed command line of a launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub policy_fd: i32,
    /// The program and its arguments, as given (they need not be UTF-8).
    pub argv: Vec<OsString>,
}

/// Parses `--policy-fd <n> -- <program> [args…]`. Nothing else is accepted before `--`, so a
/// policy cannot be passed on argv; `<n>` must be 3 or above (not stdio).
pub fn parse_args(args: &[OsString]) -> Result<Args, String> {
    let mut fd = None;
    let mut i = 0;
    while i < args.len() {
        let Some(flag) = args[i].to_str() else {
            return Err(format!(
                "unexpected {:?}: the request comes only through --policy-fd",
                args[i]
            ));
        };
        match flag {
            "--" => {
                let argv = args[i + 1..].to_vec();
                let policy_fd = fd.ok_or("--policy-fd is required")?;
                if argv.is_empty() {
                    return Err("no program after --".into());
                }
                return Ok(Args { policy_fd, argv });
            }
            "--policy-fd" if fd.is_none() => {
                let n: i32 = args
                    .get(i + 1)
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.parse().ok())
                    .ok_or("--policy-fd needs a descriptor number")?;
                if n < 3 {
                    return Err(format!("--policy-fd {n} is a standard stream"));
                }
                fd = Some(n);
                i += 2;
            }
            other => {
                return Err(format!(
                    "unexpected {other:?}: the request comes only through --policy-fd"
                ));
            }
        }
    }
    Err("missing -- before the program".into())
}

/// The first of [`REFUSED_ENV`] that `get` finds set.
pub fn refused_env(get: impl Fn(&str) -> bool) -> Option<&'static str> {
    REFUSED_ENV.iter().copied().find(|v| get(v))
}

/// Parses and checks a request: unreadable JSON, an unknown profile or an input its builder
/// refuses are all errors (`SANDBOX_UNAVAILABLE`).
pub fn read_request(bytes: &[u8]) -> Result<(LaunchRequest, SandboxPolicy), String> {
    let req: LaunchRequest =
        serde_json::from_slice(bytes).map_err(|e| format!("unreadable request: {e}"))?;
    let policy = req.policy().map_err(|e| e.to_string())?;
    Ok((req, policy))
}

/// The JSON line the launcher writes on stderr for one of its own failures.
pub fn failure_line(code: &str, profile: Option<Profile>, need: &str) -> String {
    serde_json::json!({
        "code": code,
        "profile": profile,
        "os": Os::current(),
        "need": need,
    })
    .to_string()
}

/// The JSON line the launcher writes on stderr for a limit the OS reported (a
/// `sandbox.violation`, plan 7C Decision 24): `processLimit`, `memoryLimit`, `wallTime`,
/// `nodePipes`.
pub fn violation_line(profile: Profile, op: &str) -> String {
    serde_json::json!({
        "code": "SANDBOX_VIOLATION",
        "profile": profile,
        "os": Os::current(),
        "op": op,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    #[test]
    fn args_only_through_the_descriptor() {
        assert_eq!(
            parse_args(&s(&["--policy-fd", "3", "--", "sh", "-c", "x"])),
            Ok(Args {
                policy_fd: 3,
                argv: s(&["sh", "-c", "x"])
            })
        );
        for bad in [
            &["--policy", "{}", "--", "sh"][..],
            &["--policy-fd", "3", "--policy-fd", "4", "--", "sh"],
            &["--policy-fd", "1", "--", "sh"],
            &["--policy-fd", "x", "--", "sh"],
            &["--policy-fd", "3", "--"],
            &["--policy-fd", "3", "sh"],
            &["--", "sh"],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
        assert_eq!(parse_command(&s(&["--probe"])), Ok(Command::Probe));
        assert!(parse_command(&s(&["--probe", "--", "sh"])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn program_arguments_need_not_be_utf8() {
        use std::os::unix::ffi::OsStrExt;
        let odd = std::ffi::OsStr::from_bytes(b"\xff").to_os_string();
        let mut a = s(&["--policy-fd", "3", "--", "echo"]);
        a.push(odd.clone());
        assert_eq!(parse_args(&a).unwrap().argv[1], odd);
    }

    #[test]
    fn a_request_names_agent_or_hook() {
        assert!(read_request(br#"{"profile":"worker","input":{}}"#).is_err());
        assert!(read_request(b"not json").is_err());
        assert_eq!(
            refused_env(|v| v == "FIN_SELF_SANDBOX"),
            Some("FIN_SELF_SANDBOX")
        );
        assert_eq!(refused_env(|_| false), None);
    }
}
