//! The launcher (plan 7C Decision 3, Task 5): `fin-sandbox-exec --policy-fd <n> -- <argv…>`
//! confines a program `finisherd` starts itself (an agent harness, a hook command) and then
//! becomes it.
//!
//! The request arrives on the inherited descriptor `<n>` (never on argv, which every process of
//! the user can read, and never in the environment): a [`LaunchRequest`], the profile and its
//! typed input, from which [`LaunchRequest::policy`] builds the policy, so nothing outside this
//! crate assembles one (Decision 1). The launcher reads the descriptor to its end and closes it,
//! applies the policy (Linux: Landlock, then `RLIMIT_NOFILE` from the policy's limits; the
//! seccomp filter is deferred, plan Task 4) and `exec`s the program. Because it `exec`s, the
//! program keeps the launcher's process: its exit code and the signals sent to it are the
//! program's own, with nothing to relay.
//!
//! Exit codes of the launcher itself: [`EXIT_USAGE`] for a malformed command line or a request
//! offered anywhere but the descriptor; [`EXIT_UNAVAILABLE`] when the request is unreadable or
//! invalid, or the mechanism is enabled but the policy cannot be applied
//! (`SANDBOX_UNAVAILABLE`); 126/127 when the program cannot be started (refused, or missing).
//! Each writes one JSON line on stderr. Where the mechanism is disabled (F2), the program runs
//! unconfined: `finisherd` already reports that from `sandboxStatus()`.

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
/// The largest request read from the descriptor.
pub const MAX_REQUEST: u64 = 1 << 20;
/// Environment variables that would carry a policy or a sandbox request; the launcher refuses
/// to start while one is set, so a request can only come through the descriptor.
pub const REFUSED_ENV: &[&str] = &[
    "FIN_SANDBOX_POLICY",
    "FIN_SANDBOX_REQUEST",
    "FIN_SELF_SANDBOX",
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

    /// The policy, built by the profile's builder and validated.
    pub fn policy(&self) -> Result<SandboxPolicy, crate::PolicyError> {
        match self {
            LaunchRequest::Agent(i) => crate::profile::agent(i),
            LaunchRequest::Hook(i) => crate::profile::hook(i),
        }
    }
}

/// The parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Args {
    pub policy_fd: i32,
    pub argv: Vec<String>,
}

/// Parses `--policy-fd <n> -- <program> [args…]`. Nothing else is accepted before `--`, so a
/// policy cannot be passed on argv; `<n>` must be 3 or above (not stdio).
pub fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut fd = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
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
