//! The MP4 muxer (spec §6.1): boxes are built and serialised with `mp4-atom`; the sample tables,
//! chunking (one chunk per second per track) and the edit lists are computed here. Output is
//! fast-start (`moov` before `mdat`) and byte-identical for identical input.

mod entries;
mod mp4;

use std::io::{Read, Seek, SeekFrom, Write};

use fin_core::{CoreError, Rational};
use fin_media::{ColourTags, HdrInfo, Rotation, Size};
use serde_json::json;

use crate::error::{invalid, io};

/// A codec configuration: the body of its configuration box (`av1C`, `avcC`, `hvcC`, `vpcC`,
/// `esds`, `dOps`), or a sample entry type with none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecConfig {
    Av1 { av1c: Vec<u8> },
    Avc { avcc: Vec<u8> },
    Hevc { hvcc: Vec<u8> },
    Vp9 { vpcc: Vec<u8> },
    Aac { esds: Vec<u8> },
    Opus { dops: Vec<u8> },
    Other { fourcc: [u8; 4] },
}

impl CodecConfig {
    /// The sample entry type written for this configuration.
    pub fn fourcc(&self) -> [u8; 4] {
        match self {
            CodecConfig::Av1 { .. } => *b"av01",
            CodecConfig::Avc { .. } => *b"avc1",
            CodecConfig::Hevc { .. } => *b"hvc1",
            CodecConfig::Vp9 { .. } => *b"vp09",
            CodecConfig::Aac { .. } => *b"mp4a",
            CodecConfig::Opus { .. } => *b"Opus",
            CodecConfig::Other { fourcc } => *fourcc,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoTrackConfig {
    pub config: CodecConfig,
    pub coded: Size,
    pub timescale: u32,
    pub rotation: Rotation,
    /// Horizontal over vertical spacing; `pasp` is written when it is not 1.
    pub pixel_aspect: Rational,
    /// Written as a `colr` `nclx` box (always).
    pub colour: ColourTags,
    /// `mdcv`, `clli` and `dvcC`/`dvvC` boxes.
    pub hdr: Option<HdrInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioTrackConfig {
    pub config: CodecConfig,
    /// Also the track's timescale.
    pub sample_rate: u32,
    pub channels: u16,
    /// Encoder delay in samples, written as the edit list's media time.
    pub priming: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mp4Options {
    /// Bytes reserved after `ftyp` for the `moov`. A `moov` that does not fit is written there
    /// anyway after the media data is moved forward (which reads the output back).
    pub moov_reserve: u64,
    /// Test-only knobs (feature `testing`).
    #[cfg(feature = "testing")]
    pub knobs: crate::mux::knobs::FileKnobs,
}

impl Default for Mp4Options {
    fn default() -> Self {
        Mp4Options {
            moov_reserve: 64 << 10,
            #[cfg(feature = "testing")]
            knobs: Default::default(),
        }
    }
}

/// Test-only knobs that make the muxer write what real files contain and our encoders never
/// produce, so demuxer tests can be built from muxed files.
#[cfg(feature = "testing")]
pub mod knobs {
    /// Whole-file knobs.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct FileKnobs {
        /// Write a fragmented file (`mvex` in `moov`, then `moof`+`mdat` pairs) with a
        /// fragment every this many samples of any track. Fragmented output has no edit list.
        pub fragment_every: Option<u32>,
    }

    /// The `colr` box written.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub enum ColrKnob {
        #[default]
        Nclx,
        /// An ICC profile (`prof`) with these bytes.
        Icc(Vec<u8>),
        None,
    }

    /// Per-track knobs.
    #[derive(Debug, Clone, PartialEq, Eq, Default)]
    pub struct TrackKnobs {
        /// A `tkhd` matrix in place of the rotation's (a, b, u, c, d, v, x, y, w).
        pub matrix: Option<[i32; 9]>,
        /// An empty edit of this many movie ticks before the media.
        pub empty_edit: Option<u64>,
        pub colr: ColrKnob,
    }
}

/// A track added to a writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrackRef(pub u32);

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TrackKind {
    Video(VideoTrackConfig),
    Audio(AudioTrackConfig),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Sample {
    pub pts: i64,
    pub dts: i64,
    pub duration: u32,
    pub key: bool,
    pub size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Chunk {
    pub offset: u64,
    pub count: u32,
}

#[derive(Debug, Clone)]
pub(crate) struct Track {
    pub kind: TrackKind,
    pub timescale: u32,
    pub samples: Vec<Sample>,
    pub chunks: Vec<Chunk>,
    /// The chunk being collected: its bytes, first sample and duration in ticks.
    pending: Vec<u8>,
    pending_from: usize,
    pending_ticks: u64,
    #[cfg(feature = "testing")]
    pub knobs: knobs::TrackKnobs,
}

impl Track {
    fn new(kind: TrackKind, timescale: u32) -> Track {
        Track {
            kind,
            timescale,
            samples: Vec::new(),
            chunks: Vec::new(),
            pending: Vec::new(),
            pending_from: 0,
            pending_ticks: 0,
            #[cfg(feature = "testing")]
            knobs: Default::default(),
        }
    }
}

/// Writes an MP4 file. Tracks are added before the first sample; samples of each track come
/// in decode order.
pub struct Mp4Writer<W: Read + Write + Seek> {
    w: W,
    opts: Mp4Options,
    tracks: Vec<Track>,
    started: bool,
    /// Where the `moov` reserve starts, and the `mdat` header (non-fragmented output).
    reserve_at: u64,
    mdat_at: u64,
    pos: u64,
    #[cfg(feature = "testing")]
    fragments: u32,
}

impl<W: Read + Write + Seek> Mp4Writer<W> {
    pub fn new(w: W, opts: Mp4Options) -> Result<Self, CoreError> {
        Ok(Mp4Writer {
            w,
            opts,
            tracks: Vec::new(),
            started: false,
            reserve_at: 0,
            mdat_at: 0,
            pos: 0,
            #[cfg(feature = "testing")]
            fragments: 0,
        })
    }

    fn add(&mut self, kind: TrackKind, timescale: u32) -> Result<TrackRef, CoreError> {
        if self.started {
            return Err(invalid(
                "tracks are added before the first sample",
                json!({}),
            ));
        }
        if timescale == 0 {
            return Err(invalid("a track's timescale must be positive", json!({})));
        }
        self.tracks.push(Track::new(kind, timescale));
        Ok(TrackRef(self.tracks.len() as u32 - 1))
    }

    pub fn add_video(&mut self, t: VideoTrackConfig) -> Result<TrackRef, CoreError> {
        if t.coded.width > u16::MAX as u32 || t.coded.height > u16::MAX as u32 {
            return Err(invalid(
                "a coded size must fit 16 bits",
                json!({ "coded": [t.coded.width, t.coded.height] }),
            ));
        }
        let ts = t.timescale;
        self.add(TrackKind::Video(t), ts)
    }

    pub fn add_audio(&mut self, t: AudioTrackConfig) -> Result<TrackRef, CoreError> {
        if t.sample_rate > u16::MAX as u32 && !matches!(t.config, CodecConfig::Opus { .. }) {
            return Err(invalid(
                "an audio sample rate must fit 16 bits",
                json!({ "sampleRate": t.sample_rate }),
            ));
        }
        let ts = t.sample_rate;
        self.add(TrackKind::Audio(t), ts)
    }

    /// Sets a track's test-only knobs.
    #[cfg(feature = "testing")]
    pub fn set_knobs(&mut self, track: TrackRef, knobs: knobs::TrackKnobs) {
        self.tracks[track.0 as usize].knobs = knobs;
    }

    fn put(&mut self, bytes: &[u8]) -> Result<(), CoreError> {
        self.w.write_all(bytes).map_err(|e| io(&e))?;
        self.pos += bytes.len() as u64;
        Ok(())
    }

    fn fragmented(&self) -> bool {
        #[cfg(feature = "testing")]
        return self.opts.knobs.fragment_every.is_some();
        #[cfg(not(feature = "testing"))]
        false
    }

    fn start(&mut self) -> Result<(), CoreError> {
        self.started = true;
        self.pos = self.w.stream_position().map_err(|e| io(&e))?;
        let ftyp = mp4::ftyp(&self.tracks)?;
        self.put(&ftyp)?;
        if self.fragmented() {
            let moov = mp4::moov(&self.tracks, 0, true)?;
            return self.put(&moov);
        }
        self.reserve_at = self.pos;
        if self.opts.moov_reserve > 0 {
            let free = mp4::free(self.opts.moov_reserve)?;
            self.put(&free)?;
        }
        self.mdat_at = self.pos;
        self.put(&mp4::mdat_header(0))
    }

    /// One sample of `track`: `pts` and `dts` in the track's timescale, `dts` strictly
    /// increasing by the previous sample's `duration`.
    pub fn write(
        &mut self,
        track: TrackRef,
        data: &[u8],
        pts: i64,
        dts: i64,
        duration: u32,
        key: bool,
    ) -> Result<(), CoreError> {
        let i = track.0 as usize;
        if i >= self.tracks.len() {
            return Err(invalid("unknown track", json!({ "track": track.0 })));
        }
        let size = u32::try_from(data.len()).map_err(|_| {
            invalid(
                "a sample must be smaller than 4 GiB",
                json!({ "size": data.len() }),
            )
        })?;
        if let Some(prev) = self.tracks[i].samples.last()
            && dts != prev.dts + prev.duration as i64
        {
            return Err(invalid(
                "a sample's dts must be the previous dts plus its duration",
                json!({ "track": track.0, "dts": dts, "expected": prev.dts + prev.duration as i64 }),
            ));
        }
        if !self.started {
            self.start()?;
        }
        let t = &mut self.tracks[i];
        t.samples.push(Sample {
            pts,
            dts,
            duration,
            key,
            size,
        });
        t.pending.extend_from_slice(data);
        t.pending_ticks += duration as u64;
        let full = t.pending_ticks >= t.timescale as u64;
        #[cfg(feature = "testing")]
        if let Some(n) = self.opts.knobs.fragment_every {
            if self.tracks[i].samples.len() - self.tracks[i].pending_from >= n as usize {
                return self.fragment();
            }
            return Ok(());
        }
        if full {
            self.flush(i)?;
        }
        Ok(())
    }

    /// Writes track `i`'s pending chunk.
    fn flush(&mut self, i: usize) -> Result<(), CoreError> {
        let t = &mut self.tracks[i];
        let count = (t.samples.len() - t.pending_from) as u32;
        if count == 0 {
            return Ok(());
        }
        let bytes = std::mem::take(&mut t.pending);
        t.chunks.push(Chunk {
            offset: self.pos,
            count,
        });
        t.pending_from = t.samples.len();
        t.pending_ticks = 0;
        self.put(&bytes)
    }

    /// Writes every track's pending samples as one `moof` + `mdat`.
    #[cfg(feature = "testing")]
    fn fragment(&mut self) -> Result<(), CoreError> {
        self.fragments += 1;
        let pending: Vec<(usize, usize)> = self
            .tracks
            .iter()
            .map(|t| (t.pending_from, t.samples.len()))
            .collect();
        let (moof, mdat) = mp4::fragment(&self.tracks, &pending, self.fragments)?;
        for t in &mut self.tracks {
            t.pending.clear();
            t.pending_from = t.samples.len();
            t.pending_ticks = 0;
        }
        self.put(&moof)?;
        self.put(&mdat)
    }

    /// Writes the `moov` (into the reserve, or after moving the media data forward) and
    /// returns the output.
    pub fn finish(mut self) -> Result<W, CoreError> {
        if !self.started {
            self.start()?;
        }
        #[cfg(feature = "testing")]
        if self.fragmented() {
            if self.tracks.iter().any(|t| t.pending_from < t.samples.len()) {
                self.fragment()?;
            }
            return Ok(self.w);
        }
        for i in 0..self.tracks.len() {
            self.flush(i)?;
        }
        let end = self.pos;
        let reserve = self.opts.moov_reserve;
        let mut shift = 0u64;
        let moov = loop {
            let moov = mp4::moov(&self.tracks, shift, false)?;
            let room = reserve + shift;
            let len = moov.len() as u64;
            if len == room || len + 8 <= room {
                break moov;
            }
            // Grow the room to the moov exactly, or to the moov plus the smallest `free`.
            shift = if len > room {
                len - reserve
            } else {
                shift + (len + 8 - room)
            };
        };
        if shift > 0 {
            self.move_forward(self.mdat_at, end, shift)?;
        }
        let mdat_at = self.mdat_at + shift;
        self.seek(mdat_at)?;
        self.w
            .write_all(&mp4::mdat_header(end + shift - mdat_at))
            .map_err(|e| io(&e))?;
        self.seek(self.reserve_at)?;
        self.w.write_all(&moov).map_err(|e| io(&e))?;
        let rest = reserve + shift - moov.len() as u64;
        if rest > 0 {
            self.w.write_all(&mp4::free(rest)?).map_err(|e| io(&e))?;
        }
        self.seek(end + shift)?;
        self.w.flush().map_err(|e| io(&e))?;
        Ok(self.w)
    }

    fn seek(&mut self, to: u64) -> Result<(), CoreError> {
        self.w.seek(SeekFrom::Start(to)).map_err(|e| io(&e))?;
        Ok(())
    }

    /// Moves bytes `[from, to)` forward by `by`, last block first.
    fn move_forward(&mut self, from: u64, to: u64, by: u64) -> Result<(), CoreError> {
        const BLOCK: u64 = 1 << 20;
        let mut buf = vec![0u8; BLOCK as usize];
        let mut hi = to;
        while hi > from {
            let lo = hi.saturating_sub(BLOCK).max(from);
            let n = (hi - lo) as usize;
            self.seek(lo)?;
            self.w.read_exact(&mut buf[..n]).map_err(|e| io(&e))?;
            self.seek(lo + by)?;
            self.w.write_all(&buf[..n]).map_err(|e| io(&e))?;
            hi = lo;
        }
        Ok(())
    }
}
