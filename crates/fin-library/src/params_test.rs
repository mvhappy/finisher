//! `test.stage` params (feature `test-hooks`; plan 5D Task 11): the stage framework's test
//! kind in the `library` family. Its items are fingerprints, each naming one file of `files`
//! (the `StageFile`'s raw chain and `Expect`), opened only through
//! [`open_for_read`](crate::open_for_read). The other fields are knobs for the tests.

use std::collections::BTreeMap;

use fin_jobs::kinds::{KindParams, schema_as, validate_as};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::raw::RawRel;
use crate::types::Expect;

/// One file of a `test.stage` job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestStageFile {
    pub fp: String,
    pub file_id: i64,
    pub rel: RawRel,
    pub expect: Expect,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TestStageParams {
    pub library: String,
    pub root: String,
    #[serde(default)]
    pub hydrate: bool,
    /// The pool the job holds (the library's `read` pool).
    pub pool: String,
    /// Fingerprints.
    pub items: Vec<String>,
    pub files: Vec<TestStageFile>,
    /// Bytes read from each file (default 4096).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_bytes: Option<u64>,
    /// Waited before each item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    /// Fingerprint → the code its item fails with.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fail_items: BTreeMap<String, String>,
    /// Fingerprints whose item aborts the process once begun (the engine isolates them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crash_on: Vec<String>,
    /// The whole job fails with this code (`retry: never`) before its first item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_error: Option<String>,
}

/// `test.stage` v1.
pub static PARAMS: &[KindParams] = &[KindParams {
    kind: "test.stage",
    version: 1,
    validate: validate_as::<TestStageParams>,
    schema: schema_as::<TestStageParams>,
}];
