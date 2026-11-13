//! `table.bin` (plan 6B-1 Decision 7): a source's [`SampleTable`] in a versioned binary form,
//! kept beside a proxy in its cache entry so every reader maps a source time to an original
//! frame index with `frame_at` on the original's table, then reads proxy frame `i`.
//!
//! Little-endian, fixed-size records:
//!
//! | bytes | field |
//! |---|---|
//! | 8 | magic `FINSTBL\0` |
//! | 4 | version ([`TABLE_VERSION`]) |
//! | 4 | timescale |
//! | 4 | `n` presented frames |
//! | 4 | `m` sync points |
//! | 37 × `n` | frames in presentation order: `pts` i64, `ct` i64, `duration` u32, `decodeIndex` u32, `offset` u64, `size` u32, `keyframe` u8 (0 or 1) |
//! | 12 × `m` | sync points in decode order: `decodeIndex` u32, `ct` i64 |
//!
//! [`decode`] reads exactly that many bytes (anything else is `MEDIA_CORRUPT {at: "table.bin"}`)
//! and rebuilds the table through [`SampleTable::with_sync`], so its checks apply.

use fin_core::CoreError;
use serde_json::json;

use crate::media_corrupt;
use crate::sample::{FrameEntry, SampleTable, SyncPoint};

/// The file name in a proxy's cache entry.
pub const TABLE_FILE: &str = "table.bin";
/// The format version this build writes and reads.
pub const TABLE_VERSION: u32 = 1;
const MAGIC: &[u8; 8] = b"FINSTBL\0";
const HEADER: usize = 24;
const FRAME: usize = 37;
const SYNC: usize = 12;

/// `table` as `table.bin` bytes.
pub fn encode(table: &SampleTable) -> Vec<u8> {
    let (frames, sync) = (table.frames(), table.sync());
    let mut out = Vec::with_capacity(HEADER + FRAME * frames.len() + SYNC * sync.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&TABLE_VERSION.to_le_bytes());
    out.extend_from_slice(&table.timescale().to_le_bytes());
    out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
    out.extend_from_slice(&(sync.len() as u32).to_le_bytes());
    for f in frames {
        out.extend_from_slice(&f.pts.to_le_bytes());
        out.extend_from_slice(&f.ct.to_le_bytes());
        out.extend_from_slice(&f.duration.to_le_bytes());
        out.extend_from_slice(&f.decode_index.to_le_bytes());
        out.extend_from_slice(&f.offset.to_le_bytes());
        out.extend_from_slice(&f.size.to_le_bytes());
        out.push(u8::from(f.keyframe));
    }
    for s in sync {
        out.extend_from_slice(&s.decode_index.to_le_bytes());
        out.extend_from_slice(&s.ct.to_le_bytes());
    }
    out
}

fn corrupt(message: &str) -> CoreError {
    media_corrupt(format!("table.bin: {message}"), json!(TABLE_FILE))
}

/// Reads `N` bytes at `*at`.
fn take<const N: usize>(b: &[u8], at: &mut usize) -> [u8; N] {
    let mut out = [0u8; N];
    out.copy_from_slice(&b[*at..*at + N]);
    *at += N;
    out
}

/// The table `bytes` hold (module docs).
pub fn decode(bytes: &[u8]) -> Result<SampleTable, CoreError> {
    if bytes.len() < HEADER || &bytes[..8] != MAGIC {
        return Err(corrupt("not a sample table"));
    }
    let mut at = 8;
    let version = u32::from_le_bytes(take(bytes, &mut at));
    if version != TABLE_VERSION {
        return Err(corrupt(&format!(
            "version {version} (this build reads {TABLE_VERSION})"
        )));
    }
    let timescale = u32::from_le_bytes(take(bytes, &mut at));
    let n = u32::from_le_bytes(take(bytes, &mut at)) as u64;
    let m = u32::from_le_bytes(take(bytes, &mut at)) as u64;
    let want = HEADER as u64 + FRAME as u64 * n + SYNC as u64 * m;
    if bytes.len() as u64 != want {
        return Err(corrupt(&format!(
            "{} bytes for {n} frames and {m} sync points ({want} expected)",
            bytes.len()
        )));
    }
    let mut frames = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let pts = i64::from_le_bytes(take(bytes, &mut at));
        let ct = i64::from_le_bytes(take(bytes, &mut at));
        let duration = u32::from_le_bytes(take(bytes, &mut at));
        let decode_index = u32::from_le_bytes(take(bytes, &mut at));
        let offset = u64::from_le_bytes(take(bytes, &mut at));
        let size = u32::from_le_bytes(take(bytes, &mut at));
        let keyframe = match take::<1>(bytes, &mut at)[0] {
            0 => false,
            1 => true,
            _ => return Err(corrupt("a keyframe flag is not 0 or 1")),
        };
        frames.push(FrameEntry {
            pts,
            ct,
            duration,
            keyframe,
            decode_index,
            offset,
            size,
        });
    }
    let mut sync = Vec::with_capacity(m as usize);
    for _ in 0..m {
        let decode_index = u32::from_le_bytes(take(bytes, &mut at));
        let ct = i64::from_le_bytes(take(bytes, &mut at));
        sync.push(SyncPoint { decode_index, ct });
    }
    if frames.windows(2).any(|w| w[0].pts >= w[1].pts) {
        return Err(corrupt("frames are not in presentation order"));
    }
    SampleTable::with_sync(timescale, frames, sync)
}
