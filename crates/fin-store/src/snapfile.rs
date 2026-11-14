//! Snapshot files (`.fsnap`, sub-project 4 spec §3.4): the segment frame
//! ([`crate::segment`]) with a snapshot header and one body line holding the model state.
//! Snapshots are derived data; in 4A only compaction writes them.

use fin_core::Project;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::segment::{self, SegError, check_hash, check_nonempty, corrupt};

/// The snapshot format written by this code.
pub const FORMAT_VERSION: u32 = 1;
/// The oldest reader that can read what this code writes.
pub const MIN_READER_VERSION: u32 = 1;
/// This code reads any snapshot whose `minReaderVersion` is at most this. Snapshots have their
/// own version, independent of [`segment::READER_VERSION`].
pub const READER_VERSION: u32 = 1;

const MAGIC: &str = "finisher-snapshot";

/// A snapshot header line. `magic` is implied, as for [`segment::Header`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapHeader {
    pub format_version: u32,
    pub min_reader_version: u32,
    pub project_id: String,
    pub branch: String,
    /// The state after the record with this seq…
    pub seq: u64,
    /// …whose hash is this. A snapshot seeds an import only if this lies on the chain.
    pub tx_hash: String,
    /// The `schemaVersion` of the body.
    pub model_schema_version: u32,
    pub writer_id: String,
    pub epoch: u64,
    /// `blake3:` of the project's genesis ([`crate::project_file::genesis_hash`]).
    pub genesis_hash: String,
}

/// A decoded, verified snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub header: SnapHeader,
    /// The `Project` JSON as written, for `fin_core::upcast_snapshot`.
    pub body: Vec<u8>,
    /// `blake3:<64 hex>`.
    pub hash: String,
}

pub fn encode(header: &SnapHeader, project: &Project) -> (Vec<u8>, String) {
    let body = serde_json::to_vec(project).expect("a Project always serialises");
    segment::encode_frame(MAGIC, header, &[&body])
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, SegError> {
    let frame = segment::decode_frame(bytes, MAGIC, FORMAT_VERSION, READER_VERSION)?;
    let header: SnapHeader = serde_json::from_value(Value::Object(frame.header))
        .map_err(|e| corrupt(format!("header: {e}")))?;
    check_nonempty("projectId", &header.project_id)?;
    check_nonempty("branch", &header.branch)?;
    check_nonempty("writerId", &header.writer_id)?;
    check_hash("txHash", &header.tx_hash)?;
    check_hash("genesisHash", &header.genesis_hash)?;
    let [body] = frame.lines.as_slice() else {
        return Err(corrupt("a snapshot holds exactly one body line"));
    };
    Ok(Snapshot {
        header,
        body: body.to_vec(),
        hash: frame.hash,
    })
}
