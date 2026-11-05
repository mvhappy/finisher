//! Preparing the next segment of a branch: encoding it, or adopting one a crash left.

use std::fs;
use std::path::Path;

use super::rows::broken;
use super::{Attempt, Row};
use crate::names::{self, SegmentName};
use crate::segment::{self, Header};
use crate::{Store, StoreError, fsio};

/// Encodes the next segment from `rows`: as many as fit the caps, at least one.
pub(super) fn prepare(
    store: &Store,
    branch: &str,
    rows: &[Row],
    lease_epoch: u64,
    prev: Option<String>,
) -> Result<Attempt, StoreError> {
    let salt = match &store.export.salt {
        Some((b, f, ms)) if b == branch && Some(*f) == rows.first().map(|r| r.seq) => *ms,
        _ => 0,
    };
    let genesis_hash = crate::project_file::db_genesis_hash(&store.conn)?;
    let mut n = 0;
    let mut body = 0;
    for row in rows {
        let line = row.bytes.len() + 1;
        if n > 0
            && (n == segment::MAX_RECORDS
                || body + line + segment::FRAME_RESERVE > segment::MAX_BYTES)
        {
            break;
        }
        n += 1;
        body += line;
    }
    loop {
        let chunk = &rows[..n];
        let first = chunk[0].seq;
        let last = chunk[n - 1].seq;
        let versions = chunk.iter().map(|r| r.schema_version);
        // The highest commit epoch, never the epoch at export (module docs).
        let epoch = chunk.iter().map(|r| r.epoch).max().unwrap_or(0);
        if epoch > lease_epoch {
            return Err(broken(
                branch,
                format!("records committed at epoch {epoch}, above the lease epoch {lease_epoch}"),
            ));
        }
        let created_at = chunk.iter().map(|r| r.committed_at).max().unwrap_or(0) + salt;
        let lines: Vec<&[u8]> = chunk.iter().map(|r| r.bytes.as_slice()).collect();
        // From content, so a re-export is byte-identical (an edit-only segment stays v1).
        let (format_version, min_reader_version) = segment::version_for(&lines);
        let header = Header {
            format_version,
            min_reader_version,
            project_id: store.project_id.clone(),
            branch: branch.to_string(),
            writer_id: store.server_id.clone(),
            epoch,
            first_seq: first,
            last_seq: last,
            prev_segment_hash: prev.clone(),
            op_schema_versions: [
                versions.clone().min().unwrap_or(0),
                versions.max().unwrap_or(0),
            ],
            created_at,
            genesis_hash: genesis_hash.clone(),
            pack: false,
            replaces: Vec::new(),
        };
        let (bytes, hash) = segment::encode(&header, &lines);
        if bytes.len() > segment::MAX_BYTES {
            if n > 1 {
                n -= 1;
                continue;
            }
            // `too_large` stops export before such a record; never write it.
            return Err(broken(
                branch,
                format!("seq {first} does not fit a segment"),
            ));
        }
        let name = names::hash12(&hash)
            .and_then(|h| SegmentName::new(epoch, first, last, h))
            .ok_or_else(|| {
                broken(
                    branch,
                    format!("epoch {epoch} or seqs {first}..={last} do not fit a segment name"),
                )
            })?;
        return Ok(Attempt {
            branch: branch.to_string(),
            first_seq: first,
            last_seq: last,
            epoch,
            prev,
            name: name.to_string(),
            bytes,
            hash,
            tx_hashes: chunk.iter().map(|r| r.tx_hash.clone()).collect(),
            failed_reads: 0,
        });
    }
}

/// A segment already in `dir` that holds exactly our next records from the same predecessor:
/// left by a crash between the rename and the marking. The longest one wins.
pub(super) fn adoptable(
    store: &Store,
    branch: &str,
    dir: &Path,
    rows: &[Row],
    prev: Option<&str>,
) -> Option<Attempt> {
    let first = rows[0].seq;
    let last = rows[rows.len() - 1].seq;
    let mut candidates: Vec<SegmentName> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| SegmentName::parse(e.ok()?.file_name().to_str()?))
        .filter(|n| n.first_seq() == first && n.last_seq() <= last)
        .collect();
    candidates.sort_by_key(|n| std::cmp::Reverse(n.last_seq()));
    candidates.into_iter().find_map(|name| {
        let bytes = fsio::read_uncached(&dir.join(name.to_string())).ok()?;
        let seg = segment::decode(&bytes).ok()?;
        let h = &seg.header;
        let same = h.branch == branch
            && h.project_id == store.project_id
            && h.prev_segment_hash.as_deref() == prev
            && !h.pack
            && h.epoch == name.epoch()
            && names::hash12(&seg.hash) == Some(name.hash12())
            && seg.records.len() as u64 == name.last_seq() - first + 1
            && seg.records.iter().zip(rows).all(|(r, row)| *r == row.bytes);
        same.then(|| Attempt {
            branch: branch.to_string(),
            first_seq: first,
            last_seq: name.last_seq(),
            epoch: h.epoch,
            prev: prev.map(str::to_string),
            name: name.to_string(),
            bytes: Vec::new(),
            hash: seg.hash.clone(),
            tx_hashes: rows[..seg.records.len()]
                .iter()
                .map(|r| r.tx_hash.clone())
                .collect(),
            failed_reads: 0,
        })
    })
}
