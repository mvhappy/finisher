//! The batch file a scan worker writes to its job's `out/` (Decision 26): NDJSON, one
//! [`ScanBatch`] per file, so the indexer thread can read, bound and validate it line by line.
//!
//! ```text
//! {"seq":7,"entries":2,"dirsDone":1}     header: the counts of the lines that follow
//! {…a ScanEntry…}                         `entries` lines
//! {…a ScanEntry…}
//! {…a DirDone…}                           `dirsDone` lines
//! ```
//!
//! Counts, not tags, type the lines; a file with fewer or more lines than its header says is
//! refused whole, so a truncated write never applies half a batch.

use std::io::{self, BufRead};

use serde::{Deserialize, Serialize};

use crate::types::{DirDone, ScanBatch, ScanEntry};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Header {
    seq: u64,
    entries: usize,
    dirs_done: usize,
}

/// The file name of batch `seq` inside `out/` (a plain name; `library.batch` carries it).
pub fn file_name(seq: u64) -> String {
    format!("scan-{seq:08}.ndjson")
}

/// `batch` as its NDJSON file.
pub fn encode(batch: &ScanBatch) -> io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(256 * (batch.entries.len() + batch.dirs_done.len() + 1));
    let header = Header {
        seq: batch.seq,
        entries: batch.entries.len(),
        dirs_done: batch.dirs_done.len(),
    };
    line(&mut out, &header)?;
    for e in &batch.entries {
        line(&mut out, e)?;
    }
    for d in &batch.dirs_done {
        line(&mut out, d)?;
    }
    Ok(out)
}

fn line(out: &mut Vec<u8>, v: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *out, v).map_err(io::Error::other)?;
    out.push(b'\n');
    Ok(())
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// The batch in `r` (a file [`encode`] wrote); `InvalidData` for a bad line, a count that
/// does not match, or trailing lines.
pub fn decode(r: impl BufRead) -> io::Result<ScanBatch> {
    let mut lines = r.lines();
    let mut next = |what: &str| -> io::Result<String> {
        lines.next().ok_or_else(|| bad(format!("missing {what}")))?
    };
    let header: Header =
        serde_json::from_str(&next("header")?).map_err(|e| bad(format!("header: {e}")))?;
    let mut batch = ScanBatch {
        seq: header.seq,
        entries: Vec::with_capacity(header.entries),
        dirs_done: Vec::with_capacity(header.dirs_done),
    };
    for i in 0..header.entries {
        let e: ScanEntry =
            serde_json::from_str(&next("entry")?).map_err(|e| bad(format!("entry {i}: {e}")))?;
        batch.entries.push(e);
    }
    for i in 0..header.dirs_done {
        let d: DirDone = serde_json::from_str(&next("dirDone")?)
            .map_err(|e| bad(format!("dirDone {i}: {e}")))?;
        batch.dirs_done.push(d);
    }
    if next("end").is_ok() {
        return Err(bad("lines after the last"));
    }
    Ok(batch)
}
