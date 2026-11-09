//! The JSON shapes fin-node's `JobHost` takes and gives (5C-1 plan, Task 6), and their
//! conversions to and from the host's own types. They are exported through `fin-store`'s
//! `export_schema` ([`register_schemas`]) and generated into `@finisher/core-types`, so the
//! TypeScript side never mirrors them by hand. Inputs refuse unknown fields.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use fin_jobs::{FileMode, Hello, HelperKind, WorkerMsg};
use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::{
    CgroupMode, ExitInfo, ExitReason, GroupSpec, HostEvent, KillReason, MemoryRule, PathGrant,
    Profile, Program, Qos, Role, SpawnRequest, Timing, Watch,
};

/// An input field the host cannot use: `field` (dotted from the argument) and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireError {
    pub field: &'static str,
    pub error: String,
}

/// `JobHost.start`'s options. Timers in milliseconds; unset ones take the defaults.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobHostOptions {
    /// Where `{ bundled }` programs resolve: the directory of `workersBinaryPath()`.
    pub binary_dir: PathBuf,
    #[serde(default)]
    #[schemars(with = "JobHostTiming")]
    pub timing: Option<JobHostTiming>,
    /// Delegated cgroups where they are ours (Linux; default `auto`).
    #[serde(default)]
    #[schemars(with = "JobHostCgroups")]
    pub cgroups: Option<JobHostCgroups>,
    /// The Unix host watchdog (default on; ignored on Windows).
    #[serde(default)]
    #[schemars(with = "bool")]
    pub watchdog: Option<bool>,
    /// Events waiting for the JS thread before the host's readers wait too (default 1,024).
    #[serde(default)]
    #[schemars(with = "u32")]
    pub event_queue: Option<u32>,
    /// The summed JSON bytes of those events (default 16 MiB; one larger event still goes
    /// alone). `exit`, `spawned` and `groupEmpty` are not counted: a process has one of each.
    #[serde(default)]
    #[schemars(with = "u32")]
    pub event_queue_bytes: Option<u32>,
    /// `<server-data>`: the sandbox spawner places each process's grants against it (plan 7C
    /// Task 7). Without it, a profile that is enforced on this host refuses every spawn.
    #[serde(default)]
    #[schemars(with = "PathBuf")]
    pub server_data: Option<PathBuf>,
}

/// The host's timers in milliseconds; unset ones take the defaults.
#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobHostTiming {
    #[serde(default)]
    #[schemars(with = "u32")]
    pub hello_ms: Option<u32>,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub hung_ms: Option<u32>,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub kill_wait_ms: Option<u32>,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub rss_sample_ms: Option<u32>,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub exit_drain_ms: Option<u32>,
}

