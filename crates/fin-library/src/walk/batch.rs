//! Batches over the job envelope (Decision 26): up to `batch_entries` items (entries plus
//! `DirDone`s) and about `batch_bytes` of NDJSON per [`ScanBatch`]; a listing larger than one
//! batch is split, and its `DirDone` lands in the batch that holds its last entry, so every
//! entry of a listing precedes the `DirDone` that closes it (Decision 25).

use std::time::{Duration, Instant};

use super::{CancelToken, WalkError as Halt, WalkStop};
use crate::raw::RawRel;
use crate::types::{DirDone, ScanBatch, ScanEntry};

/// Base64 grows bytes by 4/3; JSON adds quotes and separators.
fn b64_len(n: usize) -> usize {
    n.div_ceil(3) * 4 + 3
}

fn rel_len(r: &RawRel) -> usize {
    16 + r
        .components()
        .iter()
        .map(|c| b64_len(c.len()))
        .sum::<usize>()
}

/// An upper estimate of an entry's NDJSON size (field names, base64 names, escaped text).
pub(crate) fn entry_bytes(e: &ScanEntry) -> usize {
    240 + rel_len(&e.dir)
        + b64_len(e.name.len())
        + 2 * (e.name_nfc.len() + e.rel_key.len())
        + e.file_id.as_ref().map_or(0, |f| b64_len(f.len()))
        + e.dev.as_ref().map_or(0, String::len)
        + e.excluded.as_ref().map_or(0, String::len)
}

pub(crate) fn done_bytes(d: &DirDone) -> usize {
    96 + rel_len(&d.dir) + d.code.as_ref().map_or(0, String::len)
}

/// Collects entries and `DirDone`s into batches and hands each full one to the sink.
pub(crate) struct Batcher<'a> {
    sink: &'a mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
    cancel: &'a CancelToken,
    max_items: usize,
    max_bytes: usize,
    flush_every: Duration,
    cur: ScanBatch,
    items: usize,
    bytes: usize,
    since: Option<Instant>,
    pub(crate) batches: u64,
}

impl<'a> Batcher<'a> {
    pub(crate) fn new(
        sink: &'a mut dyn FnMut(ScanBatch) -> Result<(), WalkStop>,
        cancel: &'a CancelToken,
        max_items: usize,
        max_bytes: usize,
        flush_every: Duration,
    ) -> Self {
        Self {
            sink,
            cancel,
            max_items: max_items.max(1),
            max_bytes: max_bytes.max(1),
            flush_every,
            cur: ScanBatch {
                seq: 0,
                entries: Vec::new(),
                dirs_done: Vec::new(),
            },
            items: 0,
            bytes: 0,
            since: None,
            batches: 0,
        }
    }

    fn room(&mut self, bytes: usize) -> Result<(), Halt> {
        if self.items > 0 && (self.items >= self.max_items || self.bytes + bytes > self.max_bytes) {
            self.flush()?;
        }
        if self.since.is_none() {
            self.since = Some(Instant::now());
        }
        self.items += 1;
        self.bytes += bytes;
        Ok(())
    }

    pub(crate) fn entries(&mut self, entries: Vec<ScanEntry>) -> Result<(), Halt> {
        for e in entries {
            self.room(entry_bytes(&e))?;
            self.cur.entries.push(e);
        }
        Ok(())
    }

    pub(crate) fn done(&mut self, d: DirDone) -> Result<(), Halt> {
        self.room(done_bytes(&d))?;
        self.cur.dirs_done.push(d);
        Ok(())
    }

    /// Hands the batch to the sink when it is non-empty; a cancel seen afterwards halts the
    /// walk, so at most this batch follows a cancel.
    pub(crate) fn flush(&mut self) -> Result<(), Halt> {
        if self.items == 0 {
            return Ok(());
        }
        let next = ScanBatch {
            seq: self.cur.seq + 1,
            entries: Vec::new(),
            dirs_done: Vec::new(),
        };
        let batch = std::mem::replace(&mut self.cur, next);
        self.items = 0;
        self.bytes = 0;
        self.since = None;
        self.batches += 1;
        (self.sink)(batch).map_err(Halt::Stopped)?;
        if self.cancel.is_cancelled() {
            return Err(Halt::Cancelled);
        }
        Ok(())
    }

    /// When the open batch is due by time (`Timing::flush`).
    pub(crate) fn due(&self) -> Option<Instant> {
        self.since.map(|s| s + self.flush_every)
    }

    pub(crate) fn tick(&mut self, now: Instant) -> Result<(), Halt> {
        match self.due() {
            Some(d) if d <= now => self.flush(),
            _ => Ok(()),
        }
    }
}
