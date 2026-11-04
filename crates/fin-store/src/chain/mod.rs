//! Chain resolution (sub-project 4 spec §3.6 steps 4–8, §3.5 "On sync folders", §3.9
//! "Readers"): given the verified segments of one branch and what the local DB already holds,
//! decide what to import, what to mark exported, what to set aside and whether the branch has
//! forked. Pure: no I/O, no clock, and the result depends only on the *set* of segments.
//!
//! # Model
//!
//! History is a tree of **records**, keyed by `txHash` (the BLAKE3 of the record's bytes) and
//! linked by each record's `parentHash`. Segments are only containers. This makes the spec's
//! equivalences fall out of content addressing:
//! - a pack and the segments it replaces hold the same records, so during the grace window
//!   they are one line, not two successors (spec §3.6 step 5, §3.9);
//! - a re-exported duplicate segment adds nothing;
//! - a follower whose DB stops in the middle of a pack resumes inside it;
//! - links cannot form cycles (a cycle would need a hash preimage), and every traversal is
//!   iterative and visits each record at most once.
//!
//! Segment-level fields are still checked and used where the spec says so:
//! - a segment's `prevSegmentHash` must resolve to a present segment or to a hash listed in a
//!   present pack's `replaces`; otherwise the segment waits (a gap);
//! - the segment's first record must continue that predecessor (its `parentHash` is the
//!   predecessor's last record, `seq` is one more), else the segment is malformed;
//! - a record's epoch is the highest epoch of the segments that hold it.
//!
//! # Epoch rule
//!
//! Where two or more lines continue from one record (or from the branch start), each line's
//! epoch is the highest epoch anywhere in it ("the higher-epoch line wins", spec §2.4, §3.6
//! step 6). The DB's unexported tail is a line too, with the epoch it was committed at. On
//! local and network storage the unique highest line wins and the others are quarantined
//! ([`QuarantineReason::StaleEpoch`]); a tie is a fork. On sync storage any two lines are a
//! fork (spec §3.5).
//!
//! The tail competes only while this server holds the lease ([`Known::holds_lease`]).
//! Without the lease it never wins: as soon as the folder holds anything after the point the
//! tail starts from, the tail is set aside (`local_tail_diverged`, spec §3.6 step 7) and the
//! folder's line is imported.
//!
//! # Nothing is dropped
//!
//! Every record of a usable segment ends up in exactly one of: `apply`, `mark_exported`,
//! `quarantine`, `pending`, or the DB's own line (already applied). Records that descend from
//! a malformed file can never link, so they are quarantined as
//! `Malformed("descends from malformed segment <hash12>")`, and a present malformed file is
//! never reported in `waiting`.
//!
//! # Verdicts are per poll
//!
//! `Malformed` verdicts are recomputed on every poll and must not be persisted as final. They
//! depend on what else is present: for example a lying pack (one that lists an absent segment
//! in `replaces`) is honoured until that segment arrives and shows the pack does not hold its
//! records, and while it is honoured it can make a genuine successor look malformed or
//! waiting. Quarantine rows keep the bytes; the next poll may resolve them differently.
//!
//! # Canonical output order
//!
//! - `apply`: chain order (parent before child), as slices of the segment that holds each
//!   record; a record held by several segments is taken from a pack first, then from the
//!   segment with the smallest hash.
//! - `mark_exported`: chain order.
//! - `quarantine`: by segment hash, then first record index.
//! - `waiting`: sorted, without duplicates.
//! - `fork.sides`: by the competing record's `txHash`, the local tail last.

mod graph;
mod walk;

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use serde::Deserialize;

use crate::db::hash_bytes;
use crate::segment::{Header, Segment, check_hash};
use crate::storage_kind::StorageKind;

/// The fields of one record line that resolution needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecMeta {
    pub tx_id: String,
    /// `blake3:<64 hex>` of the record bytes.
    pub tx_hash: String,
    pub seq: u64,
    pub parent_hash: Option<String>,
}

/// A verified segment, or a slice of one, with each record's `txId` and `txHash` parsed once.
///
/// Cheap to clone: it shares the scanned `Arc<Segment>` and the parsed records. Building one
/// hashes every record, so callers that resolve on every poll should cache `SegRef`s by
/// segment hash (segments are write-once).
#[derive(Clone)]
pub struct SegRef {
    seg: Arc<Segment>,
    parsed: Arc<Result<Vec<RecMeta>, String>>,
    range: Range<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecordHead {
    tx_id: String,
    seq: u64,
    #[serde(default)]
    parent_hash: Option<String>,
}

impl SegRef {
    /// The whole segment. Records that do not parse make it malformed for [`resolve`] (it is
    /// quarantined, never applied).
    pub fn new(seg: Arc<Segment>) -> SegRef {
        let parsed = parse_records(&seg);
        let range = 0..seg.records.len();
        SegRef {
            seg,
            parsed: Arc::new(parsed),
            range,
        }
    }

    pub fn hash(&self) -> &str {
        &self.seg.hash
    }

    pub fn header(&self) -> &Header {
        &self.seg.header
    }

    pub fn segment(&self) -> &Arc<Segment> {
        &self.seg
    }

    /// Which of the segment's records this refers to.
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    pub fn is_whole(&self) -> bool {
        self.range == (0..self.seg.records.len())
    }

    /// The record bytes in this slice, exactly as written.
    pub fn records(&self) -> impl Iterator<Item = &[u8]> {
        self.seg.records[self.range.clone()]
            .iter()
            .map(Vec::as_slice)
    }

    /// The parsed records in this slice; empty if the segment's records do not parse.
    pub fn txs(&self) -> &[RecMeta] {
        match &*self.parsed {
            Ok(all) => &all[self.range.clone()],
            Err(_) => &[],
        }
    }

