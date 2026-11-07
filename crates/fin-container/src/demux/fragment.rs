//! Fragmented files (`mvex`/`trex`, `moof`/`traf`/`tfhd`/`tfdt`/`trun`): each fragment's
//! samples are appended to their track's sample list in decode order.

use std::io::{Read, Seek};

use fin_core::CoreError;
use mp4_atom::{Tfdt, Tfhd, Trex, Trun, TrunSamples};

use super::reader::{BoxHeader, Reader, be_u32, check_count, decode};
use super::table::Sample;
use crate::error::{corrupt, limit};

/// A track's fragment defaults from `trex`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Defaults {
    pub track_id: u32,
    pub duration: u32,
    pub size: u32,
    pub flags: u32,
}

pub(crate) fn trex(body: &[u8], h: &BoxHeader) -> Result<Defaults, CoreError> {
    let t = decode::<Trex>(body, h)?;
    Ok(Defaults {
        track_id: t.track_id,
        duration: t.default_sample_duration,
        size: t.default_sample_size,
        flags: t.default_sample_flags,
    })
}

const NON_SYNC: u32 = 0x0001_0000;

/// A trun entry's optional duration, size, flags and composition offset.
type Fields = (Option<u32>, Option<u32>, Option<u32>, Option<i64>);

/// A `trun`'s declared count against its size: 4 bytes for each per-sample field present.
fn check_trun(body: &[u8], h: &BoxHeader) -> Result<(), CoreError> {
    let flags = be_u32(body, 0)
        .ok_or_else(|| corrupt("the trun box is cut short", h.start, "trun"))?
        & 0x00ff_ffff;
    let count = be_u32(body, 4)
        .ok_or_else(|| corrupt("the trun box is cut short", h.start, "trun"))?
        as u64;
    let head = 8 + 4 * ((flags & 0x1 != 0) as usize + (flags & 0x4 != 0) as usize);
    let per = 4 * (flags & 0xf00).count_ones() as u64;
    check_count(body, head, count, per, h)
}

/// Appends the samples of fragment `moof` to `tracks` (`(track id, samples)` pairs); tracks
/// the fragment names but the file does not are skipped.
pub(crate) fn append<R: Read + Seek>(
    rd: &mut Reader<R>,
    moof: &BoxHeader,
    defaults: &[Defaults],
    tracks: &mut [(u32, &mut Vec<Sample>)],
    depth: u8,
) -> Result<(), CoreError> {
    let max = rd.limits.max_samples as u64;
    for traf in rd.children(moof.body, moof.end, depth + 1)? {
        if &traf.kind != b"traf" {
            continue;
        }
        let parts = rd.children(traf.body, traf.end, depth + 2)?;
        let tfhd_h = parts
            .iter()
            .find(|p| &p.kind == b"tfhd")
            .ok_or_else(|| corrupt("a traf has no tfhd", traf.start, "traf"))?;
        let tfhd = decode::<Tfhd>(&rd.body(tfhd_h)?, tfhd_h)?;
        let Some(k) = tracks.iter().position(|(id, _)| *id == tfhd.track_id) else {
            continue;
        };
        let d = defaults
            .iter()
            .find(|d| d.track_id == tfhd.track_id)
            .copied()
            .unwrap_or(Defaults {
                track_id: tfhd.track_id,
                duration: 0,
                size: 0,
                flags: 0,
            });
        let samples = &mut *tracks[k].1;
        let mut dts = samples.last().map_or(0, |s| s.dts + s.duration as i64);
        if let Some(h) = parts.iter().find(|p| &p.kind == b"tfdt") {
            dts = i64::try_from(decode::<Tfdt>(&rd.body(h)?, h)?.base_media_decode_time)
                .map_err(|_| corrupt("tfdt is out of range", h.start, "tfdt"))?;
        }
        let base = tfhd.base_data_offset.unwrap_or(moof.start);
        let mut at = base;
        for h in parts.iter().filter(|p| &p.kind == b"trun") {
            let body = rd.body(h)?;
            check_trun(&body, h)?;
            let trun = decode::<Trun>(&body, h)?;
            if let Some(off) = trun.data_offset {
                at = base.checked_add_signed(off as i64).ok_or_else(|| {
                    corrupt("a trun data offset is out of range", h.start, "trun")
                })?;
            }
            let entries: Vec<Fields> = match &trun.samples {
                TrunSamples::Explicit(e) => e
                    .iter()
                    .map(|e| (e.duration, e.size, e.flags, e.cts))
                    .collect(),
                TrunSamples::Inherited { count, first_flags } => {
                    if samples.len() as u64 + *count as u64 > max {
                        return Err(limit(
                            "maxSamples",
                            samples.len() as u64 + *count as u64,
                            max,
                        ));
                    }
                    (0..*count)
                        .map(|i| (None, None, if i == 0 { *first_flags } else { None }, None))
                        .collect()
                }
            };
            if samples.len() as u64 + entries.len() as u64 > max {
                return Err(limit(
                    "maxSamples",
                    samples.len() as u64 + entries.len() as u64,
                    max,
                ));
            }
            for (duration, size, flags, cts) in entries {
                let duration = duration
                    .or(tfhd.default_sample_duration)
                    .unwrap_or(d.duration);
                let size = size.or(tfhd.default_sample_size).unwrap_or(d.size);
                let flags = flags.or(tfhd.default_sample_flags).unwrap_or(d.flags);
                if at.saturating_add(size as u64) > rd.len {
                    return Err(corrupt(
                        format!("a fragment sample at {at} runs past the end of the file"),
                        at,
                        "trun",
                    ));
                }
                samples.push(Sample {
                    dts,
                    cto: cts.unwrap_or(0),
                    duration,
                    size,
                    offset: at,
                    key: flags & NON_SYNC == 0,
                });
                dts += duration as i64;
                at += size as u64;
            }
        }
    }
    Ok(())
}
