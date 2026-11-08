//! The `library` job family's wire types (plan 5D Task 6; Decisions 26, 27, 33, 34, 36–38):
//! the params and results of `library.scan`, `library.probe` and `library.hash`, the one
//! line of a probe or hash job's `in/files.ndjson`, and the host calls the worker makes.
//! They live here, not in `fin-workers` (a binary), so `export_schema` can generate their
//! TypeScript and `validateJobParams` can decode them. Every params type refuses unknown
//! fields; results are read by the service and tolerate additions.
//!
//! **Granted files** (names as the host's `files` list gives them): `out/` (scan: the
//! batch files, written), `in/dirs.ndjson` (scan: one [`KnownDir`](crate::KnownDir) per line,
//! optional), `in/files.ndjson` (probe and hash: one [`FileLine`] per item).
//!
//! **Host calls:** [`CALL_BATCH`] `{file, seq}` after each batch file is written (the reply
//! means the indexer applied it), [`CALL_PROBED`] `{results}` (≤ [`PROBED_MAX`]
//! [`ProbeOutcome`]s) before the items they cover are acknowledged.

use std::collections::BTreeMap;

use fin_jobs::kinds::{KindParams, schema_as, validate_as};
use schemars::{JsonSchema, SchemaGenerator};
use serde::{Deserialize, Serialize};

use crate::exclude::ExcludeSpec;
use crate::hash::HashOutcome;
use crate::probe::ProbeOutcome;
use crate::raw::RawRel;
use crate::timing::Timing;
use crate::types::{Expect, VolumeIdentity};

/// The host call that hands over one written batch file (`out/<file>`).
pub const CALL_BATCH: &str = "library.batch";
/// The host call that reports up to [`PROBED_MAX`] probe outcomes.
pub const CALL_PROBED: &str = "library.probed";
/// Outcomes per `library.probed` call (Decision 34).
pub const PROBED_MAX: usize = 32;
/// `error` code of a job that found the root unusable (Decision 33); `retry: never`.
pub const LIBRARY_OFFLINE: &str = "LIBRARY_OFFLINE";
/// The grant of a scan job's output directory.
pub const OUT_DIR: &str = "out/";
/// The grant of a scan job's known directories.
pub const DIRS_FILE: &str = "in/dirs.ndjson";
/// The grant of a probe or hash job's files.
pub const FILES_FILE: &str = "in/files.ndjson";
/// The `code` of a file the library's kinds table does not list (Decision 37).
pub const KIND_FILTER: &str = "KIND_FILTER";

/// What a scan lists (`WalkMode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "LibraryScanMode")]
pub enum ScanMode {
    /// Every directory; resumable by `gen` and `in/dirs.ndjson` (Decision 27).
    Full,
    /// Exactly the directories in `dirs` (plus new subdirectories they report).
    Dirs,
    /// The known directories whose mtime changed, and new ones (Decision 28).
    Poll,
}

/// `library.scan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryScanParams")]
pub struct LibraryScanParams {
    pub library: String,
    /// The library's canonical root: an absolute directory.
    pub root: String,
    pub mode: ScanMode,
    /// The generation a full scan writes (`gen`; a word reserved in Rust 2024).
    #[serde(rename = "gen")]
    pub generation: u64,
    /// `mode: dirs`: the directories to list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dirs: Option<Vec<RawRel>>,
    pub excludes: ExcludeSpec,
    /// Read threads; absent means 64 (local roots), the service passes 32 for network and
    /// sync roots (Decision 9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<u32>")]
    pub threads: Option<u32>,
    /// The library's stored volume identity, which the first stat is judged against
    /// (Decision 17); absent for the first scan of a new library.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume: Option<VolumeIdentity>,
    /// Scaled deadlines. Test builds only: a release worker refuses the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timing: Option<Timing>,
}

/// What a scan did (`WalkSummary`, with the identity it measured).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "LibraryScanResult")]
pub struct LibraryScanResult {
    /// The identity the first stat measured (refreshed into the library after a full scan).
    pub identity: VolumeIdentity,
    pub listed: u64,
    pub incomplete: u64,
    pub timeouts: u64,
    pub unchanged: u64,
    pub skipped: u64,
    pub entries: u64,
    pub batches: u64,
    /// Read threads still stuck in an abandoned read when the walk ended.
    pub stuck: u64,
    /// The process's dataless policy, as the OS reports it (`off` on macOS, Decision 6).
    pub dataless_policy: String,
}

