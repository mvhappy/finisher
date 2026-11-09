//! The MP4/MOV demuxer (spec §6.1, §6.3): `mp4-atom` decodes the boxes; which frame is shown,
//! what the file is, and what a hostile file can make us allocate are decided here. The first
//! video and the first audio track are read; other tracks are listed in `MediaInfo.ignored`.
//! A file that starts with the EBML magic is read by the WebM/Matroska demuxer ([`webm`]) into
//! the same tracks.

mod codec_config;
mod colour;
mod edit_list;
mod fragment;
mod matrix;
mod reader;
mod table;
mod webm;

use std::io::{Read, Seek, SeekFrom};

use fin_core::{CoreError, Rational};
use fin_media::{
    AudioCodec, AudioTrackInfo, Chroma, ColourTags, ContainerKind, IgnoredTrack, MediaInfo,
    Rotation, SampleTable, VideoCodec, VideoTrackInfo, display_size,
};
use mp4_atom::{Ftyp, Mdhd, Mvhd, Tkhd};
use serde_json::json;

use self::codec_config::Entry;
use self::colour::ColrInfo;
use self::edit_list::Edit;
use self::reader::{BoxHeader, Reader, be_u32, decode};
use self::table::{Boxes, Sample};
use crate::error::{corrupt, invalid, io, limit, unsupported_container};
use crate::{CodecConfig, Limits, TrackRef};

/// One sample in decode order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleInfo {
    /// Decode time and composition time in the track's timescale, before edit lists.
    pub dts: i64,
    pub cts: i64,
    pub duration: u32,
    pub size: u32,
    pub offset: u64,
    pub keyframe: bool,
}

impl From<&Sample> for SampleInfo {
    fn from(s: &Sample) -> Self {
        SampleInfo {
            dts: s.dts,
            cts: s.ct(),
            duration: s.duration,
            size: s.size,
            offset: s.offset,
            keyframe: s.key,
        }
    }
}

/// The first video track: presented frames (`SampleTable`), decode-order samples, and the
/// codec configuration for the decoder.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoTrack {
    pub track: TrackRef,
    pub table: SampleTable,
    /// The configuration record as bytes (`avcC`, `hvcC`, `av1C`, `vpcC`; ProRes and other
    /// codecs without one are `Other {fourcc}`), for the decoder.
    pub config: CodecConfig,
    /// What the sample entry and its record say (6B-1 Task 3: a backend is chosen by codec,
    /// profile and bit depth).
    pub params: CodecParams,
    samples: Vec<Sample>,
}

/// A video track's codec as its sample entry and configuration record describe it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodecParams {
    pub codec: VideoCodec,
    /// The sample entry type (`avc1`, `hvc1`, `apch`, …).
    pub fourcc: [u8; 4],
    /// The record's profile number: `profile_idc` (H.264), `general_profile_idc` (HEVC),
    /// `seq_profile` (AV1), the VP9 profile; absent without a record.
    pub profile: Option<u8>,
    pub level: Option<u8>,
    pub bit_depth: u8,
    pub chroma: Chroma,
}

impl VideoTrack {
    /// Samples in decode order (presented or not).
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn sample(&self, decode_index: usize) -> Option<SampleInfo> {
        self.samples.get(decode_index).map(SampleInfo::from)
    }
}

/// The first audio track.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub track: TrackRef,
    pub timescale: u32,
    pub config: CodecConfig,
    samples: Vec<Sample>,
}

impl AudioTrack {
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    pub fn sample(&self, decode_index: usize) -> Option<SampleInfo> {
        self.samples.get(decode_index).map(SampleInfo::from)
    }
}

/// A track's boxes as found in `moov`.
struct Trak {
    id: u32,
    handler: [u8; 4],
    matrix: [i32; 9],
    timescale: u32,
    stbl: Option<BoxHeader>,
    boxes: Boxes,
    edits: Vec<Edit>,
    samples: Vec<Sample>,
}

const TOP: [&[u8; 4]; 12] = [
    b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot", b"uuid", b"moof", b"mfra",
    b"sidx", b"styp",
];

pub struct Demuxer<R: Read + Seek> {
    r: R,
    limits: Limits,
    info: MediaInfo,
    video: Option<VideoTrack>,
    audio: Option<AudioTrack>,
}

fn child<'a>(boxes: &'a [BoxHeader], kind: &[u8; 4]) -> Option<&'a BoxHeader> {
    boxes.iter().find(|b| &b.kind == kind)
}

