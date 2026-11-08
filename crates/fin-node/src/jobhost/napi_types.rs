//! The JSON shapes `JobHost` takes and gives (`packages/fin-node/index.d.ts`), turned into
//! and out of `fin-jobhost`'s types. Inputs refuse unknown fields.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use fin_core::CoreError;
use fin_jobhost::{
    CgroupMode, ExitInfo, ExitReason, GroupSpec, HostCaps, HostEvent, KillReason, PathGrant,
    Profile, Program, Qos, Role, SpawnRequest, Timing, Watch,
};
use fin_jobs::{FileMode, HelperKind};
use napi_derive::napi;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};

/// Decodes `json` as `T`, or `INVALID_REQUEST {field, error}`.
pub(crate) fn decode<T: DeserializeOwned>(field: &str, json: &str) -> Result<T, CoreError> {
    serde_json::from_str(json).map_err(|e| invalid(field, e.to_string()))
}

pub(crate) fn invalid(field: &str, error: String) -> CoreError {
    CoreError::new("INVALID_REQUEST", format!("{field}: {error}"))
        .with_details(json!({ "field": field, "error": error }))
}

/// `JobHostOptions`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Options {
    pub binary_dir: PathBuf,
    #[serde(default)]
    pub timing: TimingIn,
    #[serde(default)]
    pub cgroups: Option<CgroupsIn>,
    #[serde(default)]
    pub watchdog: Option<bool>,
    #[serde(default)]
    pub event_queue: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TimingIn {
    hello_ms: Option<u32>,
    hung_ms: Option<u32>,
    kill_wait_ms: Option<u32>,
    rss_sample_ms: Option<u32>,
    exit_drain_ms: Option<u32>,
}