/// `library.probe` (Decision 34): items are catalog file ids (decimal strings); the
/// `in/files.ndjson` grant holds a [`FileLine`] for each, and only the lines named by `items`
/// run (the engine's `restrict` narrows `items`, not the file).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryProbeParams")]
pub struct LibraryProbeParams {
    pub library: String,
    pub root: String,
    /// The library's setting: a placeholder is read only when `true` (Decision 6).
    pub hydrate: bool,
    /// The library's bandwidth budget for this job's slot, in bytes per second (Decision 36).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<u64>")]
    pub read_bps: Option<u64>,
    /// The stored identity the root is judged against before and during the job.
    pub volume: VolumeIdentity,
    /// Extensions to probe (lower case, no dot); absent probes everything (`kinds: all`).
    /// Others are `skipped` with code [`KIND_FILTER`] (Decision 37).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extensions: Option<Vec<String>>,
    /// The grant that holds the lines.
    #[serde(default = "default_files")]
    pub files: String,
    pub items: Vec<String>,
}

/// `library.hash` (Decision 38): as [`LibraryProbeParams`], without the extension filter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryHashParams")]
pub struct LibraryHashParams {
    pub library: String,
    pub root: String,
    pub hydrate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<u64>")]
    pub read_bps: Option<u64>,
    pub volume: VolumeIdentity,
    #[serde(default = "default_files")]
    pub files: String,
    pub items: Vec<String>,
}

fn default_files() -> String {
    FILES_FILE.to_owned()
}

/// One line of `in/files.ndjson`: a catalogued file to read (Decision 20).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryFileLine")]
pub struct FileLine {
    pub id: i64,
    pub rel: RawRel,
    pub expect: Expect,
}

/// What `library.probe` reports: outcomes by state (the outcomes themselves went through
/// `library.probed`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "LibraryProbeResult")]
pub struct LibraryProbeResult {
    pub states: BTreeMap<String, u64>,
    /// Files whose outcome was `changed`: their directories need a rescan (Task 5).
    pub changed: u64,
    pub dataless_policy: String,
}

/// What `library.hash` reports: one value per item (`blake3:<hex>`) went through `item`;
/// here only the counts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[schemars(rename = "LibraryHashResult")]
pub struct LibraryHashResult {
    pub states: BTreeMap<String, u64>,
    pub changed: u64,
    pub dataless_policy: String,
}

/// The `library.batch` call's params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryBatchCall")]
pub struct BatchCall {
    /// A plain file name inside the job's `out/`.
    pub file: String,
    pub seq: u64,
}

/// The `library.probed` call's params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "LibraryProbedCall")]
pub struct ProbedCall {
    pub results: Vec<ProbeOutcome>,
}

/// Every library kind's params, at version 1.
pub static PARAMS: &[KindParams] = &[
    KindParams {
        kind: "library.scan",
        version: 1,
        validate: validate_as::<LibraryScanParams>,
        schema: schema_as::<LibraryScanParams>,
    },
    KindParams {
        kind: "library.probe",
        version: 1,
        validate: validate_as::<LibraryProbeParams>,
        schema: schema_as::<LibraryProbeParams>,
    },
    KindParams {
        kind: "library.hash",
        version: 1,
        validate: validate_as::<LibraryHashParams>,
        schema: schema_as::<LibraryHashParams>,
    },
];

/// Adds every library params, result and call type to `g` (`export_schema`).
pub fn register_schemas(g: &mut SchemaGenerator) {
    g.subschema_for::<LibraryScanParams>();
    g.subschema_for::<LibraryProbeParams>();
    g.subschema_for::<LibraryHashParams>();
    g.subschema_for::<LibraryScanResult>();
    g.subschema_for::<LibraryProbeResult>();
    g.subschema_for::<LibraryHashResult>();
    g.subschema_for::<FileLine>();
    g.subschema_for::<BatchCall>();
    g.subschema_for::<ProbedCall>();
    g.subschema_for::<ProbeOutcome>();
    g.subschema_for::<HashOutcome>();
    g.subschema_for::<ExcludeSpec>();
    g.subschema_for::<Timing>();
}
