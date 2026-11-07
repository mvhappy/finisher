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

/// Appends the samples of fragment `moof` to `tracks` (`(track id, samples, is video)`); tracks
/// the fragment names but the file does not are skipped. A track holds at most
/// `min(max_samples, file length)` samples, a video sample at least one byte.
pub(crate) fn append<R: Read + Seek>(
    rd: &mut Reader<R>,
    moof: &BoxHeader,
    defaults: &[Defaults],
    tracks: &mut [(u32, &mut Vec<Sample>, bool)],
    depth: u8,
) -> Result<(), CoreError> {
    let max = (rd.limits.max_samples as u64).min(rd.len);
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
        let Some(k) = tracks.iter().position(|(id, _, _)| *id == tfhd.track_id) else {
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
        let video = tracks[k].2;
        let samples = &mut *tracks[k].1;
        let mut dts = match samples.last() {
            Some(s) => s.end(),
            None => 0,
        };
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
            let count = match &trun.samples {
                TrunSamples::Explicit(e) => e.len() as u64,
                TrunSamples::Inherited { count, .. } => u64::from(*count),
            };
            // A run without per-sample fields costs no bytes a sample, so a few bytes could
            // declare millions: samples are capped by the file's length too.
            if samples.len() as u64 + count > max {
                return Err(limit("maxSamples", samples.len() as u64 + count, max));
            }
            for i in 0..count as usize {
                let (duration, size, flags, cts) = match &trun.samples {
                    TrunSamples::Explicit(e) => (e[i].duration, e[i].size, e[i].flags, e[i].cts),
                    TrunSamples::Inherited { first_flags, .. } => {
                        (None, None, if i == 0 { *first_flags } else { None }, None)
                    }
                };
                let duration = duration
                    .or(tfhd.default_sample_duration)
                    .unwrap_or(d.duration);
                let size = size.or(tfhd.default_sample_size).unwrap_or(d.size);
                let flags = flags.or(tfhd.default_sample_flags).unwrap_or(d.flags);
                if video && size == 0 {
                    return Err(corrupt(
                        "a fragment video sample has no bytes",
                        h.start,
                        "trun",
                    ));
                }
                if at.saturating_add(size as u64) > rd.len {
                    return Err(corrupt(
                        format!("a fragment sample at {at} runs past the end of the file"),
                        at,
                        "trun",
                    ));
                }
                let s = Sample::checked(
                    dts,
                    cts.unwrap_or(0),
                    duration,
                    size,
                    at,
                    flags & NON_SYNC == 0,
                )
                .ok_or_else(|| corrupt("fragment times overflow", h.start, "trun"))?;
                dts = s.dts + i64::from(duration);
                samples.push(s);
                at += size as u64;
            }
        }
    }
    Ok(())
}
