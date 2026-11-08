//! The batch file a scan worker writes to its job's `out/` (Decision 26) and the indexer's
//! intake reads (`apps/finisherd/src/library/catalog/batch-file.ts`, plan 5D Task 7a):
//!
//! - **NDJSON, each line one generated [`ScanBatch`]** (`{seq, entries, dirsDone}`), lines in
//!   order; the worker writes one line per file, so one file holds one walker batch;
//! - at most [`MAX_ENTRIES`] entries (`ScanBatch.entries`, all lines together) and at most
//!   [`MAX_BYTES`] bytes per file;
//! - UTF-8, no line is blank, no key the types do not name (`deny_unknown_fields`).
//!
//! **Order within the job:** a listing's entries all precede the `DirDone` that closes it
//! (in the same line, or in an earlier one or file), and `DirDone.entries` counts every entry
//! emitted for that listing, excluded ones included (plan 5D Task 7b). The walker guarantees
//! both; this module only moves the batches.

use std::io::{self, BufRead};

use crate::types::ScanBatch;

/// Entries per file (the intake's `BATCH_MAX_ENTRIES`).
pub const MAX_ENTRIES: usize = 4096;
/// Bytes per file (the intake's `BATCH_MAX_BYTES`).
pub const MAX_BYTES: usize = 8 * 1024 * 1024;

/// The file name of batch `seq` inside `out/` (a plain name; `library.batch` carries it).
pub fn file_name(seq: u64) -> String {
    format!("scan-{seq:08}.ndjson")
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// `batch` as a one-line batch file; `InvalidInput` when it passes the entry or byte cap.
pub fn encode(batch: &ScanBatch) -> io::Result<Vec<u8>> {
    if batch.entries.len() > MAX_ENTRIES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "a batch of {} entries passes {MAX_ENTRIES}",
                batch.entries.len()
            ),
        ));
    }
    let mut out = serde_json::to_vec(batch).map_err(io::Error::other)?;
    out.push(b'\n');
    if out.len() > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("a batch of {} bytes passes {MAX_BYTES}", out.len()),
        ));
    }
    Ok(out)
}

/// The batches of a batch file, one per line; `InvalidData` for no line, a blank or invalid
/// line, or more than [`MAX_ENTRIES`] entries in all.
pub fn decode(r: impl BufRead) -> io::Result<Vec<ScanBatch>> {
    let mut out = Vec::new();
    let mut entries = 0;
    for (i, line) in r.lines().enumerate() {
        let b: ScanBatch =
            serde_json::from_str(&line?).map_err(|e| bad(format!("line {}: {e}", i + 1)))?;
        entries += b.entries.len();
        if entries > MAX_ENTRIES {
            return Err(bad(format!(
                "line {}: more than {MAX_ENTRIES} entries",
                i + 1
            )));
        }
        out.push(b);
    }
    if out.is_empty() {
        return Err(bad("no batch"));
    }
    Ok(out)
}