impl<R: Read + Seek> Demuxer<R> {
    /// Parses the container: every malformed input is `MEDIA_CORRUPT`, `MEDIA_LIMIT`,
    /// `UNSUPPORTED_CONTAINER` or `UNSUPPORTED_TRANSFORM`, never a panic.
    pub fn open(mut r: R, limits: &Limits) -> Result<Self, CoreError> {
        let (info, video, audio) = parse(&mut r, *limits)?;
        Ok(Demuxer {
            r,
            limits: *limits,
            info,
            video,
            audio,
        })
    }

    pub fn info(&self) -> &MediaInfo {
        &self.info
    }

    pub fn video(&self) -> Option<&VideoTrack> {
        self.video.as_ref()
    }

    pub fn audio(&self) -> Option<&AudioTrack> {
        self.audio.as_ref()
    }

    /// Reads sample `decode_index` of `track` into `buf` (replacing its contents).
    pub fn read_sample(
        &mut self,
        track: TrackRef,
        decode_index: u32,
        buf: &mut Vec<u8>,
    ) -> Result<(), CoreError> {
        let samples = match (&self.video, &self.audio) {
            (Some(v), _) if v.track == track => &v.samples,
            (_, Some(a)) if a.track == track => &a.samples,
            _ => return Err(invalid("no such track", json!({ "track": track.0 }))),
        };
        let s = *samples.get(decode_index as usize).ok_or_else(|| {
            invalid(
                "no such sample",
                json!({ "track": track.0, "decodeIndex": decode_index }),
            )
        })?;
        if s.size as u64 > self.limits.max_box_bytes {
            return Err(limit(
                "maxBoxBytes",
                s.size as u64,
                self.limits.max_box_bytes,
            ));
        }
        buf.resize(s.size as usize, 0);
        self.r.seek(SeekFrom::Start(s.offset)).map_err(|e| io(&e))?;
        self.r.read_exact(buf).map_err(|e| match e.kind() {
            std::io::ErrorKind::UnexpectedEof => corrupt(
                format!("sample {decode_index} is cut short"),
                s.offset,
                "mdat",
            ),
            _ => io(&e),
        })
    }
}

fn parse_trak<R: Read + Seek>(rd: &mut Reader<R>, trak: &BoxHeader) -> Result<Trak, CoreError> {
    let parts = rd.children(trak.body, trak.end, 2)?;
    let tkhd_h =
        child(&parts, b"tkhd").ok_or_else(|| corrupt("a trak has no tkhd", trak.start, "trak"))?;
    let tkhd = decode::<Tkhd>(&rd.body(tkhd_h)?, tkhd_h)?;
    let m = &tkhd.matrix;
    let mut t = Trak {
        id: tkhd.track_id,
        handler: *b"    ",
        matrix: [m.a, m.b, m.u, m.c, m.d, m.v, m.x, m.y, m.w],
        timescale: 0,
        stbl: None,
        boxes: Boxes::default(),
        edits: Vec::new(),
        samples: Vec::new(),
    };
    if let Some(edts) = child(&parts, b"edts") {
        let inner = rd.children(edts.body, edts.end, 3)?;
        if let Some(elst) = child(&inner, b"elst") {
            t.edits = edit_list::parse(&rd.body(elst)?, elst)?;
        }
    }
    let mdia =
        *child(&parts, b"mdia").ok_or_else(|| corrupt("a trak has no mdia", trak.start, "trak"))?;
    let mdia_parts = rd.children(mdia.body, mdia.end, 3)?;
    if let Some(h) = child(&mdia_parts, b"mdhd") {
        t.timescale = decode::<Mdhd>(&rd.body(h)?, h)?.timescale;
    }
    if let Some(h) = child(&mdia_parts, b"hdlr") {
        // pre_defined (QuickTime: the component type), then the handler type.
        let body = rd.body(h)?;
        t.handler = body
            .get(8..12)
            .and_then(|b| b.try_into().ok())
            .unwrap_or(*b"    ");
    }
    if let Some(minf) = child(&mdia_parts, b"minf").copied() {
        let minf_parts = rd.children(minf.body, minf.end, 4)?;
        if let Some(stbl) = child(&minf_parts, b"stbl").copied() {
            for b in rd.children(stbl.body, stbl.end, 5)? {
                t.boxes.set(b);
            }
            t.stbl = Some(stbl);
        }
    }
    Ok(t)
}

/// `(tags, icc)` of a visual entry: `colr`, else the codec configuration's, else untagged.
fn colour(e: &Entry) -> (ColourTags, bool) {
    let (mut tags, icc) = match (e.colr, e.config_colour) {
        (Some(ColrInfo::Tags(t)), _) => (t, false),
        (Some(ColrInfo::Icc), c) => (c.unwrap_or_default(), true),
        (None, c) => (c.unwrap_or_default(), false),
    };
    tags.siting = e.siting;
    (tags, icc)
}

