//! The `test` family's params and results (feature `test-hooks`; 5C-1 plan, Task 2). Rust is
//! their source: `validateJobParams` decodes them, and the TypeScript types are generated
//! (schema names prefixed `Test`). Every params type refuses unknown fields.

use std::collections::BTreeMap;

use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::kinds::{KindParams, schema_as, validate_as};
use crate::msg_host::FileMode;

macro_rules! params {
    ($($(#[$m:meta])* $name:ident as $schema:literal { $($body:tt)* })*) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[schemars(rename = $schema)]
        pub struct $name { $($body)* }
    )*};
}

macro_rules! result {
    ($($(#[$m:meta])* $name:ident as $schema:literal { $($body:tt)* })*) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase")]
        #[schemars(rename = $schema)]
        pub struct $name { $($body)* }
    )*};
}

macro_rules! choice {
    ($($(#[$m:meta])* $name:ident as $schema:literal { $($v:ident),* $(,)? })*) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
        #[serde(rename_all = "camelCase")]
        #[schemars(rename = $schema)]
        pub enum $name { #[default] $($v),* }
    )*};
}

choice! {
    /// What `test.echo` reports about its own process.
    EchoReport as "TestEchoReport" { Pid, Env, Args, Fds, Cwd, Pdeathsig, Nice, Cgroup }
    /// How `test.crash` dies: `abort`, `SIGSEGV`/an access violation, `SIGBUS`/
    /// `EXCEPTION_IN_PAGE_ERROR`, Windows `__fastfail`, `exit(3)`, or by allocating until its
    /// group's memory limit kills it (`oom`; it aborts past 2 GiB).
    CrashHow as "TestCrashHow" { Abort, Segv, Bus, Fastfail, Exit, Oom }
    /// `test.spawn`: the grandchild stays in the group or leaves it with `setsid`.
    Detach as "TestDetach" { None, Setsid }
    /// `test.garbage`: the protocol violation to commit.
    Garbage as "TestGarbage" { Json, Seq, Long, OtherJob, UnknownType }
    /// The test helper's behaviour (`fin-workers --helper test:<behaviour>`).
    HelperBehaviour as "TestHelperBehaviour" { Ok, ExitEarly, Hang, Spawn, Call }
}

params! {
    /// `test.sleep`: sleeps `ms` (checking cancel), with optional progress.
    SleepParams as "TestSleepParams" {
        pub ms: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub progress_every_ms: Option<u64>,
    }
    /// `test.echo`: returns `value`, plus what `report` asks about the process.
    EchoParams as "TestEchoParams" {
        pub value: Value,
        #[serde(default)]
        pub report: Vec<EchoReport>,
    }
    /// `test.progressFlood`: `n` progress messages as fast as it can; with `stderrBytes`, `n`
    /// stderr lines of that many bytes (`flood <i> ` then padding) instead.
    ProgressFloodParams as "TestProgressFloodParams" {
        pub n: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u32")]
        pub stderr_bytes: Option<u32>,
    }
    /// `test.warm` (warm): returns `value` with the pid and how many jobs the process served.
    WarmParams as "TestWarmParams" {
        pub value: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub delay_ms: Option<u64>,
        /// A job-directory file kept open past `ready` (breaks the warm contract on purpose).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub hold_file: Option<String>,
        /// Paths the worker tries to read (plan 7C Task 12): each answered with the errno of
        /// the attempt, `null` when it read.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub probe: Vec<String>,
    }
    /// `test.crash` (batch): `begin`/`item` per item; crashes inside a poison item (before
    /// its `begin` with `between`), or inside the second item of any run of two or more with
    /// `crashOnSecond`.
    CrashParams as "TestCrashParams" {
        pub items: Vec<String>,
        #[serde(default)]
        pub poison: Vec<String>,
        pub how: CrashHow,
        #[serde(default)]
        pub between: bool,
        #[serde(default)]
        pub crash_on_second: bool,
        /// Milliseconds per item.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub item_ms: Option<u64>,
    }
    /// `test.fail`: answers `error {code, retry}` on every attempt up to `failUntil` (every
    /// attempt without it), then succeeds with the attempt number.
    FailParams as "TestFailParams" {
        pub code: String,
        #[serde(default)]
        pub retry: crate::Retry,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u32")]
        pub fail_until: Option<u32>,
    }
    /// `test.hang`: stops `alive`, stops reading stdin (EOF unseen, the pipe fills) and blocks.
    HangParams as "TestHangParams" {}
    /// `test.stall`: keeps `alive`, never reports progress, ends at cancel.
    StallParams as "TestStallParams" {}
    /// `test.spawn` (breaks the no-process rule on purpose): double-forks a sleeping
    /// grandchild, writes the pids to the mapped file `out`, then sleeps.
    SpawnParams as "TestSpawnParams" {
        pub detach: Detach,
        pub out: String,
        /// How long the grandchild sleeps (default 600 s).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub grandchild_ms: Option<u64>,
    }
    /// `test.files`: writes a pattern into every `write` file, reads every `read` file, and
    /// leaves a file in each `write` directory; fails (`retry: backoff`) on attempt
    /// `failOnAttempt`.
    FilesParams as "TestFilesParams" {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u32")]
        pub fail_on_attempt: Option<u32>,
    }
    /// `test.checkpoint` (resumable): a checkpoint per step; crashes on attempt 1 at
    /// `crashAt`.
    CheckpointParams as "TestCheckpointParams" {
        pub steps: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u32")]
        pub crash_at: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub step_ms: Option<u64>,
    }
    /// `test.oom`: allocates and touches `mib` MiB, maps `mapFile` (if given) read-only and
    /// reads every page of it, writes `1` to `readyFile` (if given), then holds.
    OomParams as "TestOomParams" {
        pub mib: u64,
        /// Hold the memory this long before answering.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub hold_ms: Option<u64>,
        /// A file (mapped name, or under the job directory) to map: file-backed pages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub map_file: Option<String>,
        /// A file to write once the memory is held.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub ready_file: Option<String>,
    }
    /// `test.call`: one host call; returns the reply.
    CallParams as "TestCallParams" {
        pub method: String,
        #[serde(default)]
        pub params: Value,
    }
    /// `test.diag`: one `diag` of kind `test.diag`.
    DiagParams as "TestDiagParams" {}
    /// `test.garbage`: commits the protocol violation `what`, then waits to be killed.
    GarbageParams as "TestGarbageParams" { pub what: Garbage }
    /// `test.exitAfterResult`: writes `result` and exits 0 at once.
    ExitAfterResultParams as "TestExitAfterResultParams" {}
    /// `test.helper`: the job of a test helper (`helper` picks its behaviour).
    HelperParams as "TestHelperParams" {
        #[serde(default)]
        pub helper: HelperBehaviour,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "u64")]
        pub delay_ms: Option<u64>,
    }
    /// `test.badHello` (family `testbad`): its `hello` claims another envelope.
    BadHelloParams as "TestBadHelloParams" {}
    /// `test.pluginMinor` (family `testplugin`): `hello` with minor 1 and no envelope, then a
    /// message of an unknown type with an `id`. `key` is its warm key.
    PluginMinorParams as "TestPluginMinorParams" {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub key: Option<String>,
    }
}

result! {
    EchoResult as "TestEchoResult" {
        pub value: Value,
        pub pid: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "BTreeMap<String, String>")]
        pub env: Option<BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Vec<String>")]
        pub args: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Vec<u32>")]
        pub fds: Option<Vec<u32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub cwd: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "i32")]
        pub pdeathsig: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "i32")]
        pub nice: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub cgroup: Option<String>,
    }
    SleepResult as "TestSleepResult" { pub slept_ms: u64 }
    CountResult as "TestCountResult" { pub sent: u64 }
    WarmResult as "TestWarmResult" {
        pub value: Value,
        pub pid: u32,
        pub served: u64,
        /// One errno (or `null`) per `probe` path, in order.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub probes: Vec<Option<i32>>,
    }
    CrashResult as "TestCrashResult" { pub done: Vec<String> }
    /// What `test.spawn` writes to its `out` file (JSON, then zero padding).
    SpawnPids as "TestSpawnPids" { pub worker: u32, pub mid: u32, pub grandchild: u32 }
    FileReport as "TestFileReport" {
        pub name: String,
        pub mode: FileMode,
        pub bytes: u64,
        /// FNV-1a 64 of the content after the run, as hex.
        pub hash: String,
    }
    FilesResult as "TestFilesResult" {
        pub files: Vec<FileReport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "bool")]
        pub out_empty: Option<bool>,
    }
    CheckpointResult as "TestCheckpointResult" { pub steps: u32, pub resumed_from: u32 }
    OomResult as "TestOomResult" { pub mib: u64 }
    FailResult as "TestFailResult" { pub attempt: u32 }
    DiagResult as "TestDiagResult" { pub sent: bool }
    HelperResult as "TestHelperResult" { pub helpers: Vec<String> }
    PluginMinorResult as "TestPluginMinorResult" {
        pub reply: Value,
        pub pid: u32,
        pub served: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "String")]
        pub key: Option<String>,
    }
}

