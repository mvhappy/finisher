//! Mechanism status (plan 7C Decisions 5, 12 and 23): what each profile gets on this host.
//!
//! - [`mechanism`] is the OS mechanism alone. Linux: the Landlock version query is the only
//!   source (`EPERM`, `ENOSYS`, `EOPNOTSUPP` → `Disabled {landlockDisabled}`, F2: run, warn;
//!   ABI < 3 → `Unavailable {need: "kernel ≥ 6.2"}`). macOS and Windows have no mechanism in
//!   this crate yet, so they answer `Disabled {profileNotReady}`.
//! - [`status`] adds [`READY`]: a profile whose escape suite has not passed on this OS runs as
//!   before 7C (`Disabled {profileNotReady}`), unless the mechanism itself is disabled, which
//!   is the more useful warning (its fix is a boot parameter). Every profile is treated alike
//!   (plan 7C Task 7): the job host's `SandboxSpawner` is the only requester of the `worker`
//!   profile and applies it only once it is ready, so `status` and the spawner's
//!   `caps().profiles` agree.
//!
//! `Disabled` runs unconfined with persistent warnings; `Unavailable` refuses the start
//! (`SANDBOX_UNAVAILABLE {profile, os, need}`). There is no owner opt-out (owner decision
//! 2026-11-08).
//!
//! Test hook (`test-hooks` builds only): `FIN_TEST_SANDBOX_STATUS=disabled|unavailable|enforced`
//! replaces [`mechanism`]'s answer, and with it what [`crate::self_apply`] does.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub use fin_core::events::payloads::UnconfinedReason as DisabledReason;
use fin_core::events::payloads::{SandboxOs, SandboxProfileName};

use crate::policy::{Os, Profile};

/// What a profile gets on this host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[schemars(rename = "SandboxMechanism")]
pub enum Mechanism {
    /// Applied, with what (`landlock abi 8`).
    Enforced { detail: String },
    /// Runs unconfined, with warnings (F2).
    Disabled { reason: DisabledReason },
    /// The start is refused: the host lacks `need`.
    Unavailable { need: String },
}

/// Every profile's status on this host (`sandboxStatus()` in fin-node).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SandboxStatus {
    pub os: SandboxOs,
    pub agent: Mechanism,
    pub hook: Mechanism,
    pub worker: Mechanism,
    pub plugin: Mechanism,
}

/// Whether `fin-sandbox-exec` can hide Unix sockets from `agent` and `hook` processes in a user
/// and mount namespace on this host (plan 7C Decision 26). Only the launcher binary can tell
/// (an AppArmor profile attaches to it by path), so `finisherd` asks it: `--probe`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[schemars(rename = "SandboxNamespaces")]
pub enum Namespaces {
    Available,
    /// Today's behaviour (Landlock alone), with `SANDBOX_SOCKETS_EXPOSED`.
    Unavailable {
        reason: String,
    },
    /// Not Linux.
    NotApplicable,
}

/// What `fin-sandbox-exec --probe` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "SandboxLauncherProbe")]
pub struct LauncherProbe {
    pub namespaces: Namespaces,
}

/// The profiles that are the default on an OS, because their escape suite passes there
/// (Decision 5; Task 16 adds each row as the suites pass on the validation hosts). Empty until
/// then nothing is applied by default; seccomp (Task 4) completes Linux's profiles.
pub const READY: &[(Profile, Os)] = &[];

/// The variable of the test hook (`test-hooks` builds only).
pub const TEST_ENV: &str = "FIN_TEST_SANDBOX_STATUS";
/// The test hook that adds rows to [`READY`] for this OS (`test-hooks` builds only):
/// `FIN_TEST_SANDBOX_READY=worker,plugin`.
pub const TEST_READY_ENV: &str = "FIN_TEST_SANDBOX_READY";

