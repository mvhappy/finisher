//! `Cues`: the index of keyframes by cluster. Read under the limits (each element counts
//! against `max_boxes`, the video track's cue points against `max_samples`) and checked against
//! the cluster scan. The sample table needs every block, so the scan always runs; the cues
//! then add sync points (see [`mark`]).

use std::collections::HashMap;
use std::io::{Read, Seek};

use fin_core::CoreError;

use super::cluster::RawFrame;
use super::ebml::{Ebml, Element};
use crate::error::limit;

/// One cue of the video track: its time in Segment ticks and the cluster's position relative
/// to the Segment's body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Cue {
    pub time: u64,
    pub cluster: u64,
}

/// The cue points of `track`.
pub(super) fn read<R: Read + Seek>(
    eb: &mut Ebml<R>,
    cues: &Element,
    track: u64,
) -> Result<Vec<Cue>, CoreError> {
    let max = eb.limits.max_samples as u64;
    let mut out = Vec::new();
    for p in eb.children(cues, 2)? {
        if p.id != 0xBB {
            continue;
        }
        let mut time = None;
        let mut positions = Vec::new();
        for c in eb.children(&p, 3)? {
            match c.id {
                0xB3 => time = Some(eb.uint(&c)?),
                0xB7 => {
                    let (mut t, mut pos) = (None, None);
                    for f in eb.children(&c, 4)? {
                        match f.id {
                            0xF7 => t = Some(eb.uint(&f)?),
                            0xF1 => pos = Some(eb.uint(&f)?),
                            _ => {}
                        }
                    }
                    if let (Some(t), Some(pos)) = (t, pos) {
                        positions.push((t, pos));
                    }
                }
                _ => {}
            }
        }
        let Some(time) = time else { continue };
        for (_, pos) in positions.into_iter().filter(|&(t, _)| t == track) {
            if out.len() as u64 >= max {
                return Err(limit("maxSamples", out.len() as u64 + 1, max));
            }
            out.push(Cue { time, cluster: pos });
        }
    }
    Ok(out)
}

/// Marks the frames the cues name as keyframes: the frame at the cue's time in the cluster at
/// the cue's position (relative to the Segment's body). Keyframes are the block flags plus
/// these, so a file whose muxer left the flags unset still has sync points; a cue that names
/// no scanned cluster or no frame is ignored. Returns how many cues named a frame.
pub(super) fn mark(
    cues: &[Cue],
    clusters: &[Element],
    segment_body: u64,
    frames: &mut [RawFrame],
) -> usize {
    let mut at: HashMap<(u32, i64), usize> = HashMap::new();
    for (i, f) in frames.iter().enumerate() {
        at.entry((f.cluster, f.ts)).or_insert(i);
    }
    let mut named = 0;
    for c in cues {
        let index = segment_body
            .checked_add(c.cluster)
            .and_then(|pos| clusters.binary_search_by_key(&pos, |e| e.start).ok());
        let frame = index
            .zip(i64::try_from(c.time).ok())
            .and_then(|(i, t)| at.get(&(i as u32, t)));
        if let Some(&i) = frame {
            frames[i].key = true;
            named += 1;
        }
    }
    named
}
