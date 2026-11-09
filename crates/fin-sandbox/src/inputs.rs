//! The typed inputs of the four profiles (plan 7C Decision 1). They are schemars types,
//! exported to `@finisher/core-types`: `finisherd` fills them with canonical paths (links
//! resolved) and `fin-sandbox` builds the policy, so no policy type is written by hand in
//! TypeScript.

use std::path::PathBuf;

use fin_core::events::payloads::agent::{AgentMode, Harness};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::policy::DeviceRule;

/// How `fin` inside the sandbox reaches `finisherd` (spec §8.2–8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum FinEndpoint {
    /// Linux: the loopback agent listener `tcp:127.0.0.1:<port>` (spec §8.3).
    Tcp { port: u16 },
    /// macOS: `<server-data>/run/fin.sock` (sp5 §4.11).
    UnixSocket { path: PathBuf },
    /// Windows: the named pipe, reached through its DACL; nothing in the policy.
    Pipe,
}

/// A CLI agent session (spec §8.1, §8.5, §8.6; plan 7C Decision 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentInput {
    /// One path component (it names `workspace/<sessionId>/` and `agents/<sessionId>/`).
    pub session_id: String,
    pub mode: AgentMode,
    /// `claudeCode`, `codex` or `fake`; the built-in agent needs no sandbox (spec §8.9).
    pub harness: Harness,
    pub project_dir: PathBuf,
    /// `<projectDir>/workspace/<sessionId>`.
    pub workspace_dir: PathBuf,
    /// `<projectDir>/workspace/shared`; written only outside `plan` mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shared_dir: Option<PathBuf>,
    /// The roots of the libraries the project references (read-only).
    #[serde(default)]
    pub library_roots: Vec<PathBuf>,
    /// `<serverData>/agents/<sessionId>`.
    pub session_dir: PathBuf,
    /// The harness's sign-in store file, inside `<serverData>/providers/<harness>/home/`
    /// (spec §8.6): read and written in place, never its directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in_store: Option<PathBuf>,
    /// The base toolchain list plus the harness's own (files and directories, read-only).
    #[serde(default)]
    pub toolchains: Vec<PathBuf>,
    pub server_data: PathBuf,
    pub home: PathBuf,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fin: Option<FinEndpoint>,
}

/// A hook command (spec §8.10; plan 7C Decision 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookInput {
    /// Read-only.
    pub project_dir: PathBuf,
    /// The hook's own file.
    pub hook_file: PathBuf,
    #[serde(default)]
    pub toolchains: Vec<PathBuf>,
    /// A private temporary directory, the only write root.
    pub tmp: PathBuf,
    pub server_data: PathBuf,
    pub home: PathBuf,
    /// Present only when the hook declares `fin`: its credential file and the endpoint. Without
    /// it the hook has no network and no credential.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fin: Option<HookFin>,
    /// The hook's time limit (`Limits.wall_ms`): the launcher ends the whole tree when it passes
    /// (on Windows also the backstop for a Node process stuck on piped stdio, Decision 25).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HookFin {
    /// The `plan`-scoped credential 7F mints for the call (read-only).
    pub credential: PathBuf,
    pub endpoint: FinEndpoint,
}

/// A media worker (spec §8.10; sp6 §3.5; plan 7C Decisions 8 and 19).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerInput {
    /// A private temporary directory (read and write).
    pub tmp: PathBuf,
    /// The font and media caches (read-only).
    #[serde(default)]
    pub caches: Vec<PathBuf>,
    /// GPU device nodes and driver files (sub-project 6 adds them, 6B-1).
    #[serde(default)]
    pub devices: Vec<DeviceRule>,
    pub server_data: PathBuf,
    pub scope: WorkerScope,
}

/// What one worker serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind", deny_unknown_fields)]
pub enum WorkerScope {
    /// One job: its `in/` (read), its `out/` (write) and the manifest's source files.
    #[serde(rename_all = "camelCase")]
    Cold {
        /// `<serverData>/jobs/<jobId>`.
        job_dir: PathBuf,
        /// The manifest's sources, per file where the OS allows it (read-only).
        #[serde(default)]
        sources: Vec<PathBuf>,
    },
    /// A warm worker: its own directory (each job it serves is created under
    /// `<workerDir>/jobs/<jobId>/`, Decision 19), never the jobs root, and one project's
    /// sources and library roots (owner decision 2026-11-08: one warm worker per project).
    #[serde(rename_all = "camelCase")]
    Warm {
        /// `<serverData>/jobs/<name>`.
        worker_dir: PathBuf,
        /// The project's `sources/` and the library roots it references (read-only).
        #[serde(default)]
        library_roots: Vec<PathBuf>,
    },
}

/// A code plugin's process (sp8 §16.1, plan 7C Decisions 9 and 21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginInput {
    /// The plugin's package directory inside `<serverData>/plugins/store/` (read-only).
    pub store_dir: PathBuf,
    /// The runtime: finisherd's Node or the Electron framework, for `node` plugins.
    #[serde(default)]
    pub runtime: Vec<PathBuf>,
    /// System libraries and frameworks (`/usr/lib`, `/lib`, `/System/Library`, `System32`).
    #[serde(default)]
    pub system: Vec<PathBuf>,
    /// ONNX Runtime's library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub onnx_runtime: Option<PathBuf>,
    /// The model files the manifest names.
    #[serde(default)]
    pub models: Vec<PathBuf>,
    /// The call's or job's directory: `in/` is read-only, `out/` writable.
    pub call_dir: PathBuf,
    /// A private temporary directory.
    pub tmp: PathBuf,
    /// The per-project plugin data directory, when the manifest is granted `fs pluginData`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin_data: Option<PluginData>,
    /// GPU device nodes; empty without the `gpu` grant. Never `/dev/dri/card*`.
    #[serde(default)]
    pub gpu: Vec<DeviceRule>,
    /// The manifest's declared memory need (default 2 GiB, at most 16 GiB; sp8 §3.3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    pub server_data: PathBuf,
    pub home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginData {
    pub dir: PathBuf,
    /// `fs pluginData readWrite` (otherwise read-only).
    pub write: bool,
}