/// The most common frame duration's rate, and whether durations differ by more than 0.1 %
/// (the last frame ignored).
fn rates(table: &SampleTable) -> (Rational, bool) {
    let f = table.frames();
    let body: Vec<u32> = f[..f.len().saturating_sub(1).max(1)]
        .iter()
        .map(|e| e.duration)
        .filter(|&d| d > 0)
        .collect();
    let (min, max) = (
        body.iter().copied().min().unwrap_or(1),
        body.iter().copied().max().unwrap_or(1),
    );
    let vfr = (max - min) as u64 * 1000 > min as u64;
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
    (
        Rational::new(table.timescale() as i64, best.1 as i64).unwrap_or(Rational::from_int(0)),
        vfr,
    )
}

/// `sgpd` `roll` distance (in samples of one AAC frame), if any.
fn roll(rd: &mut Reader<impl Read + Seek>, b: &Boxes) -> Result<Option<i16>, CoreError> {
    let Some(h) = &b.sgpd else { return Ok(None) };
    let body = rd.body(h)?;
    if body.get(4..8) != Some(b"roll") || body.first() != Some(&1) {
        return Ok(None);
    }
    // Version 1: default_length, entry_count, then (description_length if 0) roll_distance.
    let len = be_u32(&body, 8).unwrap_or(0);
    let at = if len == 0 { 20 } else { 16 };
    Ok(body
        .get(at..at + 2)
        .map(|b| i16::from_be_bytes([b[0], b[1]])))
}

type Parsed = (MediaInfo, Option<VideoTrack>, Option<AudioTrack>);