/// [`READY`], with the test hook's rows in `test-hooks` builds.
pub fn ready() -> Vec<(Profile, Os)> {
    #[allow(unused_mut)]
    let mut rows = READY.to_vec();
    #[cfg(feature = "test-hooks")]
    if let Ok(v) = std::env::var(TEST_READY_ENV) {
        for name in v.split(',') {
            let p = match name.trim() {
                "agent" => Profile::Agent,
                "hook" => Profile::Hook,
                "worker" => Profile::Worker,
                "plugin" => Profile::Plugin,
                _ => continue,
            };
            rows.push((p, Os::current()));
        }
    }
    rows
}

/// The OS mechanism on this host, whatever the profile.
pub fn mechanism() -> Mechanism {
    #[cfg(feature = "test-hooks")]
    if let Some(m) = stubbed() {
        return m;
    }
    os_mechanism()
}

#[cfg(feature = "test-hooks")]
fn stubbed() -> Option<Mechanism> {
    match std::env::var(TEST_ENV).ok()?.as_str() {
        "disabled" => Some(Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        }),
        "unavailable" => Some(Mechanism::Unavailable {
            need: format!("{} (stubbed)", NEED),
        }),
        "enforced" => Some(Mechanism::Enforced {
            detail: "stubbed".into(),
        }),
        _ => None,
    }
}

#[cfg(target_os = "linux")]
const NEED: &str = crate::linux::landlock::NEED;
// Only the test stub names a need off Linux.
#[cfg(all(not(target_os = "linux"), feature = "test-hooks"))]
const NEED: &str = "an OS sandbox this build implements";

#[cfg(target_os = "linux")]
fn os_mechanism() -> Mechanism {
    use crate::linux::landlock::{MIN_ABI, Support, query};
    match query() {
        Support::Abi(a) if a.0 >= MIN_ABI => Mechanism::Enforced {
            detail: format!("landlock abi {}", a.0),
        },
        Support::Abi(_) => Mechanism::Unavailable { need: NEED.into() },
        Support::Disabled { .. } => Mechanism::Disabled {
            reason: DisabledReason::LandlockDisabled,
        },
        Support::Unknown { ret, errno } => Mechanism::Unavailable {
            need: format!("a Landlock version query that answers (returned {ret}, errno {errno})"),
        },
    }
}

#[cfg(not(target_os = "linux"))]
fn os_mechanism() -> Mechanism {
    // Seatbelt (Task 8) and AppContainer (Task 9) are not built yet.
    Mechanism::Disabled {
        reason: DisabledReason::ProfileNotReady,
    }
}

/// What `profile` gets on this host.
pub fn status(profile: Profile) -> Mechanism {
    status_with(profile, mechanism(), &ready())
}

/// [`status`] for a given mechanism and table (pure, for tests).
pub fn status_with(profile: Profile, mechanism: Mechanism, ready: &[(Profile, Os)]) -> Mechanism {
    match mechanism {
        m @ Mechanism::Disabled { .. } => m,
        _ if !ready.contains(&(profile, Os::current())) => Mechanism::Disabled {
            reason: DisabledReason::ProfileNotReady,
        },
        m => m,
    }
}

/// Every profile's status.
pub fn all() -> SandboxStatus {
    let m = mechanism();
    let ready = ready();
    let s = |p| status_with(p, m.clone(), &ready);
    SandboxStatus {
        os: Os::current().into(),
        agent: s(Profile::Agent),
        hook: s(Profile::Hook),
        worker: s(Profile::Worker),
        plugin: s(Profile::Plugin),
    }
}

impl From<Profile> for SandboxProfileName {
    fn from(p: Profile) -> Self {
        match p {
            Profile::Agent => SandboxProfileName::Agent,
            Profile::Hook => SandboxProfileName::Hook,
            Profile::Worker => SandboxProfileName::Worker,
            Profile::Plugin => SandboxProfileName::Plugin,
        }
    }
}

impl From<Os> for SandboxOs {
    fn from(o: Os) -> Self {
        match o {
            Os::Linux => SandboxOs::Linux,
            Os::Macos => SandboxOs::Macos,
            Os::Windows => SandboxOs::Windows,
        }
    }
}
