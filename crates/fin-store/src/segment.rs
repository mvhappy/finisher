//! Segment files (`.fseg`, sub-project 4 spec §3.4): UTF-8 JSON lines.
//!
//! 1. A header line `{"magic":"finisher-segment","formatVersion",…}`.
//! 2. One record line per transaction: the exact bytes stored at commit, never re-encoded.
//! 3. A trailer line `{"crc32c":"<8 hex>","hash":"blake3:<64 hex>"}`, both over every byte
//!    before the trailer line.
//!
//! Every line, the trailer included, ends with `\n`. Snapshots ([`crate::snapfile`]) use the
//! same frame, so the frame code here is shared.
//!
//! The version follows content (plan 4B Decision 4): a segment or pack whose records are all
//! `edit` is written as format 1 (`minReaderVersion` 1, readable by 4A), one holding any 4B kind
//! as format 2 ([`version_for`]). So a re-export is byte-identical, and a 4A follower keeps
//! following until a 4B kind is used. Readers accept both.
//!
//! Decoding tells a file that has not fully arrived ([`SegError::Incomplete`]: retried on the
//! next poll) from a damaged one ([`SegError::Corrupt`]). A truncated file is always
//! `Incomplete`; any single damaged byte of a complete file is always `Corrupt`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The segment format this code writes for a file holding any 4B record kind
/// ([`version_for`]).
pub const FORMAT_VERSION: u32 = 2;
/// The oldest reader that can read such a file.
pub const MIN_READER_VERSION: u32 = 2;
/// This code reads any segment whose `minReaderVersion` is at most this (versions 1 and 2).
pub const READER_VERSION: u32 = 2;
/// The format of a file whose records are all `edit`: what sub-project 4A writes and reads.
const EDIT_ONLY_VERSION: u32 = 1;
/// A segment holds at most this many records ([`max_records`])…
pub const MAX_RECORDS: usize = 1_000;

#[cfg(any(test, feature = "test-hooks"))]
thread_local! {
    static MAX_RECORDS_FOR_TEST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// The most records export and compaction put in one segment: [`MAX_RECORDS`], or what
/// [`set_max_records_for_test`] set on this thread.
pub fn max_records() -> usize {
    #[cfg(any(test, feature = "test-hooks"))]
    if let Some(n) = MAX_RECORDS_FOR_TEST.with(std::cell::Cell::get) {
        return n;
    }
    MAX_RECORDS
}

/// Test hook: export and compaction on the calling thread cut segments after `n` records
/// (at least 1), so a few records span several segments. Thread-local: tests running in
/// parallel keep the real limit. Readers never check the count.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn set_max_records_for_test(n: usize) {
    MAX_RECORDS_FOR_TEST.with(|c| c.set(Some(n.max(1))));
}
/// …or this many bytes. One transaction is never split, so no record may be larger than
/// [`MAX_RECORD_BYTES`]: then every segment fits.
pub const MAX_BYTES: usize = 4 << 20;
/// Room kept for the header and trailer lines (both are far smaller).
pub const FRAME_RESERVE: usize = 1024;
/// The largest record a commit accepts (`TX_TOO_LARGE` above it).
pub const MAX_RECORD_BYTES: usize = MAX_BYTES - FRAME_RESERVE;

const MAGIC: &str = "finisher-segment";
const HASH_ALG: &str = "blake3";
const TRAILER_START: &[u8] = b"{\"crc32c\":\"";

/// A segment header line. `magic` is implied: written by [`encode`], checked by [`decode`].
/// `pack`/`replaces` (compaction packs, spec §3.9) are written only for packs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub format_version: u32,
    pub min_reader_version: u32,
    pub project_id: String,
    pub branch: String,
    pub writer_id: String,
    pub epoch: u64,
    pub first_seq: u64,
    pub last_seq: u64,
    /// The previous segment's hash on this branch; `null` for the first.
    pub prev_segment_hash: Option<String>,
    /// `[min, max]` record `schemaVersion` in the file.
    pub op_schema_versions: [u32; 2],
    /// Unix milliseconds; informational, never used for ordering.
    pub created_at: i64,
    /// `blake3:` of the project's genesis ([`crate::project_file::genesis_hash`]).
    pub genesis_hash: String,
    /// On a 4B branch (any branch but `main`): the `txHash` of the `branch` record that made
    /// it, so a reader tells two branches made under one id apart (plan 4B Decision 33).
    /// Absent on `main`. A segment that has it is format 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch_record: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pack: bool,
    /// For a pack: the segments it replaces.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaces: Vec<Replaced>,
}