    /// Why the records do not parse, if they don't.
    pub fn parse_error(&self) -> Option<&str> {
        self.parsed.as_ref().as_ref().err().map(String::as_str)
    }

    fn all_txs(&self) -> &[RecMeta] {
        match &*self.parsed {
            Ok(all) => all,
            Err(_) => &[],
        }
    }

    fn whole(&self) -> SegRef {
        self.slice(0..self.seg.records.len())
    }

    fn slice(&self, range: Range<usize>) -> SegRef {
        SegRef {
            seg: self.seg.clone(),
            parsed: self.parsed.clone(),
            range,
        }
    }
}

fn parse_records(seg: &Segment) -> Result<Vec<RecMeta>, String> {
    seg.records
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let head: RecordHead =
                serde_json::from_slice(bytes).map_err(|e| format!("record {i}: {e}"))?;
            if head.tx_id.is_empty() {
                return Err(format!("record {i}: txId is empty"));
            }
            if let Some(p) = &head.parent_hash {
                check_hash("parentHash", p).map_err(|e| format!("record {i}: {e}"))?;
            }
            Ok(RecMeta {
                tx_id: head.tx_id,
                tx_hash: hash_bytes(bytes),
                seq: head.seq,
                parent_hash: head.parent_hash,
            })
        })
        .collect()
}

/// Two `SegRef`s are equal when they name the same records of the same file.
impl PartialEq for SegRef {
    fn eq(&self, other: &Self) -> bool {
        self.seg.hash == other.seg.hash && self.range == other.range
    }
}

impl Eq for SegRef {}

impl fmt::Debug for SegRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let short = self.seg.hash.get(7..19).unwrap_or(&self.seg.hash);
        write!(
            f,
            "SegRef({short} e{} {:?})",
            self.seg.header.epoch, self.range
        )
    }
}

/// What the local DB holds for this branch.
#[derive(Debug, Clone)]
pub struct Known {
    /// `txHash` of the DB's last record that the folder already has (exported by this server,
    /// or imported). `None`: nothing on this branch is in the folder yet.
    pub head: Option<String>,
    /// The segment that holds `head` (the DB's last known segment). Reported in `waiting`
    /// when the folder does not hold `head`.
    pub last_hash: Option<String>,
    /// Every record of this branch in the DB, the tail included: `txId → txHash`.
    pub known_tx: HashMap<String, String>,
    /// The unexported records, in seq order, as `(txId, txHash)`. They continue from `head`.
    pub tail: Vec<(String, String)>,
    /// The highest epoch the tail was committed at (`txs.epoch`). Ignored when `tail` is empty.
    pub tail_epoch: u64,
    /// Whether this server holds the lease now. Without it the tail never wins: once the
    /// folder has moved on from where the tail starts, the tail is set aside (spec §3.6 step 7).
    pub holds_lease: bool,
    pub kind: StorageKind,
}

impl Known {
    /// A DB that holds nothing for this branch.
    pub fn empty(kind: StorageKind) -> Known {
        Known {
            head: None,
            last_hash: None,
            known_tx: HashMap::new(),
            tail: Vec::new(),
            tail_epoch: 0,
            holds_lease: false,
            kind,
        }
    }
}

/// Why segment records are set aside. Quarantined records are kept, never applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuarantineReason {
    /// On a line that lost the epoch rule (spec §3.6 step 6).
    StaleEpoch,
    /// Verified bytes that do not form a valid chain: records that do not parse, seqs or
    /// `parentHash`es that do not continue, a pack that does not contain what it says it
    /// replaces. Only a bug or a hand-made file produces this.
    Malformed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkReason {
    /// Two lines with the same highest epoch (a bug or a manual copy).
    EqualEpoch,
    /// Two lines on a sync folder, where the epoch rule never decides.
    SyncFolder,
    /// A line the DB has already applied (or exported) lost the epoch rule.
    AppliedLineLost,
    /// A record to import carries a `txId` the DB already holds with a different `txHash`.
    TxIdReused { tx_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ForkSide {
    /// A folder line, starting with the record `tx_hash` held in `segment`.
    Segment {
        segment: String,
        tx_hash: String,
        epoch: u64,
    },
    /// This server's unexported records.
    LocalTail { epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForkInfo {
    pub reason: ForkReason,
    /// The last record both lines share (`None`: the branch start).
    pub after: Option<String>,
    pub sides: Vec<ForkSide>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Resolution {
    /// Records to import, in order.
    pub apply: Vec<SegRef>,
    /// `(txId, segment hash)`: tail records the folder already holds (a crash between the
    /// rename and the marking, spec §3.4). Mark them exported; never re-apply them.
    pub mark_exported: Vec<(String, String)>,
    pub quarantine: Vec<(SegRef, QuarantineReason)>,
    /// Set when the branch cannot continue without a person choosing (`CHAIN_FORK`).
    /// Records before the fork point are still in `apply`.
    pub fork: Option<ForkInfo>,
    /// Missing predecessor segment hashes (gaps). Retry on the next poll.
    pub waiting: Vec<String>,
    /// Records neither imported nor set aside yet: beyond a fork point, behind a gap, or
    /// everything when the folder does not hold `Known::head`.
    pub pending: Vec<SegRef>,
    /// The unexported tail lost: quarantine every tail record not in `mark_exported`, then
    /// import `apply`.
    pub local_tail_diverged: bool,
}

/// Resolves one branch. `branch_segments` may come in any order and hold duplicates.
pub fn resolve(branch_segments: &[SegRef], known: &Known) -> Resolution {
    let graph = graph::Graph::build(branch_segments);
    walk::run(&graph, known)
}
