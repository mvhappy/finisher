//! A WebM/Matroska writer for tests only (the WebM muxer is cut from v1, spec §1): tracks,
//! blocks as `SimpleBlock` or `BlockGroup`, the four lacings, clusters of known or unknown
//! size, an unknown-sized Segment, and `Cues`. The element helpers build hostile files too.

/// The id's bytes (ids keep their length marker).
pub fn id_bytes(id: u32) -> Vec<u8> {
    let b = id.to_be_bytes();
    let skip = b.iter().take_while(|&&c| c == 0).count().min(3);
    b[skip..].to_vec()
}

/// `n` as the shortest EBML size (the all-ones value of each length is reserved).
pub fn size_bytes(n: u64) -> Vec<u8> {
    let len = (1..=8)
        .find(|&l| n < (1u64 << (7 * l)) - 1)
        .expect("size under 2⁵⁶ - 1");
    let v = n | (1u64 << (7 * len));
    v.to_be_bytes()[8 - len..].to_vec()
}

/// The 8-byte "unknown size".
pub const UNKNOWN_SIZE: [u8; 8] = [0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];

pub fn element(id: u32, body: &[u8]) -> Vec<u8> {
    [id_bytes(id), size_bytes(body.len() as u64), body.to_vec()].concat()
}

pub fn uint(id: u32, v: u64) -> Vec<u8> {
    let b = v.to_be_bytes();
    let skip = b.iter().take_while(|&&c| c == 0).count().min(7);
    element(id, &b[skip..])
}

pub fn float(id: u32, v: f64) -> Vec<u8> {
    element(id, &v.to_be_bytes())
}

