//! The EBML header, the first `Segment`'s layout (where its `Info`, `Tracks`, `Cues` and
//! clusters are; an unknown-sized `Cluster` ends at the next Segment-level id), `Info` and the
//! track entries.

use std::io::{Read, Seek};

use fin_core::CoreError;
use fin_media::ContainerKind;

use super::cluster;
use super::ebml::*;
use crate::error::{corrupt, unsupported_container};

/// `Colour` as stored (absent elements are `None`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct ColourDesc {
    pub matrix: Option<u64>,
    pub bits: Option<u64>,
    pub subsampling: (Option<u64>, Option<u64>),
    pub siting: (Option<u64>, Option<u64>),
    pub range: Option<u64>,
    pub transfer: Option<u64>,
    pub primaries: Option<u64>,
    pub max_cll: Option<u64>,
    pub max_fall: Option<u64>,
    /// R x, R y, G x, G y, B x, B y, white x, white y, luminance max, min.
    pub mastering: Option<[f64; 10]>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct VideoDesc {
    pub width: u64,
    pub height: u64,
    /// `DisplayWidth` and `DisplayHeight` when `DisplayUnit` is pixels.
    pub display: Option<(u64, u64)>,
    pub colour: Option<ColourDesc>,
    /// `ProjectionType` and whether a pose is set.
    pub projection: Option<(u64, bool)>,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct AudioDesc {
    pub rate: f64,
    pub channels: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct TrackDesc {
    pub number: u64,
    pub kind: u64,
    pub codec_id: String,
    pub private: Option<Vec<u8>>,
    /// Nanoseconds.
    pub default_duration: Option<u64>,
    /// Nanoseconds.
    pub codec_delay: u64,
    /// A `ContentEncodings` element (compression or encryption of the frames).
    pub encoded: bool,
    pub video: Option<VideoDesc>,
    pub audio: Option<AudioDesc>,
    pub at: u64,
}

pub(super) struct Layout {
    pub container: ContainerKind,
    /// Cluster and cue positions are relative to this offset.
    pub segment_body: u64,
    /// Nanoseconds a tick.
    pub scale: u64,
    pub tracks: Vec<TrackDesc>,
    pub clusters: Vec<Element>,
    pub cues: Option<Element>,
}

/// The EBML header at 0: the doc type, and that this reader can read the file.
fn header<R: Read + Seek>(eb: &mut Ebml<R>) -> Result<(ContainerKind, u64), CoreError> {
    let h = eb.header(0, eb.len, 0, true)?;
    if h.id != EBML || h.unknown {
        return Err(unsupported_container("unknown", "not an EBML file"));
    }
    let mut doc = String::from("matroska");
    for c in eb.children(&h, 1)? {
        let refuse = |what: &str| Err(unsupported_container("matroska", what));
        match c.id {
            DOC_TYPE => doc = eb.string(&c)?,
            EBML_READ_VERSION if eb.uint(&c)? > 1 => return refuse("EBML read version over 1"),
            EBML_MAX_ID_LENGTH if eb.uint(&c)? > 4 => return refuse("element ids over 4 bytes"),
            EBML_MAX_SIZE_LENGTH if eb.uint(&c)? > 8 => {
                return refuse("element sizes over 8 bytes");
            }
            DOC_TYPE_READ_VERSION if eb.uint(&c)? > 4 => {
                return refuse("Matroska read version over 4");
            }
            _ => {}
        }
    }
    let kind = match doc.as_str() {
        "webm" => ContainerKind::Webm,
        "matroska" => ContainerKind::Matroska,
        _ => {
            return Err(unsupported_container(
                "unknown",
                "an EBML file that is not WebM or Matroska",
            ));
        }
    };
    Ok((kind, h.end))
}

/// The first Segment's top-level elements (clusters found, not read).
pub(super) fn layout<R: Read + Seek>(eb: &mut Ebml<R>) -> Result<Layout, CoreError> {
    let (container, mut at) = header(eb)?;
    // Void or CRC elements may sit between the header and the Segment.
    let seg = loop {
        let e = eb.header(at, eb.len, 0, true)?;
        if e.id == SEGMENT {
            break e;
        }
        at = e.end;
    };
    let mut l = Layout {
        container,
        segment_body: seg.body,
        scale: 1_000_000,
        tracks: Vec::new(),
        clusters: Vec::new(),
        cues: None,
    };
    let (mut info, mut tracks) = (None, None);
    let mut at = seg.body;
    while seg.end - at >= 2 {
        let mut e = eb.header(at, seg.end, 1, true)?;
        if e.unknown {
            if e.id != CLUSTER {
                return Err(corrupt("a Segment inside a Segment", e.start, "Segment"));
            }
            e.end = cluster::unknown_end(eb, &e)?;
        }
        match e.id {
            // Another stream or Segment after the first: only the first is read.
            EBML | SEGMENT => break,
            CLUSTER => l.clusters.push(e),
            INFO if info.is_none() => info = Some(e),
            TRACKS if tracks.is_none() => tracks = Some(e),
            CUES if l.cues.is_none() => l.cues = Some(e),
            _ => {}
        }
        at = e.end;
    }
    if let Some(info) = info {
        for c in eb.children(&info, 2)? {
            if c.id == TIMESTAMP_SCALE {
                l.scale = eb.uint(&c)?;
            }
        }
    }
    if l.scale == 0 {
        return Err(corrupt("the TimestampScale is 0", seg.body, "Info"));
    }
    let tracks =
        tracks.ok_or_else(|| corrupt("the Segment has no Tracks", seg.start, "Segment"))?;
    for t in eb.children(&tracks, 2)? {
        if t.id == TRACK_ENTRY {
            l.tracks.push(track(eb, &t)?);
        }
    }
    Ok(l)
}

fn track<R: Read + Seek>(eb: &mut Ebml<R>, t: &Element) -> Result<TrackDesc, CoreError> {
    let mut d = TrackDesc {
        number: 0,
        kind: 0,
        codec_id: String::new(),
        private: None,
        default_duration: None,
        codec_delay: 0,
        encoded: false,
        video: None,
        audio: None,
        at: t.start,
    };
    for c in eb.children(t, 3)? {
        match c.id {
            0xD7 => d.number = eb.uint(&c)?,
            0x83 => d.kind = eb.uint(&c)?,
            0x86 => d.codec_id = eb.string(&c)?,
            0x63A2 => d.private = Some(eb.body(&c)?),
            0x23E383 => d.default_duration = Some(eb.uint(&c)?).filter(|&n| n > 0),
            0x56AA => d.codec_delay = eb.uint(&c)?,
            0x6D80 => d.encoded = !eb.children(&c, 4)?.is_empty(),
            0xE0 => d.video = Some(video(eb, &c)?),
            0xE1 => d.audio = Some(audio(eb, &c)?),
            _ => {}
        }
    }
    if d.number == 0 {
        return Err(corrupt("a track has no TrackNumber", t.start, "TrackEntry"));
    }
    Ok(d)
}

fn video<R: Read + Seek>(eb: &mut Ebml<R>, v: &Element) -> Result<VideoDesc, CoreError> {
    let mut d = VideoDesc::default();
    let (mut dw, mut dh, mut unit) = (None, None, 0);
    for c in eb.children(v, 4)? {
        match c.id {
            0xB0 => d.width = eb.uint(&c)?,
            0xBA => d.height = eb.uint(&c)?,
            0x54B0 => dw = Some(eb.uint(&c)?),
            0x54BA => dh = Some(eb.uint(&c)?),
            0x54B2 => unit = eb.uint(&c)?,
            0x55B0 => d.colour = Some(colour(eb, &c)?),
            0x7670 => {
                let (mut kind, mut posed) = (0, false);
                for p in eb.children(&c, 5)? {
                    match p.id {
                        0x7671 => kind = eb.uint(&p)?,
                        0x7673..=0x7675 => posed |= eb.float(&p)? != 0.0,
                        _ => {}
                    }
                }
                d.projection = Some((kind, posed));
            }
            _ => {}
        }
    }
    // Display sizes in centimetres, inches or as an aspect ratio say nothing of pixels.
    if unit == 0 {
        d.display = Some((dw.unwrap_or(d.width), dh.unwrap_or(d.height)));
    }
    Ok(d)
}

fn colour<R: Read + Seek>(eb: &mut Ebml<R>, e: &Element) -> Result<ColourDesc, CoreError> {
    let mut d = ColourDesc::default();
    for c in eb.children(e, 5)? {
        match c.id {
            0x55B1 => d.matrix = Some(eb.uint(&c)?),
            0x55B2 => d.bits = Some(eb.uint(&c)?),
            0x55B3 => d.subsampling.0 = Some(eb.uint(&c)?),
            0x55B4 => d.subsampling.1 = Some(eb.uint(&c)?),
            0x55B7 => d.siting.0 = Some(eb.uint(&c)?),
            0x55B8 => d.siting.1 = Some(eb.uint(&c)?),
            0x55B9 => d.range = Some(eb.uint(&c)?),
            0x55BA => d.transfer = Some(eb.uint(&c)?),
            0x55BB => d.primaries = Some(eb.uint(&c)?),
            0x55BC => d.max_cll = Some(eb.uint(&c)?),
            0x55BD => d.max_fall = Some(eb.uint(&c)?),
            0x55D0 => {
                let mut m = [f64::NAN; 10];
                for p in eb.children(&c, 6)? {
                    if let 0x55D1..=0x55DA = p.id {
                        m[(p.id - 0x55D1) as usize] = eb.float(&p)?;
                    }
                }
                d.mastering = Some(m);
            }
            _ => {}
        }
    }
    Ok(d)
}

fn audio<R: Read + Seek>(eb: &mut Ebml<R>, a: &Element) -> Result<AudioDesc, CoreError> {
    let mut d = AudioDesc {
        rate: 8000.0,
        channels: 1,
    };
    for c in eb.children(a, 4)? {
        match c.id {
            0xB5 => d.rate = eb.float(&c)?,
            0x9F => d.channels = eb.uint(&c)?,
            _ => {}
        }
    }
    Ok(d)
}

/// The `format` detail of an `UNSUPPORTED_CONTAINER` for this file.
pub(super) fn format_of(kind: ContainerKind) -> &'static str {
    match kind {
        ContainerKind::Webm => "webm",
        _ => "matroska",
    }
}