impl JobHostTiming {
    pub fn timing(&self) -> Timing {
        let d = Timing::default();
        Timing {
            hello_ms: self.hello_ms.unwrap_or(d.hello_ms),
            hung_ms: self.hung_ms.unwrap_or(d.hung_ms),
            kill_wait_ms: self.kill_wait_ms.unwrap_or(d.kill_wait_ms),
            rss_sample_ms: self.rss_sample_ms.unwrap_or(d.rss_sample_ms),
            exit_drain_ms: self.exit_drain_ms.unwrap_or(d.exit_drain_ms),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobHostCgroups {
    Auto,
    Off,
}

impl From<JobHostCgroups> for CgroupMode {
    fn from(c: JobHostCgroups) -> CgroupMode {
        match c {
            JobHostCgroups::Auto => CgroupMode::Auto,
            JobHostCgroups::Off => CgroupMode::Off,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobHostQos {
    #[default]
    Normal,
    Utility,
    Background,
}

impl From<JobHostQos> for Qos {
    fn from(q: JobHostQos) -> Qos {
        match q {
            JobHostQos::Normal => Qos::Normal,
            JobHostQos::Utility => Qos::Utility,
            JobHostQos::Background => Qos::Background,
        }
    }
}

/// `openGroup`'s spec.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobGroupSpec {
    pub label: String,
    /// The group's memory limit: the sum of its processes (Decision 20).
    #[serde(default)]
    #[schemars(with = "u64")]
    pub memory_bytes: Option<u64>,
    #[serde(default)]
    pub qos: JobHostQos,
}

impl From<JobGroupSpec> for GroupSpec {
    fn from(g: JobGroupSpec) -> GroupSpec {
        GroupSpec {
            label: g.label,
            memory_bytes: g.memory_bytes,
            qos: g.qos.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobSpawnRole {
    Worker,
    Helper,
}

/// A helper's `{kind, id}`.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobSpawnHelper {
    pub kind: HelperKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobSpawnProfile {
    #[default]
    Worker,
    Plugin,
}

/// `bundled`: `fin-workers`, `fin-analyze` or `fin-sandbox-exec` in `binaryDir`; `path`: plugin processes only.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum JobSpawnProgram {
    Bundled(String),
    Path(PathBuf),
}

/// A path the process may use (for a sandbox's rules).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobSpawnGrant {
    pub path: PathBuf,
    pub mode: FileMode,
}

/// One process to start. `env` is its whole environment: nothing else is inherited. `helper`
/// is required for a helper and refused for a worker.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobSpawnRequest {
    pub role: JobSpawnRole,
    #[serde(default)]
    #[schemars(with = "JobSpawnHelper")]
    pub helper: Option<JobSpawnHelper>,
    #[serde(default)]
    pub profile: JobSpawnProfile,
    pub family: String,
    pub program: JobSpawnProgram,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub grants: Vec<JobSpawnGrant>,
    #[serde(default)]
    pub qos: JobHostQos,
    #[serde(default)]
    pub label: String,
}

impl JobSpawnRequest {
    pub fn request(self) -> Result<SpawnRequest, WireError> {
        let role = match (self.role, self.helper) {
            (JobSpawnRole::Worker, None) => Role::Worker,
            (JobSpawnRole::Helper, Some(h)) => Role::Helper {
                kind: h.kind,
                id: h.id,
            },
            (JobSpawnRole::Worker, Some(_)) => return Err(helper("a worker has no helper")),
            (JobSpawnRole::Helper, None) => return Err(helper("a helper needs {kind, id}")),
        };
        Ok(SpawnRequest {
            role,
            profile: match self.profile {
                JobSpawnProfile::Worker => Profile::Worker,
                JobSpawnProfile::Plugin => Profile::Plugin,
            },
            family: self.family,
            program: match self.program {
                JobSpawnProgram::Bundled(name) => Program::Bundled(name),
                JobSpawnProgram::Path(p) => Program::Path(p),
            },
            args: self.args.into_iter().map(OsString::from).collect(),
            env: self
                .env
                .into_iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v)))
                .collect(),
            cwd: self.cwd,
            grants: self
                .grants
                .into_iter()
                .map(|g| PathGrant {
                    path: g.path,
                    mode: g.mode,
                })
                .collect(),
            qos: self.qos.into(),
            label: self.label,
        })
    }
}

fn helper(error: &str) -> WireError {
    WireError {
        field: "req.helper",
        error: error.into(),
    }
}

/// `watch`'s argument: `stallMs` without `begin`, `item` or `progress`; `timeoutMs` from now.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct JobWatch {
    pub job: String,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub stall_ms: Option<u32>,
    #[serde(default)]
    #[schemars(with = "u32")]
    pub timeout_ms: Option<u32>,
}

impl From<JobWatch> for Watch {
    fn from(w: JobWatch) -> Watch {
        Watch {
            job: w.job,
            stall_ms: w.stall_ms,
            timeout_ms: w.timeout_ms,
        }
    }
}

/// A group memory rule (Decision 20).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobMemoryRule {
    #[serde(rename = "fixedMiB", default)]
    #[schemars(with = "u64")]
    pub fixed_mib: Option<u64>,
    #[serde(rename = "ramFraction", default)]
    #[schemars(with = "f64")]
    pub ram_fraction: Option<f64>,
    #[serde(rename = "capMiB", default)]
    #[schemars(with = "u64")]
    pub cap_mib: Option<u64>,
}