pub fn string(id: u32, v: &str) -> Vec<u8> {
    element(id, v.as_bytes())
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrackKind {
    Video {
        width: u64,
        height: u64,
        /// `DisplayWidth`/`DisplayHeight` in pixels.
        display: Option<(u64, u64)>,
        /// Raw children appended to `Video` (`Colour`, `Projection`, …).
        extra: Vec<u8>,
    },
    Audio {
        rate: f64,
        channels: u64,
    },
    /// Any other `TrackType` (17 subtitles, …).
    Other(u64),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub number: u64,
    pub kind: TrackKind,
    pub codec_id: String,
    pub private: Option<Vec<u8>>,
    /// Nanoseconds.
    pub default_duration: Option<u64>,
    /// Nanoseconds.
    pub codec_delay: Option<u64>,
    /// Raw children appended to the `TrackEntry` (`ContentEncodings`, …).
    pub extra: Vec<u8>,
}

impl Track {
    pub fn video(number: u64, codec_id: &str, width: u64, height: u64) -> Track {
        Track {
            number,
            kind: TrackKind::Video {
                width,
                height,
                display: None,
                extra: Vec::new(),
            },
            codec_id: codec_id.into(),
            private: None,
            default_duration: None,
            codec_delay: None,
            extra: Vec::new(),
        }
    }

    pub fn audio(number: u64, codec_id: &str, rate: f64, channels: u64) -> Track {
        Track {
            kind: TrackKind::Audio { rate, channels },
            ..Track::video(number, codec_id, 0, 0)
        }
    }

    fn entry(&self) -> Vec<u8> {
        let (kind, media) = match &self.kind {
            TrackKind::Video {
                width,
                height,
                display,
                extra,
            } => {
                let mut v = [uint(0xB0, *width), uint(0xBA, *height)].concat();
                if let Some((w, h)) = display {
                    v.extend(uint(0x54B0, *w));
                    v.extend(uint(0x54BA, *h));
                }
                v.extend(extra);
                (1, element(0xE0, &v))
            }
            TrackKind::Audio { rate, channels } => (
                2,
                element(0xE1, &[float(0xB5, *rate), uint(0x9F, *channels)].concat()),
            ),
            TrackKind::Other(k) => (*k, Vec::new()),
        };
        let mut b = [
            uint(0xD7, self.number),
            uint(0x73C5, self.number),
            uint(0x83, kind),
            string(0x86, &self.codec_id),
        ]
        .concat();
        if let Some(p) = &self.private {
            b.extend(element(0x63A2, p));
        }
        if let Some(d) = self.default_duration {
            b.extend(uint(0x23E383, d));
        }
        if let Some(d) = self.codec_delay {
            b.extend(uint(0x56AA, d));
        }
        b.extend(media);
        b.extend(&self.extra);
        element(0xAE, &b)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lacing {
    None,
    Xiph,
    Fixed,
    Ebml,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub track: u64,
    /// Segment ticks (`TimestampScale` nanoseconds each).
    pub ts: i64,
    pub key: bool,
    pub frames: Vec<Vec<u8>>,
    pub lacing: Lacing,
    /// A `BlockGroup` (keyframes without, others with a `ReferenceBlock`) instead of a
    /// `SimpleBlock`.
    pub group: bool,
    /// A `BlockGroup`'s `BlockDuration`.
    pub duration: Option<u64>,
    /// A `BlockGroup`'s `DiscardPadding` (ns).
    pub padding: Option<i64>,
}

impl Block {
    pub fn simple(track: u64, ts: i64, key: bool, data: Vec<u8>) -> Block {
        Block {
            track,
            ts,
            key,
            frames: vec![data],
            lacing: Lacing::None,
            group: false,
            duration: None,
            padding: None,
        }
    }

    fn bytes(&self, cluster_ts: i64, unflag: bool) -> Vec<u8> {
        let rel = i16::try_from(self.ts - cluster_ts).expect("block within the cluster's range");
        let mut b = size_bytes(self.track);
        b.extend(rel.to_be_bytes());
        let lacing = match self.lacing {
            Lacing::None => 0,
            Lacing::Xiph => 1,
            Lacing::Fixed => 2,
            Lacing::Ebml => 3,
        };
        b.push((u8::from(self.key && !self.group && !unflag) << 7) | (lacing << 1));
        let n = self.frames.len();
        if self.lacing != Lacing::None {
            b.push((n - 1) as u8);
        }
        match self.lacing {
            Lacing::Xiph => {
                for f in &self.frames[..n - 1] {
                    let mut s = f.len();
                    while s >= 255 {
                        b.push(255);
                        s -= 255;
                    }
                    b.push(s as u8);
                }
            }
            Lacing::Ebml => {
                b.extend(size_bytes(self.frames[0].len() as u64));
                for w in self.frames[..n - 1].windows(2) {
                    let d = w[1].len() as i64 - w[0].len() as i64;
                    // Signed: the shortest length whose range holds d.
                    let len = (1..=8)
                        .find(|&l| d.abs() < (1i64 << (7 * l - 1)) - 1)
                        .expect("difference fits");
                    let v = (d + (1i64 << (7 * len - 1)) - 1) as u64 | (1u64 << (7 * len));
                    b.extend(&v.to_be_bytes()[8 - len..]);
                }
            }
            _ => {}
        }
        for f in &self.frames {
            b.extend(f);
        }
        if !self.group {
            return element(0xA3, &b);
        }
        let mut g = element(0xA1, &b);
        if !self.key {
            g.extend(element(0xFB, &[0xFF]));
        }
        if let Some(d) = self.duration {
            g.extend(uint(0x9B, d));
        }
        if let Some(p) = self.padding {
            g.extend(element(0x75A2, &p.to_be_bytes()));
        }
        element(0xA0, &g)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub doc_type: String,
    pub timestamp_scale: u64,
    /// Blocks a cluster (a cluster also ends when a block is out of its `i16` range).
    pub cluster_blocks: usize,
    pub cues: bool,
    /// Added to every cue's time (a disagreeing index).
    pub cue_shift: i64,
    pub unknown_size_clusters: bool,
    pub unknown_size_segment: bool,
    /// Writes every `SimpleBlock` without its keyframe flag (cues still name the keyframes).
    pub unflag_keys: bool,
    /// Two-byte `Void` elements appended inside `Cues` (a hostile index).
    pub cue_voids: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            doc_type: "webm".into(),
            timestamp_scale: 1_000_000,
            cluster_blocks: 8,
            cues: false,
            cue_shift: 0,
            unknown_size_clusters: false,
            unknown_size_segment: false,
            unflag_keys: false,
            cue_voids: 0,
        }
    }
}

/// The EBML header for `doc_type`.
pub fn ebml_header(doc_type: &str) -> Vec<u8> {
    element(
        0x1A45_DFA3,
        &[
            uint(0x4286, 1),
            uint(0x42F7, 1),
            uint(0x42F2, 4),
            uint(0x42F3, 8),
            string(0x4282, doc_type),
            uint(0x4287, 4),
            uint(0x4285, 2),
        ]
        .concat(),
    )
}

/// A whole file: the EBML header, then a Segment of `Info`, `Tracks`, clusters and (with
/// `cues`) `Cues` naming the first keyframe of the first track in each cluster.
pub fn write(o: &Options, tracks: &[Track], blocks: &[Block]) -> Vec<u8> {
    let info = element(
        0x1549_A966,
        &[
            uint(0x2A_D7B1, o.timestamp_scale),
            string(0x4D80, "fin-test"),
        ]
        .concat(),
    );
    let tracks_el = element(
        0x1654_AE6B,
        &tracks.iter().flat_map(Track::entry).collect::<Vec<u8>>(),
    );
    let mut body = [info, tracks_el].concat();
    let cue_track = tracks.first().map(|t| t.number);
    let mut cue_points = Vec::new();
    let mut i = 0;
    while i < blocks.len() {
        let start = blocks[i].ts;
        let mut j = i;
        while j < blocks.len()
            && j - i < o.cluster_blocks
            && blocks[j]
                .ts
                .checked_sub(start.max(0))
                .is_some_and(|d| i16::try_from(d).is_ok())
        {
            j += 1;
        }
        let pos = body.len() as u64;
        if let Some(k) = blocks[i..j]
            .iter()
            .find(|b| Some(b.track) == cue_track && b.key)
        {
            cue_points.push((k.ts + o.cue_shift, pos));
        }
        let mut c = uint(0xE7, start.max(0) as u64);
        for b in &blocks[i..j] {
            c.extend(b.bytes(start.max(0), o.unflag_keys));
        }
        if o.unknown_size_clusters {
            body.extend(id_bytes(0x1F43_B675));
            body.extend(UNKNOWN_SIZE);
            body.extend(c);
        } else {
            body.extend(element(0x1F43_B675, &c));
        }
        i = j;
    }
    if o.cues
        && let Some(track) = cue_track
    {
        let mut points: Vec<u8> = cue_points
            .iter()
            .flat_map(|&(t, pos)| {
                element(
                    0xBB,
                    &[
                        uint(0xB3, t.max(0) as u64),
                        element(0xB7, &[uint(0xF7, track), uint(0xF1, pos)].concat()),
                    ]
                    .concat(),
                )
            })
            .collect();
        for _ in 0..o.cue_voids {
            points.extend([0xEC, 0x80]);
        }
        body.extend(element(0x1C53_BB6B, &points));
    }
    let mut out = ebml_header(&o.doc_type);
    if o.unknown_size_segment {
        out.extend(id_bytes(0x1853_8067));
        out.extend(UNKNOWN_SIZE);
        out.extend(body);
    } else {
        out.extend(element(0x1853_8067, &body));
    }
    out
}

/// `Colour` with code points, range, and optional content light and mastering display.
pub fn colour(primaries: u64, transfer: u64, matrix: u64, range: u64) -> Vec<u8> {
    element(
        0x55B0,
        &[
            uint(0x55BB, primaries),
            uint(0x55BA, transfer),
            uint(0x55B1, matrix),
            uint(0x55B9, range),
        ]
        .concat(),
    )
}

/// `Colour` for HDR10: BT.2020, PQ, limited range, `MaxCLL`/`MaxFALL` and a P3-D65 mastering
/// display of 1000/0.0050 cd/m².
pub fn hdr10_colour(max_cll: u64, max_fall: u64) -> Vec<u8> {
    let m = [
        0.680, 0.320, 0.265, 0.690, 0.150, 0.060, 0.3127, 0.3290, 1000.0, 0.005,
    ];
    let mastering: Vec<u8> = m
        .iter()
        .enumerate()
        .flat_map(|(i, &v)| float(0x55D1 + i as u32, v))
        .collect();
    element(
        0x55B0,
        &[
            uint(0x55BB, 9),
            uint(0x55BA, 16),
            uint(0x55B1, 9),
            uint(0x55B9, 1),
            uint(0x55B7, 1),
            uint(0x55B8, 2),
            uint(0x55BC, max_cll),
            uint(0x55BD, max_fall),
            element(0x55D0, &mastering),
        ]
        .concat(),
    )
}

/// An `OpusHead` (mapping family 0).
pub fn opus_head(channels: u8, pre_skip: u16) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1);
    h.push(channels);
    h.extend(pre_skip.to_le_bytes());
    h.extend(48_000u32.to_le_bytes());
    h.extend(0i16.to_le_bytes());
    h.push(0);
    h
}