fn parse<R: Read + Seek>(r: &mut R, limits: Limits) -> Result<Parsed, CoreError> {
    if webm::sniff(r)? {
        return webm::parse(r, limits);
    }
    let mut rd = Reader::new(r, limits)?;
    if rd.len < 8 {
        return Err(unsupported_container(
            "unknown",
            "the file is too short to be MP4 or MOV",
        ));
    }
    let mut head = [0u8; 8];
    rd.read_at(0, &mut head)?;
    if !TOP.iter().any(|k| head[4..8] == **k) {
        return Err(unsupported_container(
            "unknown",
            "not an ISO base media (MP4/MOV) file",
        ));
    }
    let top = rd.children(0, rd.len, 0)?;
    let mut container = ContainerKind::Mp4;
    if let Some(h) = child(&top, b"ftyp") {
        let ftyp = decode::<Ftyp>(&rd.body(h)?, h)?;
        if ftyp.major_brand.as_ref() == b"qt  " {
            container = ContainerKind::Mov;
        }
    } else {
        container = ContainerKind::Mov;
    }
    let moov =
        *child(&top, b"moov").ok_or_else(|| corrupt("the file has no moov box", 0, "moov"))?;
    let moov_parts = rd.children(moov.body, moov.end, 1)?;
    let mvhd_h = child(&moov_parts, b"mvhd")
        .ok_or_else(|| corrupt("moov has no mvhd", moov.start, "moov"))?;
    let movie_ts = decode::<Mvhd>(&rd.body(mvhd_h)?, mvhd_h)?.timescale;
    let mut defaults = Vec::new();
    if let Some(mvex) = child(&moov_parts, b"mvex").copied() {
        for h in rd
            .children(mvex.body, mvex.end, 2)?
            .iter()
            .filter(|h| &h.kind == b"trex")
        {
            defaults.push(fragment::trex(&rd.body(h)?, h)?);
        }
    }
    let mut traks = Vec::new();
    for h in moov_parts.iter().filter(|h| &h.kind == b"trak") {
        traks.push(parse_trak(&mut rd, h)?);
    }
    let vi = traks.iter().position(|t| &t.handler == b"vide");
    let ai = traks.iter().position(|t| &t.handler == b"soun");
    let ignored = traks
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != vi && Some(*i) != ai)
        .map(|(_, t)| IgnoredTrack {
            id: t.id,
            handler: String::from_utf8_lossy(&t.handler).trim_end().to_string(),
        })
        .collect();
    for i in [vi, ai].into_iter().flatten() {
        let t = &mut traks[i];
        if let Some(stbl) = t.stbl {
            t.samples = table::expand(&mut rd, &t.boxes, &stbl)?;
        }
    }
    let moofs: Vec<BoxHeader> = top.iter().filter(|h| &h.kind == b"moof").copied().collect();
    let fragmented = !moofs.is_empty() || child(&moov_parts, b"mvex").is_some();
    for moof in &moofs {
        let mut chosen: Vec<(u32, &mut Vec<Sample>, bool)> = traks
            .iter_mut()
            .enumerate()
            .filter(|(i, _)| Some(*i) == vi || Some(*i) == ai)
            .map(|(i, t)| (t.id, &mut t.samples, Some(i) == vi))
            .collect();
        fragment::append(&mut rd, moof, &defaults, &mut chosen, 1)?;
    }

    let mut duration = Rational::zero();
    let mut video = None;
    let mut video_info = None;
    if let Some(i) = vi {
        let samples = std::mem::take(&mut traks[i].samples);
        let t = &traks[i];
        let stsd = t.boxes.stsd.ok_or_else(|| {
            corrupt(
                "the video track has no stsd",
                t.stbl.map_or(0, |s| s.start),
                "stbl",
            )
        })?;
        let e = codec_config::entry(&mut rd, &stsd, true, 6)?;
        let (frames, sync) = edit_list::present(
            &samples,
            &t.edits,
            t.timescale,
            movie_ts,
            limits.max_samples as u64,
        )?;
        let table = SampleTable::with_sync(t.timescale, frames, sync)?;
        let rotation: Rotation = matrix::rotation(t.matrix)?;
        let pixel_aspect = e.pasp.map_or(Rational::from_int(1), |(h, v)| {
            Rational::new(h as i64, v as i64).unwrap_or(Rational::from_int(1))
        });
        let coded = e.coded.unwrap_or_default();
        let (tags, icc) = colour(&e);
        let (nominal_rate, vfr) = rates(&table);
        duration = duration.max(Rational::new(table.end_ticks().max(0), t.timescale as i64)?);
        video_info = Some(VideoTrackInfo {
            codec: VideoCodec::from_fourcc(&e.fourcc),
            fourcc: String::from_utf8_lossy(&e.fourcc).into_owned(),
            profile: e.profile,
            level: e.level,
            bit_depth: e.bit_depth,
            chroma: e.chroma,
            coded,
            display: display_size(coded, rotation, pixel_aspect),
            rotation,
            pixel_aspect,
            colour: tags,
            icc,
            hdr: (!e.hdr.is_empty()).then_some(e.hdr),
            nominal_rate,
            vfr,
            frames: table.len() as u64,
        });
        video = Some(VideoTrack {
            track: TrackRef(t.id),
            table,
            config: e.config,
            params: CodecParams {
                codec: VideoCodec::from_fourcc(&e.fourcc),
                fourcc: e.fourcc,
                profile: e.profile,
                level: e.level,
                bit_depth: e.bit_depth,
                chroma: e.chroma,
            },
            samples,
        });
    }
    let mut audio = None;
    let mut audio_info = None;
    if let Some(i) = ai {
        let samples = std::mem::take(&mut traks[i].samples);
        let t = &traks[i];
        if let Some(stsd) = t.boxes.stsd {
            let e = codec_config::entry(&mut rd, &stsd, false, 6)?;
            let (channels, rate) = e.audio.unwrap_or((0, 0));
            let rate = if rate == 0 { t.timescale } else { rate };
            let (mut priming, shown) =
                edit_list::audio_timing(&samples, &t.edits, t.timescale, movie_ts);
            let to_samples =
                |ticks: u64| (ticks as u128 * rate as u128 / t.timescale.max(1) as u128) as u64;
            priming = to_samples(priming);
            if t.edits.is_empty() {
                priming = match (e.pre_skip, roll(&mut rd, &t.boxes)?) {
                    (Some(p), _) => p as u64,
                    (None, Some(r)) if r < 0 => (-(r as i64)) as u64 * 1024,
                    _ => 0,
                };
            }
            let total: u64 = to_samples(samples.iter().map(|s| s.duration as u64).sum());
            let padding = total
                .saturating_sub(priming)
                .saturating_sub(to_samples(shown));
            if t.timescale > 0 {
                duration = duration.max(Rational::new(
                    shown.min(i64::MAX as u64) as i64,
                    t.timescale as i64,
                )?);
            }
            audio_info = Some(AudioTrackInfo {
                codec: AudioCodec::from_fourcc(&e.fourcc),
                fourcc: String::from_utf8_lossy(&e.fourcc).into_owned(),
                sample_rate: rate,
                channels,
                priming: priming.min(u32::MAX as u64) as u32,
                padding: if t.edits.is_empty() {
                    0
                } else {
                    padding.min(u32::MAX as u64) as u32
                },
            });
            audio = Some(AudioTrack {
                track: TrackRef(t.id),
                timescale: t.timescale,
                config: e.config,
                samples,
            });
        }
    }
    let info = MediaInfo {
        container,
        duration,
        fragmented,
        video: video_info,
        audio: audio_info,
        ignored,
    };
    Ok((info, video, audio))
}