impl TimingIn {
    pub(crate) fn timing(&self) -> Timing {
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

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub(crate) enum CgroupsIn {
    Auto,
    Off,
}

impl CgroupsIn {
    pub(crate) fn mode(self) -> CgroupMode {
        match self {
            CgroupsIn::Auto => CgroupMode::Auto,
            CgroupsIn::Off => CgroupMode::Off,
        }
    }
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) enum QosIn {
    #[default]
    Normal,
    Utility,
    Background,
}

impl From<QosIn> for Qos {
    fn from(q: QosIn) -> Qos {
        match q {
            QosIn::Normal => Qos::Normal,
            QosIn::Utility => Qos::Utility,
            QosIn::Background => Qos::Background,
        }
    }
}

/// `openGroup`'s spec.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct GroupIn {
    label: String,
    #[serde(default)]
    memory_bytes: Option<u64>,
    #[serde(default)]
    qos: QosIn,
}

impl From<GroupIn> for GroupSpec {
    fn from(g: GroupIn) -> GroupSpec {
        GroupSpec {
            label: g.label,
            memory_bytes: g.memory_bytes,
            qos: g.qos.into(),
        }
    }
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum RoleIn {
    Worker,
    Helper,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HelperIn {
    kind: HelperKind,
    id: String,
}

#[derive(Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
enum ProfileIn {
    #[default]
    Worker,
    Plugin,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
enum ProgramIn {
    Bundled(String),
    Path(PathBuf),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GrantIn {
    path: PathBuf,
    mode: FileMode,
}

/// `JobSpawnRequest`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SpawnIn {
    role: RoleIn,
    #[serde(default)]
    helper: Option<HelperIn>,
    #[serde(default)]
    profile: ProfileIn,
    family: String,
    program: ProgramIn,
    #[serde(default)]
    args: Vec<String>,
    /// The whole environment: nothing else is inherited.
    #[serde(default)]
    env: BTreeMap<String, String>,
    cwd: PathBuf,
    #[serde(default)]
    grants: Vec<GrantIn>,
    #[serde(default)]
    qos: QosIn,
    #[serde(default)]
    label: String,
}

impl SpawnIn {
    pub(crate) fn request(self) -> Result<SpawnRequest, CoreError> {
        let role = match (self.role, self.helper) {
            (RoleIn::Worker, None) => Role::Worker,
            (RoleIn::Helper, Some(h)) => Role::Helper {
                kind: h.kind,
                id: h.id,
            },
            (RoleIn::Worker, Some(_)) => {
                return Err(invalid("req.helper", "a worker has no helper".into()));
            }
            (RoleIn::Helper, None) => {
                return Err(invalid("req.helper", "a helper needs {kind, id}".into()));
            }
        };
        Ok(SpawnRequest {
            role,
            profile: match self.profile {
                ProfileIn::Worker => Profile::Worker,
                ProfileIn::Plugin => Profile::Plugin,
            },
            family: self.family,
            program: match self.program {
                ProgramIn::Bundled(name) => Program::Bundled(name),
                ProgramIn::Path(p) => Program::Path(p),
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

/// `watch`'s argument.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WatchIn {
    job: String,
    #[serde(default)]
    stall_ms: Option<u32>,
    #[serde(default)]
    timeout_ms: Option<u32>,
}

impl From<WatchIn> for Watch {
    fn from(w: WatchIn) -> Watch {
        Watch {
            job: w.job,
            stall_ms: w.stall_ms,
            timeout_ms: w.timeout_ms,
        }
    }
}

const KILL_REASONS: &[(&str, KillReason)] = &[
    ("cancel", KillReason::Cancel),
    ("shutdown", KillReason::Shutdown),
    ("recycle", KillReason::Recycle),
    ("idleExpired", KillReason::IdleExpired),
    ("helperFailed", KillReason::HelperFailed),
    ("jobEnded", KillReason::JobEnded),
    ("hung", KillReason::Hung),
    ("test", KillReason::Test),
];

pub(crate) fn kill_reason(name: &str) -> Result<KillReason, CoreError> {
    KILL_REASONS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, r)| *r)
        .ok_or_else(|| invalid("reason", format!("unknown kill reason {name:?}")))
}

fn kill_name(reason: KillReason) -> &'static str {
    KILL_REASONS
        .iter()
        .find(|(_, r)| *r == reason)
        .map_or("test", |(n, _)| n)
}

/// `caps()`.
#[napi(object)]
pub struct JobHostCaps {
    #[napi(ts_type = "\"jobObject\" | \"cgroup\" | \"processGroup\"")]
    pub kill_tree: String,
    pub watchdog: bool,
    pub cgroup_memory: bool,
    pub leaf_limits: bool,
    pub sandbox: String,
    pub sandboxed: bool,
}

impl From<HostCaps> for JobHostCaps {
    fn from(c: HostCaps) -> JobHostCaps {
        JobHostCaps {
            kill_tree: c.kill_tree.as_str().to_owned(),
            watchdog: c.watchdog,
            cgroup_memory: c.cgroup_memory,
            leaf_limits: c.leaf_limits,
            sandbox: c.sandbox.to_owned(),
            sandboxed: c.sandboxed,
        }
    }
}

/// `ExitInfo` as JS sees it: `reason` plus the detail of `killed` (`kill`), `protocolError`
/// and `spawnFailed` (`what`) and `protocolMismatch` (`expected`, `got`).
fn exit_json(info: &ExitInfo) -> Value {
    let mut o = Map::new();
    let reason = match &info.reason {
        ExitReason::Exited => "exited",
        ExitReason::Killed(k) => {
            o.insert("kill".into(), kill_name(*k).into());
            "killed"
        }
        ExitReason::Hung => "hung",
        ExitReason::Stalled => "stalled",
        ExitReason::TimedOut => "timedOut",
        ExitReason::Oom => "oom",
        ExitReason::ResourceFault => "resourceFault",
        ExitReason::ProtocolError(what) => {
            o.insert("what".into(), what.clone().into());
            "protocolError"
        }
        ExitReason::ProtocolMismatch { expected, got } => {
            o.insert("expected".into(), expected.clone().into());
            o.insert("got".into(), got.clone().into());
            "protocolMismatch"
        }
        ExitReason::NoHello => "noHello",
        ExitReason::SpawnFailed(what) => {
            o.insert("what".into(), what.clone().into());
            "spawnFailed"
        }
        ExitReason::Abandoned => "abandoned",
    };
    o.insert("reason".into(), reason.into());
    if let Some(c) = info.code {
        o.insert("code".into(), c.into());
    }
    if let Some(s) = info.signal {
        o.insert("signal".into(), s.into());
    }
    if let Some(f) = info.foreign {
        o.insert("foreign".into(), f.into());
    }
    if let Some(m) = info.memory_peak {
        o.insert("memoryPeak".into(), m.into());
    }
    o.insert("stderrTail".into(), info.stderr_tail.clone().into());
    o.insert("lifetimeMs".into(), info.lifetime_ms.into());
    Value::Object(o)
}

fn to_value(v: &impl serde::Serialize) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// A `JobHostEvent` as JSON.
pub(crate) fn event_json(e: &HostEvent) -> String {
    let v = match e {
        HostEvent::Hello { proc, hello } => {
            json!({ "type": "hello", "proc": proc.0, "hello": to_value(hello) })
        }
        HostEvent::Message { proc, msg, memory } => {
            let mut v = json!({ "type": "message", "proc": proc.0, "msg": to_value(msg) });
            if let Some(m) = memory {
                v["memory"] = (*m).into();
            }
            v
        }
        HostEvent::Stderr { proc, line } => {
            json!({ "type": "stderr", "proc": proc.0, "line": line })
        }
        HostEvent::Exit { proc, info } => {
            json!({ "type": "exit", "proc": proc.0, "info": exit_json(info) })
        }
        HostEvent::GroupEmpty { group } => json!({ "type": "groupEmpty", "group": group.0 }),
    };
    v.to_string()
}
