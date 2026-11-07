//! The sample table and the sampling rule (spec §10.2): one function, [`frame_at`], decides
//! which source frame every consumer (preview, `fin look`, QA, export) shows at a source time.

use std::cmp::Ordering;

use fin_core::{CoreError, Rational};
use serde_json::json;

use crate::media_corrupt;

/// One frame in presentation order: `pts` in `timescale` ticks after edit lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameEntry {
    pub pts: i64,
    pub duration: u32,
    pub keyframe: bool,
    /// The sample's index in decode order (its index in the track's sample list).
    pub decode_index: u32,
    pub offset: u64,
    pub size: u32,
}

/// A sync sample in decode order with its presentation time (after edit lists), including
/// sync samples that edit lists leave out of the presented frames: a decoder may have to
/// start there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SyncPoint {
    pub decode_index: u32,
    pub pts: i64,
}

/// A video track's presented frames sorted by `pts` (strictly increasing), and its sync
/// samples in decode order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleTable {
    timescale: u32,
    frames: Vec<FrameEntry>,
    sync: Vec<SyncPoint>,
}

impl SampleTable {
    /// The table of `frames` (any order; sorted here), whose sync points are its keyframes.
    /// `MEDIA_CORRUPT` for a zero timescale, no frames, or two frames at one pts. A decode
    /// index may appear twice (an edit list that repeats media).
    pub fn new(timescale: u32, frames: Vec<FrameEntry>) -> Result<SampleTable, CoreError> {
        let sync = frames
            .iter()
            .filter(|f| f.keyframe)
            .map(|f| SyncPoint {
                decode_index: f.decode_index,
                pts: f.pts,
            })
            .collect();
        SampleTable::with_sync(timescale, frames, sync)
    }

    /// As [`SampleTable::new`] with the sync points given (an edit list may drop a keyframe
    /// that a presented frame still decodes from).
    pub fn with_sync(
        timescale: u32,
        mut frames: Vec<FrameEntry>,
        mut sync: Vec<SyncPoint>,
    ) -> Result<SampleTable, CoreError> {
        if timescale == 0 {
            return Err(media_corrupt("the track's timescale is 0", json!("mdhd")));
        }
        if frames.is_empty() {
            return Err(media_corrupt(
                "the video track presents no frames",
                json!("stbl"),
            ));
        }
        frames.sort_by_key(|f| f.pts);
        if let Some(w) = frames.windows(2).find(|w| w[0].pts == w[1].pts) {
            return Err(media_corrupt(
                format!("two frames share presentation time {} ticks", w[0].pts),
                json!({ "pts": w[0].pts, "decodeIndex": [w[0].decode_index, w[1].decode_index] }),
            ));
        }
        sync.sort_by_key(|s| s.decode_index);
        sync.dedup_by_key(|s| s.decode_index);
        Ok(SampleTable {
            timescale,
            frames,
            sync,
        })
    }

    pub fn timescale(&self) -> u32 {
        self.timescale
    }

    /// Presented frames, sorted by `pts`.
    pub fn frames(&self) -> &[FrameEntry] {
        &self.frames
    }

    pub fn sync(&self) -> &[SyncPoint] {
        &self.sync
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Never true: a table has at least one frame.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// The presentation time of frame `i` in seconds.
    pub fn pts_seconds(&self, i: usize) -> Rational {
        Rational::new(self.frames[i].pts, self.timescale as i64).expect("timescale > 0")
    }

    /// The end of the last frame in ticks.
    pub fn end_ticks(&self) -> i64 {
        let last = self.frames[self.frames.len() - 1];
        last.pts + last.duration as i64
    }
}

/// The frame shown at a source time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameAt {
    /// Index into [`SampleTable::frames`].
    pub index: usize,
    /// The time is before the first frame's pts, or at or after the last frame's end (spec
    /// §10.2: the first or last frame is shown and `SOURCE_OUT_OF_RANGE` reported).
    pub out_of_range: bool,
}

/// Spec §10.2: frame `i` owns `(m(i−1), m(i)]` with `m(i) = (pts(i) + pts(i+1)) / 2`; ties go
/// to the earlier frame; before the first midpoint the first frame, after the last the last
/// frame (`out_of_range` when `s` is before pts(0) or at/after the last frame's end). `s` is
/// in seconds; every comparison is exact (`2·s·timescale` against `pts(i) + pts(i+1)` in
/// 128-bit integers, `s` never rounded). O(log n).
pub fn frame_at(table: &SampleTable, s: Rational) -> FrameAt {
    let frames = &table.frames;
    let ts = table.timescale as i128;
    let (num, den) = (s.num() as i128, s.den() as i128);
    // s ≤ m(i)  ⇔  2·num·ts ≤ (pts(i) + pts(i+1))·den   (den > 0)
    let lhs = 2 * num * ts;
    let at_or_before_mid =
        |i: usize| lhs <= (frames[i].pts as i128 + frames[i + 1].pts as i128) * den;
    // The first i in 0..n−1 with s ≤ m(i), else the last frame: `at_or_before_mid` is
    // monotone (false then true) because pts strictly increase.
    let (mut lo, mut hi) = (0usize, frames.len() - 1);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if at_or_before_mid(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let scaled = num * ts; // s in ticks × den
    let before = scaled.cmp(&(frames[0].pts as i128 * den)) == Ordering::Less;
    let after = scaled.cmp(&(table.end_ticks() as i128 * den)) != Ordering::Less;
    FrameAt {
        index: lo,
        out_of_range: before || after,
    }
}

/// The decode index of the sync sample a decoder must start from to output presented frame
/// `i`: the last sync sample at or before the frame in decode order, or the one before it
/// when the frame is a leading picture of an open GOP (it precedes that sync sample in
/// presentation, so it references the previous GOP). 0 when no sync sample precedes it.
pub fn decode_start(table: &SampleTable, i: usize) -> usize {
    let f = table.frames[i];
    let sync = &table.sync;
    let k = sync.partition_point(|s| s.decode_index <= f.decode_index);
    if k == 0 {
        return 0;
    }
    let at = sync[k - 1];
    if f.pts < at.pts && at.decode_index != f.decode_index {
        return if k >= 2 {
            sync[k - 2].decode_index as usize
        } else {
            0
        };
    }
    at.decode_index as usize
}
