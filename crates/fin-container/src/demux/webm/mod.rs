//! The WebM/Matroska demuxer (spec §6.1, §6.3; 6B-1 Task 12): the first video and the first
//! audio track of the first `Segment`, as the same `SampleTable`, `VideoTrack`, `AudioTrack`
//! and `MediaInfo` as MP4, so `frame_at`, `FrameReader` and probing need nothing new.
//!
//! Stages: [`ebml`] (element headers under the limits), [`segment`] (the EBML header, the
//! Segment's layout, `Info` and the track entries), [`codec_private`] (`CodecID`,
//! `CodecPrivate` and `Colour` as a sample entry), [`cluster`] (blocks and lacing), [`cues`]
//! (sync points from the index). Times: a video track's timescale is 10⁹ / gcd(10⁹,
//! `TimestampScale`) ticks a second, so block times convert exactly; an audio track's is its
//! sampling rate. Matroska stores presentation times in decode order, so decode times are the
//! sorted presentation times.

mod cluster;
mod codec_private;
mod cues;
mod ebml;
mod segment;

use std::io::{Read, Seek, SeekFrom};

use fin_core::{CoreError, Rational};
use fin_media::{
    AudioCodec, AudioTrackInfo, IgnoredTrack, MediaInfo, Rotation, SampleTable, VideoCodec,
    VideoTrackInfo, display_size,
};

use self::cluster::{RawFrame, Want};
use self::ebml::{EBML, Ebml};
use self::segment::TrackDesc;
use super::table::Sample;
use super::{AudioTrack, CodecParams, Parsed, VideoTrack, edit_list};
use crate::error::{content_encoding, corrupt, io};
use crate::{Limits, TrackRef};

const NS: u64 = 1_000_000_000;