/// One segment a pack replaces: its hash and the `txHash` of its last record. A segment that
/// names a replaced segment as its predecessor must continue exactly that record, so a pack
/// can stand in for the segment once it is deleted without letting a misnamed file link.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Replaced {
    pub hash: String,
    pub last_tx_hash: String,
}

/// A decoded, verified segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub header: Header,
    /// The record lines, byte-for-byte, without their `\n`.
    pub records: Vec<Vec<u8>>,
    /// `blake3:<64 hex>`.
    pub hash: String,
}

/// Why a segment or snapshot file is not usable.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SegError {
    /// Short, or no trailer yet: the file is still arriving. Retry later.
    #[error("the file is incomplete")]
    Incomplete,
    #[error("the file is damaged: {0}")]
    Corrupt(String),
    /// Written by a newer Finisher that this code cannot read.
    #[error("the file needs Finisher reader version {min_reader}")]
    Newer { min_reader: u32 },
    /// A newer format revision uses a hash algorithm this code does not know.
    #[error("the file uses an unsupported hash algorithm")]
    UnsupportedHash,
}

/// The file bytes and their hash. Each record must be non-empty and hold no `\n` (record
/// bytes are compact JSON, which never does).
pub fn encode(header: &Header, records: &[&[u8]]) -> (Vec<u8>, String) {
    encode_frame(MAGIC, header, records)
}

/// `(formatVersion, minReaderVersion)` for a segment holding `records`: `(1, 1)` when every
/// record's `kind` is `edit`, else `(2, 2)`. The kind is read from each record's bytes; bytes
/// whose kind cannot be read count as needing format 2.
pub fn version_for(records: &[&[u8]]) -> (u32, u32) {
    #[derive(Deserialize)]
    struct Kind {
        kind: fin_core::RecordKind,
    }
    let edit_only = records
        .iter()
        .all(|r| serde_json::from_slice::<Kind>(r).is_ok_and(|k| k.kind.is_4a()));
    if edit_only {
        (EDIT_ONLY_VERSION, EDIT_ONLY_VERSION)
    } else {
        (FORMAT_VERSION, MIN_READER_VERSION)
    }
}

/// [`version_for`] on `branch`: a segment of a 4B branch (not `main`) names its `branch`
/// record ([`Header::branch_record`]), which needs format 2 whatever its records hold.
pub fn version_for_branch(branch: &str, records: &[&[u8]]) -> (u32, u32) {
    if branch == crate::MAIN {
        version_for(records)
    } else {
        (FORMAT_VERSION, MIN_READER_VERSION)
    }
}

/// Decodes and verifies a segment of any format this code reads.
pub fn decode(bytes: &[u8]) -> Result<Segment, SegError> {
    decode_with_reader(bytes, READER_VERSION)
}

