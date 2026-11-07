//! Edit lists (`elst` v0/v1) applied to presentation times (spec §10.2): frames entirely
//! outside an edit are dropped; a frame the edit's start cuts keeps its pts clamped to that
//! start. Empty edits move the media later. Only `media_rate` 1 is supported.

use fin_core::CoreError;
use fin_media::{FrameEntry, SyncPoint};
use mp4_atom::Elst;

use super::reader::{BoxHeader, check_count, decode, full_box_count};
use super::table::Sample;
use crate::error::{limit, unsupported_container};

/// Edits per track: presenting is O(edits × samples), so a hostile edit list is capped.
const MAX_EDITS: u64 = 1024;

/// One edit: a duration in movie ticks (0 in the last edit: to the end of the media), and its
/// media time in media ticks (`None` for an empty edit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Edit {
    pub duration: u64,
    pub media_time: Option<i64>,
}

/// The edits of an `elst` body, after checking its count against its size.
pub(crate) fn parse(body: &[u8], h: &BoxHeader) -> Result<Vec<Edit>, CoreError> {
    let entry = if body.first() == Some(&1) { 20 } else { 12 };
    let count = full_box_count(body, h)? as u64;
    if count > MAX_EDITS {
        return Err(limit("maxEdits", count, MAX_EDITS));
    }
    check_count(body, 8, count, entry, h)?;
    let elst = decode::<Elst>(body, h)?;
    elst.entries
        .iter()
        .map(|e| {
            if e.media_rate.integer() != 1 || e.media_rate.decimal() != 0 {
                return Err(unsupported_container(
                    "mp4",
                    "an edit list with a media rate other than 1",
                ));
            }
            Ok(Edit {
                duration: e.segment_duration,
                media_time: e.media_time.map(|t| t as i64),
            })
        })
        .collect()
}

/// `movie` ticks in media ticks, rounded down.
fn to_media(movie: u64, media_ts: u32, movie_ts: u32) -> i64 {
    (movie as u128 * media_ts as u128 / movie_ts.max(1) as u128).min(i64::MAX as u128) as i64
}

/// The media intervals the edits present: `(presentation start, media start, media end)` in
/// media ticks (`i64::MAX` for an open end).
fn spans(edits: &[Edit], media_ts: u32, movie_ts: u32) -> Vec<(i64, i64, i64)> {
    let mut out = Vec::new();
    let mut movie = 0u64;
    for (k, e) in edits.iter().enumerate() {
        if let Some(mt) = e.media_time {
            let len = match (e.duration, k + 1 == edits.len()) {
                (0, true) => i64::MAX,
                (d, _) => to_media(d, media_ts, movie_ts),
            };
            out.push((
                to_media(movie, media_ts, movie_ts),
                mt,
                mt.saturating_add(len),
            ));
        }
        movie = movie.saturating_add(e.duration);
    }
    out
}

/// The presented frames (sorted by pts, durations from one pts to the next, the last frame's
/// to its end) and the sync samples with their presentation times.
pub(crate) fn present(
    samples: &[Sample],
    edits: &[Edit],
    media_ts: u32,
    movie_ts: u32,
) -> (Vec<FrameEntry>, Vec<SyncPoint>) {
    let spans = match edits.is_empty() {
        true => vec![(0, 0, i64::MAX)],
        false => spans(edits, media_ts, movie_ts),
    };
    let mut frames: Vec<(FrameEntry, i64)> = Vec::new();
    for &(at, mt, end) in &spans {
        for (i, s) in samples.iter().enumerate() {
            let (ct, ce) = (s.ct(), s.ct() + s.duration as i64);
            if ce <= mt || ct >= end {
                continue;
            }
            let pts = at + (ct.max(mt) - mt);
            let stop = at + (ce.min(end) - mt);
            let f = FrameEntry {
                pts,
                duration: 0,
                keyframe: s.key,
                decode_index: i as u32,
                offset: s.offset,
                size: s.size,
            };
            frames.push((f, stop));
        }
    }
    frames.sort_by_key(|(f, _)| f.pts);
    let n = frames.len();
    for k in 0..n {
        let next = if k + 1 < n {
            frames[k + 1].0.pts
        } else {
            frames[k].1
        };
        frames[k].0.duration = (next - frames[k].0.pts).clamp(0, u32::MAX as i64) as u32;
    }
    let (at, mt, _) = spans.first().copied().unwrap_or((0, 0, 0));
    let sync = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.key)
        .map(|(i, s)| SyncPoint {
            decode_index: i as u32,
            pts: at + (s.ct() - mt),
        })
        .collect();
    (frames.into_iter().map(|(f, _)| f).collect(), sync)
}

/// An audio track's priming (media ticks before the first presented sample) and presented
/// length in media ticks.
pub(crate) fn audio_timing(
    samples: &[Sample],
    edits: &[Edit],
    media_ts: u32,
    movie_ts: u32,
) -> (u64, u64) {
    let total: u64 = samples.iter().map(|s| s.duration as u64).sum();
    let spans = spans(edits, media_ts, movie_ts);
    match spans.first() {
        None => (0, total),
        Some(&(_, mt, _)) => {
            let shown: i64 = spans
                .iter()
                .map(|&(_, a, b)| b.min(total as i64) - a.max(0))
                .filter(|&d| d > 0)
                .sum();
            (mt.max(0) as u64, shown.max(0) as u64)
        }
    }
}
