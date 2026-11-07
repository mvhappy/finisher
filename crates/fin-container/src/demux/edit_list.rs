//! Edit lists (`elst` v0/v1) applied to presentation times (spec §10.2): frames entirely
//! outside an edit are dropped; a frame the edit's start cuts keeps its pts clamped to that
//! start. Empty edits move the media later. Only `media_rate` 1 is supported.

use fin_core::CoreError;
use fin_media::{FrameEntry, SyncPoint};
use mp4_atom::Elst;

use super::reader::{BoxHeader, check_count, decode, full_box_count};
use super::table::Sample;
use crate::error::{corrupt, limit, unsupported_container};

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

/// What one candidate sample of an edit gives.
enum Visit {
    /// It ends before the edit starts.
    Before,
    /// `(sample index, pts, end)`.
    Frame(u32, i64, i64),
    Overflow,
}

/// The candidate samples of one edit in composition order: binary search over `order`
/// (sample indices sorted by composition time) for those starting in
/// `[edit start − longest duration, edit end)`.
fn shown<'a>(
    samples: &'a [Sample],
    order: &'a [u32],
    longest: i64,
    (at, mt, end): (i64, i64, i64),
) -> impl Iterator<Item = Visit> + 'a {
    let lo = order.partition_point(|&i| samples[i as usize].ct() < mt.saturating_sub(longest));
    let hi = order.partition_point(|&i| samples[i as usize].ct() < end);
    order[lo..hi.max(lo)].iter().map(move |&i| {
        let s = &samples[i as usize];
        if s.ce() <= mt {
            return Visit::Before;
        }
        let pts = s
            .ct()
            .max(mt)
            .checked_sub(mt)
            .and_then(|d| at.checked_add(d));
        let stop = s
            .ce()
            .min(end)
            .checked_sub(mt)
            .and_then(|d| at.checked_add(d));
        match pts.zip(stop) {
            Some((p, e)) => Visit::Frame(i, p, e),
            None => Visit::Overflow,
        }
    })
}

/// The presented frames (sorted by pts, durations from one pts to the next, the last frame's
/// to its end) and the sync samples with their composition times. Frames are counted against
/// `max` before any is stored (an edit list can repeat the media 1,024 times), and every
/// sample looked at against `2 × max` (one very long sample widens each edit's window):
/// `MEDIA_LIMIT` past either, `MEDIA_CORRUPT` when a presentation time overflows.
pub(crate) fn present(
    samples: &[Sample],
    edits: &[Edit],
    media_ts: u32,
    movie_ts: u32,
    max: u64,
) -> Result<(Vec<FrameEntry>, Vec<SyncPoint>), CoreError> {
    // Without an edit list, pts is the composition time itself (negative ones included).
    let first = samples.iter().map(Sample::ct).min().unwrap_or(0);
    let spans = match edits.is_empty() {
        true => vec![(first, first, i64::MAX)],
        false => spans(edits, media_ts, movie_ts),
    };
    let mut order: Vec<u32> = (0..samples.len() as u32).collect();
    order.sort_by_key(|&i| (samples[i as usize].ct(), i));
    let longest = samples
        .iter()
        .map(|s| i64::from(s.duration))
        .max()
        .unwrap_or(0);
    let overflow = || corrupt("an edit list moves a frame past the time range", 0, "elst");
    let (mut count, mut work) = (0u64, 0u64);
    for &span in &spans {
        for v in shown(samples, &order, longest, span) {
            work += 1;
            if work > max.saturating_mul(2) {
                return Err(limit("maxEditWork", work, max.saturating_mul(2)));
            }
            match v {
                Visit::Before => {}
                Visit::Overflow => return Err(overflow()),
                Visit::Frame(..) => {
                    count += 1;
                    if count > max {
                        return Err(limit("maxPresentedFrames", count, max));
                    }
                }
            }
        }
    }
    let mut frames: Vec<(FrameEntry, i64)> = Vec::with_capacity(count as usize);
    for &span in &spans {
        for v in shown(samples, &order, longest, span) {
            let Visit::Frame(i, pts, stop) = v else {
                continue;
            };
            let s = &samples[i as usize];
            let f = FrameEntry {
                pts,
                ct: s.ct(),
                duration: 0,
                keyframe: s.key,
                decode_index: i,
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
        frames[k].0.duration = next
            .saturating_sub(frames[k].0.pts)
            .clamp(0, u32::MAX as i64) as u32;
    }
    let sync = samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.key)
        .map(|(i, s)| SyncPoint {
            decode_index: i as u32,
            ct: s.ct(),
        })
        .collect();
    Ok((frames.into_iter().map(|(f, _)| f).collect(), sync))
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