/// [`decode`] as a reader of `reader_version` would: a file whose `minReaderVersion` is above
/// it is [`SegError::Newer`]. Shows what an older Finisher makes of a file (its newest format
/// is its reader version). For tests.
#[doc(hidden)]
pub fn decode_with_reader(bytes: &[u8], reader_version: u32) -> Result<Segment, SegError> {
    let frame = decode_frame(bytes, MAGIC, reader_version, reader_version)?;
    let header: Header = serde_json::from_value(Value::Object(frame.header))
        .map_err(|e| corrupt(format!("header: {e}")))?;
    check_nonempty("projectId", &header.project_id)?;
    check_nonempty("branch", &header.branch)?;
    check_nonempty("writerId", &header.writer_id)?;
    check_hash("genesisHash", &header.genesis_hash)?;
    // One record per seq: `lastSeq - firstSeq + 1` records (no overflow at `u64::MAX`).
    let span = header.last_seq.checked_sub(header.first_seq);
    if span.and_then(|d| d.checked_add(1)) != Some(frame.lines.len() as u64) {
        return Err(corrupt(format!(
            "{} records for seqs {}..={}",
            frame.lines.len(),
            header.first_seq,
            header.last_seq
        )));
    }
    if header.format_version == EDIT_ONLY_VERSION {
        // A format 1 label means edit-only (what 4A reads): a 4B kind in it is damage.
        #[derive(Deserialize)]
        struct Kind {
            kind: Option<fin_core::RecordKind>,
        }
        let has_4b = frame.lines.iter().any(|l| {
            serde_json::from_slice::<Kind>(l).is_ok_and(|k| k.kind.is_some_and(|k| !k.is_4a()))
        });
        if has_4b {
            return Err(corrupt("a format 1 segment holds a record of a 4B kind"));
        }
    }
    match (&header.branch_record, header.branch == crate::MAIN) {
        (Some(_), true) => return Err(corrupt("a main segment names a branch record")),
        (None, false) => return Err(corrupt("a branch segment names no branch record")),
        (Some(r), false) => {
            check_hash("branchRecord", r)?;
            if header.format_version == EDIT_ONLY_VERSION {
                return Err(corrupt("a format 1 segment names a branch record"));
            }
        }
        (None, true) => {}
    }
    let [low, high] = header.op_schema_versions;
    if low > high {
        return Err(corrupt("opSchemaVersions is not [min, max]"));
    }
    if let Some(prev) = &header.prev_segment_hash {
        check_hash("prevSegmentHash", prev)?;
    }
    for r in &header.replaces {
        check_hash("replaces.hash", &r.hash)?;
        check_hash("replaces.lastTxHash", &r.last_tx_hash)?;
    }
    if header.pack == header.replaces.is_empty() {
        return Err(corrupt(
            "a pack, and only a pack, lists the segments it replaces",
        ));
    }
    Ok(Segment {
        header,
        records: frame.lines.iter().map(|l| l.to_vec()).collect(),
        hash: frame.hash,
    })
}

pub(crate) fn corrupt(why: impl Into<String>) -> SegError {
    SegError::Corrupt(why.into())
}

pub(crate) fn check_nonempty(field: &str, value: &str) -> Result<(), SegError> {
    if value.is_empty() {
        return Err(corrupt(format!("{field} is empty")));
    }
    Ok(())
}

/// `blake3:<64 lowercase hex>`.
pub(crate) fn check_hash(field: &str, hash: &str) -> Result<(), SegError> {
    let ok = hash
        .strip_prefix("blake3:")
        .is_some_and(|hex| hex.len() == 64 && is_lower_hex(hex.as_bytes()));
    if !ok {
        return Err(corrupt(format!("{field} {hash:?} is not a blake3 hash")));
    }
    Ok(())
}

fn is_lower_hex(s: &[u8]) -> bool {
    s.iter()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(b))
}

/// A verified frame: the header object (with `magic`) and the body lines after it.
pub(crate) struct Frame<'a> {
    pub header: Map<String, Value>,
    pub lines: Vec<&'a [u8]>,
    pub hash: String,
}

/// Header line (with `magic` first), one line per entry of `lines`, then the trailer.
pub(crate) fn encode_frame(
    magic: &str,
    header: &impl Serialize,
    lines: &[&[u8]],
) -> (Vec<u8>, String) {
    let head = serde_json::to_vec(header).expect("a header holds only strings and numbers");
    assert!(
        head.len() > 2 && head[0] == b'{',
        "a header serialises as a non-empty object"
    );
    let mut out = format!("{{\"magic\":\"{magic}\",").into_bytes();
    out.extend_from_slice(&head[1..]);
    out.push(b'\n');
    for line in lines {
        assert!(
            !line.is_empty() && !line.contains(&b'\n'),
            "a body line is non-empty and holds no newline"
        );
        out.extend_from_slice(line);
        out.push(b'\n');
    }
    let hash = format!("{HASH_ALG}:{}", blake3::hash(&out).to_hex());
    let trailer = format!(
        "{{\"crc32c\":\"{:08x}\",\"hash\":\"{hash}\"}}\n",
        crc32c::crc32c(&out)
    );
    out.extend_from_slice(trailer.as_bytes());
    (out, hash)
}