impl From<JobMemoryRule> for MemoryRule {
    fn from(r: JobMemoryRule) -> MemoryRule {
        MemoryRule {
            fixed_mib: r.fixed_mib,
            ram_fraction: r.ram_fraction,
            cap_mib: r.cap_mib,
        }
    }
}

/// Why the host killed a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobHostKillReason {
    Cancel,
    Shutdown,
    Recycle,
    IdleExpired,
    HelperFailed,
    JobEnded,
    Hung,
    Test,
}

impl From<JobHostKillReason> for KillReason {
    fn from(k: JobHostKillReason) -> KillReason {
        match k {
            JobHostKillReason::Cancel => KillReason::Cancel,
            JobHostKillReason::Shutdown => KillReason::Shutdown,
            JobHostKillReason::Recycle => KillReason::Recycle,
            JobHostKillReason::IdleExpired => KillReason::IdleExpired,
            JobHostKillReason::HelperFailed => KillReason::HelperFailed,
            JobHostKillReason::JobEnded => KillReason::JobEnded,
            JobHostKillReason::Hung => KillReason::Hung,
            JobHostKillReason::Test => KillReason::Test,
        }
    }
}

impl From<KillReason> for JobHostKillReason {
    fn from(k: KillReason) -> JobHostKillReason {
        match k {
            KillReason::Cancel => JobHostKillReason::Cancel,
            KillReason::Shutdown => JobHostKillReason::Shutdown,
            KillReason::Recycle => JobHostKillReason::Recycle,
            KillReason::IdleExpired => JobHostKillReason::IdleExpired,
            KillReason::HelperFailed => JobHostKillReason::HelperFailed,
            KillReason::JobEnded => JobHostKillReason::JobEnded,
            KillReason::Hung => JobHostKillReason::Hung,
            KillReason::Test => JobHostKillReason::Test,
        }
    }
}

/// How a process ended, with the detail of `killed` (`kill`), `protocolError` and
/// `spawnFailed` (`what`) and `protocolMismatch` (`expected`, `got`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum JobHostExitReason {
    Exited,
    Killed { kill: JobHostKillReason },
    Hung,
    Stalled,
    TimedOut,
    Oom,
    ResourceFault,
    ProtocolError { what: String },
    ProtocolMismatch { expected: String, got: String },
    NoHello,
    SpawnFailed { what: String },
    Abandoned,
}

impl From<&ExitReason> for JobHostExitReason {
    fn from(r: &ExitReason) -> JobHostExitReason {
        match r.clone() {
            ExitReason::Exited => JobHostExitReason::Exited,
            ExitReason::Killed(k) => JobHostExitReason::Killed { kill: k.into() },
            ExitReason::Hung => JobHostExitReason::Hung,
            ExitReason::Stalled => JobHostExitReason::Stalled,
            ExitReason::TimedOut => JobHostExitReason::TimedOut,
            ExitReason::Oom => JobHostExitReason::Oom,
            ExitReason::ResourceFault => JobHostExitReason::ResourceFault,
            ExitReason::ProtocolError(what) => JobHostExitReason::ProtocolError { what },
            ExitReason::ProtocolMismatch { expected, got } => {
                JobHostExitReason::ProtocolMismatch { expected, got }
            }
            ExitReason::NoHello => JobHostExitReason::NoHello,
            ExitReason::SpawnFailed(what) => JobHostExitReason::SpawnFailed { what },
            ExitReason::Abandoned => JobHostExitReason::Abandoned,
        }
    }
}

/// An `exit` event's facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobHostExitInfo {
    #[serde(flatten)]
    pub reason: JobHostExitReason,
    /// The exit code (Windows: the NTSTATUS or code; a process the host killed: 137).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "i64")]
    pub code: Option<i64>,
    /// Lower case without `SIG` (fin-core's `SIGNALS`; `other` outside them). Never on Windows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub signal: Option<String>,
    /// Processes in the group the host did not start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub foreign: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub memory_peak: Option<u64>,
    /// The last 8 KiB of stderr.
    pub stderr_tail: String,
    pub lifetime_ms: u64,
}