macro_rules! table {
    ($($kind:literal => $ty:ty),* $(,)?) => {
        /// Every test kind's params, at version 1.
        pub static PARAMS: &[KindParams] = &[$(KindParams {
            kind: $kind,
            version: 1,
            validate: validate_as::<$ty>,
            schema: schema_as::<$ty>,
        }),*];

        fn register_params(g: &mut SchemaGenerator) {
            $(g.subschema_for::<$ty>();)*
        }
    };
}

table! {
    "test.sleep" => SleepParams,
    "test.echo" => EchoParams,
    "test.progressFlood" => ProgressFloodParams,
    "test.warm" => WarmParams,
    "test.crash" => CrashParams,
    "test.hang" => HangParams,
    "test.stall" => StallParams,
    "test.spawn" => SpawnParams,
    "test.files" => FilesParams,
    "test.checkpoint" => CheckpointParams,
    "test.oom" => OomParams,
    "test.call" => CallParams,
    "test.diag" => DiagParams,
    "test.garbage" => GarbageParams,
    "test.exitAfterResult" => ExitAfterResultParams,
    "test.helper" => HelperParams,
    "test.badHello" => BadHelloParams,
    "test.pluginMinor" => PluginMinorParams,
    "test.fail" => FailParams,
}

/// Adds every test params and result type to `g` (`export_schema`).
pub fn register_schemas(g: &mut SchemaGenerator) {
    register_params(g);
    g.subschema_for::<EchoResult>();
    g.subschema_for::<SleepResult>();
    g.subschema_for::<CountResult>();
    g.subschema_for::<WarmResult>();
    g.subschema_for::<CrashResult>();
    g.subschema_for::<SpawnPids>();
    g.subschema_for::<FilesResult>();
    g.subschema_for::<CheckpointResult>();
    g.subschema_for::<OomResult>();
    g.subschema_for::<FailResult>();
    g.subschema_for::<DiagResult>();
    g.subschema_for::<HelperResult>();
    g.subschema_for::<PluginMinorResult>();
}