/// Splits, verifies and version-checks a frame. `format_version` is the newest format this
/// code writes for the file kind: within it the hash is always BLAKE3, so another algorithm is
/// damage. `reader_version` is this code's reader version for the file kind (each file kind
/// has its own): a higher `minReaderVersion` is [`SegError::Newer`].
///
/// Order: completeness, trailer shape, CRC, header and `minReaderVersion`, then the hash. The
/// CRC comes before the version check so a damaged version digit is `Corrupt`, not `Newer`.
pub(crate) fn decode_frame<'a>(
    bytes: &'a [u8],
    magic: &str,
    format_version: u32,
    reader_version: u32,
) -> Result<Frame<'a>, SegError> {
    let Some((&end, _)) = bytes.split_last() else {
        return Err(SegError::Incomplete);
    };
    let terminated = end == b'\n';
    let content = if terminated {
        &bytes[..bytes.len() - 1]
    } else {
        bytes
    };
    let body_end = content
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let last = &content[body_end..];
    if !terminated {
        // A cut file ends mid-line. Only a complete trailer followed by more bytes (a damaged
        // final newline) cannot be a cut.
        let after_trailer = last.starts_with(TRAILER_START)
            && last
                .iter()
                .position(|&b| b == b'}')
                .is_some_and(|i| i + 1 < last.len());
        return Err(if after_trailer {
            corrupt("bytes after the trailer")
        } else {
            SegError::Incomplete
        });
    }
    let Some(trailer) = parse_trailer(last) else {
        // A file cut at a line boundary ends with a whole header or record line: a JSON object
        // with neither trailer key. One damaged byte cannot turn a trailer into that.
        let cut_at_line = serde_json::from_slice::<Map<String, Value>>(last)
            .is_ok_and(|m| !m.contains_key("crc32c") && !m.contains_key("hash"));
        return Err(if cut_at_line {
            SegError::Incomplete
        } else {
            corrupt("malformed trailer")
        });
    };
    let body = &bytes[..body_end];
    if crc32c::crc32c(body) != trailer.crc {
        return Err(corrupt("CRC-32C mismatch"));
    }
    let text = std::str::from_utf8(body).map_err(|e| corrupt(format!("not UTF-8: {e}")))?;
    let Some(text) = text.strip_suffix('\n') else {
        return Err(corrupt("no header line"));
    };
    let mut lines = text.split('\n');
    let head = lines.next().unwrap_or_default();
    let header: Map<String, Value> =
        serde_json::from_str(head).map_err(|e| corrupt(format!("header: {e}")))?;
    if header.get("magic").and_then(Value::as_str) != Some(magic) {
        return Err(corrupt(format!("header magic is not {magic:?}")));
    }
    let version = |key: &str| {
        header
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| corrupt(format!("header {key} is missing or not a version")))
    };
    let (format, min_reader) = (version("formatVersion")?, version("minReaderVersion")?);
    if min_reader > format {
        return Err(corrupt("minReaderVersion is above formatVersion"));
    }
    if min_reader > reader_version {
        return Err(SegError::Newer { min_reader });
    }
    if trailer.alg != HASH_ALG {
        return Err(if format > format_version {
            SegError::UnsupportedHash
        } else {
            corrupt(format!("format {format} uses {HASH_ALG}"))
        });
    }
    let digest = blake3::hash(body).to_hex();
    if trailer.digest != digest.as_str() {
        return Err(corrupt("hash mismatch"));
    }
    if lines.clone().any(str::is_empty) {
        return Err(corrupt("empty line"));
    }
    Ok(Frame {
        header,
        lines: lines.map(str::as_bytes).collect(),
        hash: format!("{HASH_ALG}:{digest}"),
    })
}

struct Trailer<'a> {
    crc: u32,
    alg: &'a str,
    digest: &'a str,
}

/// Exactly `{"crc32c":"<8 lowercase hex>","hash":"<[a-z0-9]+>:<lowercase hex>"}`.
fn parse_trailer(line: &[u8]) -> Option<Trailer<'_>> {
    let s = std::str::from_utf8(line).ok()?;
    let rest = s.strip_prefix("{\"crc32c\":\"")?;
    let (crc, rest) = (rest.get(..8)?, rest.get(8..)?);
    let hash = rest.strip_prefix("\",\"hash\":\"")?.strip_suffix("\"}")?;
    let (alg, digest) = hash.split_once(':')?;
    let alg_ok = !alg.is_empty()
        && alg
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    (is_lower_hex(crc.as_bytes())
        && alg_ok
        && !digest.is_empty()
        && is_lower_hex(digest.as_bytes()))
    .then(|| Trailer {
        crc: u32::from_str_radix(crc, 16).expect("8 hex digits fit a u32"),
        alg,
        digest,
    })
}