/// True when the reader starts with the EBML magic (the reader is left at 0).
pub(super) fn sniff<R: Read + Seek>(r: &mut R) -> Result<bool, CoreError> {
    let mut head = Vec::with_capacity(4);
    r.seek(SeekFrom::Start(0))
        .and_then(|_| r.by_ref().take(4).read_to_end(&mut head))
        .and_then(|_| r.seek(SeekFrom::Start(0)))
        .map_err(|e| io(&e))?;
    Ok(head == EBML.to_be_bytes())
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

fn handler(kind: u64) -> String {
    match kind {
        1 => "vide",
        2 => "soun",
        0x11 => "subt",
        0x12 => "butn",
        0x10 => "logo",
        0x20 => "ctrl",
        _ => "meta",
    }
    .to_string()
}

fn track_ref(t: &TrackDesc) -> Result<TrackRef, CoreError> {
    u32::try_from(t.number)
        .map(TrackRef)
        .map_err(|_| corrupt("a TrackNumber is over 2³²", t.at, "TrackEntry"))
}

pub(super) fn parse<R: Read + Seek>(r: &mut R, limits: Limits) -> Result<Parsed, CoreError> {
    let mut eb = Ebml::new(r, limits)?;
    let l = segment::layout(&mut eb)?;
    let format = segment::format_of(l.container);
    let vt = l.tracks.iter().find(|t| t.kind == 1);
    let at = l.tracks.iter().find(|t| t.kind == 2);
    for t in [vt, at].into_iter().flatten() {
        if t.encoded {
            return Err(content_encoding(format));
        }
    }
    let ignored = l
        .tracks
        .iter()
        .filter(|t| Some(*t) != vt && Some(*t) != at)
        .map(|t| IgnoredTrack {
            id: t.number.min(u32::MAX as u64) as u32,
            handler: handler(t.kind),
        })
        .collect();
    // Refusals that need no scan come first (a projection, an oversized number).
    let ventry = vt.map(codec_private::video).transpose()?;
    let (vref, aref) = (
        vt.map(track_ref).transpose()?,
        at.map(track_ref).transpose()?,
    );
    let want = Want {
        video: vt.map(|t| t.number),
        audio: at.map(|t| t.number),
        format,
    };
    let mut scanned = cluster::scan(&mut eb, &l.clusters, &want)?;
    if let (Some(c), Some(v)) = (l.cues, vt) {
        let list = cues::read(&mut eb, &c, v.number)?;
        cues::mark(&list, &l.clusters, l.segment_body, &mut scanned.video);
    }

    let g = gcd(l.scale, NS);
    let (timescale, mul) = ((NS / g) as u32, l.scale / g);
    let mut duration = Rational::zero();
    let (mut video, mut video_info) = (None, None);
    if let (Some(t), Some(e), Some(track)) = (vt, ventry, vref) {
        let samples = video_samples(&scanned.video, t, timescale, mul)?;
        let (frames, sync) = edit_list::present(
            &samples,
            &[],
            timescale,
            timescale,
            limits.max_samples as u64,
        )?;
        let table = SampleTable::with_sync(timescale, frames, sync)?;
        let pixel_aspect = e.pasp.map_or(Rational::from_int(1), |(h, v)| {
            Rational::new(h as i64, v as i64).unwrap_or(Rational::from_int(1))
        });
        let coded = e.coded.unwrap_or_default();
        let (tags, icc) = super::colour(&e);
        let (nominal_rate, vfr) = rates(&table, t.default_duration);
        duration = duration.max(Rational::new(table.end_ticks().max(0), timescale as i64)?);
        let codec = VideoCodec::from_fourcc(&e.fourcc);
        video_info = Some(VideoTrackInfo {
            codec,
            fourcc: String::from_utf8_lossy(&e.fourcc).into_owned(),
            profile: e.profile,
            level: e.level,
            bit_depth: e.bit_depth,
            chroma: e.chroma,
            coded,
            display: display_size(coded, Rotation::R0, pixel_aspect),
            rotation: Rotation::R0,
            pixel_aspect,
            colour: tags,
            icc,
            hdr: (!e.hdr.is_empty()).then_some(e.hdr),
            nominal_rate,
            vfr,
            frames: table.len() as u64,
        });
        video = Some(VideoTrack {
            track,
            table,
            config: e.config,
            params: CodecParams {
                codec,
                fourcc: e.fourcc,
                profile: e.profile,
                level: e.level,
                bit_depth: e.bit_depth,
                chroma: e.chroma,
            },
            samples,
        });
    }
    let (mut audio, mut audio_info) = (None, None);
    if let (Some(t), Some(track)) = (at, aref) {
        let e = codec_private::audio(t);
        let (channels, rate) = e.audio.unwrap_or((0, 0));
        let ats = if rate > 0 { rate } else { timescale };
        let samples = audio_samples(&scanned.audio, t, l.scale, ats)?;
        let to = |ns: u64| (ns as u128 * ats as u128 / NS as u128).min(u32::MAX as u128) as u64;
        let priming = match e.pre_skip {
            Some(p) => p as u64 * ats as u64 / 48_000,
            None => to(t.codec_delay),
        };
        let padding = scanned
            .audio
            .last()
            .and_then(|f| f.padding)
            .map_or(0, |p| to(p.max(0) as u64));
        let total: u64 = samples.iter().map(|s| s.duration as u64).sum();
        let shown = total.saturating_sub(priming).saturating_sub(padding);
        duration = duration.max(Rational::new(
            shown.min(i64::MAX as u64) as i64,
            ats as i64,
        )?);
        audio_info = Some(AudioTrackInfo {
            codec: AudioCodec::from_fourcc(&e.fourcc),
            fourcc: String::from_utf8_lossy(&e.fourcc).into_owned(),
            sample_rate: rate,
            channels,
            priming: priming.min(u32::MAX as u64) as u32,
            padding: padding.min(u32::MAX as u64) as u32,
        });
        audio = Some(AudioTrack {
            track,
            timescale: ats,
            config: e.config,
            samples,
        });
    }
    let info = MediaInfo {
        container: l.container,
        duration,
        fragmented: false,
        video: video_info,
        audio: audio_info,
        ignored,
    };
    Ok((info, video, audio))
}

/// Video frames in decode order with presentation times in `timescale` ticks (`mul` ticks a
/// Segment tick): decode times are the sorted presentation times, durations the gap to the
/// next presentation time (the last frame's: `DefaultDuration`, its `BlockDuration`, else the
/// gap before it).
fn video_samples(
    raw: &[RawFrame],
    t: &TrackDesc,
    timescale: u32,
    mul: u64,
) -> Result<Vec<Sample>, CoreError> {
    let overflow = || corrupt("a block time overflows", t.at, "Cluster");
    let mul = i64::try_from(mul).map_err(|_| overflow())?;
    let pts: Vec<i64> = raw
        .iter()
        .map(|f| f.ts.checked_mul(mul).ok_or_else(overflow))
        .collect::<Result<_, _>>()?;
    let mut sorted = pts.clone();
    sorted.sort_unstable();
    let last_gap = sorted
        .windows(2)
        .last()
        .and_then(|w| w[1].checked_sub(w[0]));
    let tail = t
        .default_duration
        .map(|ns| (ns as u128 * timescale as u128 / NS as u128) as i64)
        .filter(|&d| d > 0);
    let mut out = Vec::with_capacity(raw.len());
    for (i, f) in raw.iter().enumerate() {
        let next = sorted.partition_point(|&p| p <= pts[i]);
        let d = match sorted.get(next) {
            Some(&n) => n.checked_sub(pts[i]).ok_or_else(overflow)?,
            None => tail
                .or(f.duration.and_then(|d| (d as i64).checked_mul(mul)))
                .or(last_gap)
                .unwrap_or(1),
        };
        let d = u32::try_from(d)
            .map_err(|_| corrupt("a frame lasts over 2³² ticks", t.at, "Cluster"))?;
        let s = Sample::checked(
            sorted[i],
            pts[i].checked_sub(sorted[i]).ok_or_else(overflow)?,
            d,
            f.size,
            f.offset,
            f.key,
        )
        .ok_or_else(overflow)?;
        out.push(s);
    }
    Ok(out)
}

/// Audio frames at `rate` samples a second: a block's frames share its time, so a laced
/// block's span (to the next block; for the last, `DefaultDuration` a frame, its
/// `BlockDuration`, else the previous block's frame length) is divided between them.
fn audio_samples(
    raw: &[RawFrame],
    t: &TrackDesc,
    scale: u64,
    rate: u32,
) -> Result<Vec<Sample>, CoreError> {
    let overflow = || corrupt("a block time overflows", t.at, "Cluster");
    let at = |ts: i64| -> Result<i64, CoreError> {
        let v = (ts as i128)
            .checked_mul(scale as i128)
            .and_then(|v| v.checked_mul(rate as i128))
            .ok_or_else(overflow)?;
        i64::try_from(v / NS as i128).map_err(|_| overflow())
    };
    // Block starts: (index of its first frame, frame count, start in samples).
    let mut blocks: Vec<(usize, usize, i64)> = Vec::new();
    for (i, f) in raw.iter().enumerate() {
        if f.lace.0 == 0 || blocks.is_empty() {
            blocks.push((i, 1, at(f.ts)?));
        } else {
            blocks.last_mut().expect("a block").1 += 1;
        }
    }
    let per_frame_default = t
        .default_duration
        .map(|ns| (ns as u128 * rate as u128 / NS as u128) as i64);
    let mut out = Vec::with_capacity(raw.len());
    let mut previous = 0i64;
    for (b, &(first, n, start)) in blocks.iter().enumerate() {
        let span = match blocks.get(b + 1) {
            Some(&(_, _, next)) => next.saturating_sub(start).max(0),
            None => per_frame_default
                .map(|d| d.saturating_mul(n as i64))
                .or(raw[first]
                    .duration
                    .map(|d| at(d.min(i64::MAX as u64) as i64))
                    .transpose()?)
                .unwrap_or(previous.saturating_mul(n as i64)),
        };
        let (each, extra) = (span / n as i64, span % n as i64);
        previous = each;
        let mut s = start;
        for k in 0..n {
            let d = each + i64::from((k as i64) < extra);
            let f = &raw[first + k];
            let d32 = u32::try_from(d).map_err(|_| overflow())?;
            out.push(Sample::checked(s, 0, d32, f.size, f.offset, true).ok_or_else(overflow)?);
            s = s.checked_add(d).ok_or_else(overflow)?;
        }
    }
    Ok(out)
}

/// The nominal rate and whether the source is VFR. Block times are rounded to the
/// `TimestampScale`, so durations a tick apart are the same duration: a CFR clip in 1 ms ticks
/// at 30 fps has 33 and 34 ms frames. The rate is `DefaultDuration`'s, else the mean frame
/// length's (CFR) or the most common one's (VFR), snapped to a standard rate within 0.05 %.
fn rates(table: &SampleTable, default_duration: Option<u64>) -> (Rational, bool) {
    let f = table.frames();
    let ts = table.timescale() as i64;
    let body: Vec<i64> = f[..f.len().saturating_sub(1).max(1)]
        .iter()
        .map(|e| e.duration as i64)
        .filter(|&d| d > 0)
        .collect();
    let (min, max) = (
        body.iter().copied().min().unwrap_or(1),
        body.iter().copied().max().unwrap_or(1),
    );
    let vfr = max - min > 1 && (max - min) * 1000 > min;
    let (num, den) = match default_duration.filter(|&d| d <= i64::MAX as u64) {
        Some(dd) => (NS as i64, dd as i64),
        None if !vfr && f.len() > 1 => (
            (f.len() as i64 - 1) * ts,
            f[f.len() - 1].pts.saturating_sub(f[0].pts).max(1),
        ),
        None => {
            let mut sorted = body.clone();
            sorted.sort_unstable();
            let mut best = (0usize, min.max(1));
            let mut k = 0;
            while k < sorted.len() {
                let run = sorted[k..].iter().take_while(|&&d| d == sorted[k]).count();
                if run > best.0 {
                    best = (run, sorted[k]);
                }
                k += run;
            }
            (ts, best.1)
        }
    };
    const STANDARD: [(i64, i64); 12] = [
        (24_000, 1001),
        (24, 1),
        (25, 1),
        (30_000, 1001),
        (30, 1),
        (48, 1),
        (50, 1),
        (60_000, 1001),
        (60, 1),
        (100, 1),
        (120, 1),
        (240, 1),
    ];
    for (sn, sd) in STANDARD {
        let (a, b) = (num as i128 * sd as i128, sn as i128 * den as i128);
        if (a - b).abs() * 2000 < b {
            return (Rational::new(sn, sd).expect("a rate"), vfr);
        }
    }
    (
        Rational::new(num, den).unwrap_or(Rational::from_int(0)),
        vfr,
    )
}