impl From<&ExitInfo> for JobHostExitInfo {
    fn from(i: &ExitInfo) -> JobHostExitInfo {
        JobHostExitInfo {
            reason: (&i.reason).into(),
            code: i.code,
            signal: i.signal.map(str::to_owned),
            foreign: i.foreign,
            memory_peak: i.memory_peak,
            stderr_tail: i.stderr_tail.clone(),
            lifetime_ms: i.lifetime_ms,
        }
    }
}

/// What the host reports. Per process: `spawned` (its `spawn` has resolved), `hello`, its
/// messages and `stderr` lines, then `exit`; `groupEmpty` follows the last `exit` of a group.
/// `progress` is coalesced (at most every 250 ms, the newest value, and always before the
/// next other message); `alive` never arrives. `memory` (the worker group's bytes) comes with
/// `ready` only.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum JobHostEvent {
    Spawned {
        proc: u64,
        group: u64,
    },
    Hello {
        proc: u64,
        hello: Hello,
    },
    Message {
        proc: u64,
        msg: WorkerMsg,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        memory: Option<u64>,
    },
    Stderr {
        proc: u64,
        line: String,
    },
    Exit {
        proc: u64,
        info: JobHostExitInfo,
    },
    GroupEmpty {
        group: u64,
    },
    /// A process whose `exit` said `abandoned` has really exited now (Decision 18).
    AbandonedExit {
        proc: u64,
    },
}

impl From<HostEvent> for JobHostEvent {
    fn from(e: HostEvent) -> JobHostEvent {
        match e {
            HostEvent::Spawned { proc, group } => JobHostEvent::Spawned {
                proc: proc.0,
                group: group.0,
            },
            HostEvent::Hello { proc, hello } => JobHostEvent::Hello {
                proc: proc.0,
                hello,
            },
            HostEvent::Message { proc, msg, memory } => JobHostEvent::Message {
                proc: proc.0,
                msg,
                memory,
            },
            HostEvent::Stderr { proc, line } => JobHostEvent::Stderr { proc: proc.0, line },
            HostEvent::Exit { proc, info } => JobHostEvent::Exit {
                proc: proc.0,
                info: (&info).into(),
            },
            HostEvent::GroupEmpty { group } => JobHostEvent::GroupEmpty { group: group.0 },
            HostEvent::AbandonedExit { proc } => JobHostEvent::AbandonedExit { proc: proc.0 },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum JobHostKillTree {
    JobObject,
    Cgroup,
    ProcessGroup,
}

pub use crate::wire_caps::{JobHostCaps, JobHostProfileState, JobHostProfiles};

/// Adds every shape above to `g` (`export_schema`).
pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<JobHostOptions>();
    g.subschema_for::<JobGroupSpec>();
    g.subschema_for::<JobSpawnRequest>();
    g.subschema_for::<JobWatch>();
    g.subschema_for::<JobMemoryRule>();
    g.subschema_for::<JobHostKillReason>();
    g.subschema_for::<JobHostEvent>();
    g.subschema_for::<JobHostCaps>();
}

#[cfg(test)]
mod wire_tests {
    use super::*;

    #[test]
    fn exit_info_flattens_its_reason() {
        let info = JobHostExitInfo {
            reason: JobHostExitReason::Killed {
                kill: JobHostKillReason::IdleExpired,
            },
            code: None,
            signal: Some("kill".into()),
            foreign: None,
            memory_peak: Some(5),
            stderr_tail: String::new(),
            lifetime_ms: 7,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({ "reason": "killed", "kill": "idleExpired", "signal": "kill",
                "memoryPeak": 5, "stderrTail": "", "lifetimeMs": 7 })
        );
    }

    #[test]
    fn kill_reasons_round_trip() {
        for k in [
            KillReason::Cancel,
            KillReason::Shutdown,
            KillReason::Recycle,
            KillReason::IdleExpired,
            KillReason::HelperFailed,
            KillReason::JobEnded,
            KillReason::Hung,
            KillReason::Test,
        ] {
            assert_eq!(KillReason::from(JobHostKillReason::from(k)), k);
        }
    }
}
