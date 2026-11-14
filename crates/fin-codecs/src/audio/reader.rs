//! `AudioReader` (6C plan Task 12, spec §8.1): a file's first audio track as interleaved `f32`
//! at the source's own rate, in its own channel order with its [`ChannelLayout`], from the first
//! audible sample to the last. MP4/MOV/M4A go through `fin-container` (its limits; the reader
//! trims `AudioTrackInfo`'s priming and padding); WAV, AIFF, CAF, FLAC, Ogg and MP3 through
//! symphonia's format readers (which trim their own delay and padding, gapless). It never
//! resamples or downmixes: the worker does, with `fin-audio` (Decision 3).
//!
//! Reads are bounded: more than [`AudioLimits::max_channels`] channels, a rate above
//! [`AudioLimits::max_rate`] or more than [`AudioLimits::max_seconds`] of audio is `MEDIA_LIMIT`;
//! a container that states its length and delivers fewer frames is `MEDIA_CORRUPT` (truncated).

use std::io::{Read, Seek, SeekFrom};

use fin_container::Limits;
use fin_core::CoreError;
use fin_media::audio_layout::ChannelLayout;
use fin_media::backend::VendorClass;
use fin_media::backend_audio::audio_backend;
use fin_media::{AudioCodec, AudioTrackInfo};
use serde_json::json;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

use super::mp4;
use super::registry::{open_audio_decoder, symphonia_decodes};
use super::sniff::{Kind, mp3_exact, ogg_end, sniff, wave_layout};
use super::symphonia::{BACKEND, SymphoniaDecoder, codec_of, error, guarded};
use super::traits::{AudioConfig, AudioDecoder, DecodedAudio};
use crate::traits::BackendId;

/// What a read may cost (spec §6.1 for the container; the audio bounds of Task 12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioLimits {
    pub container: Limits,
    /// Audio longer than this is refused (24 h).
    pub max_seconds: u64,
    pub max_channels: u16,
    pub max_rate: u32,
}

impl Default for AudioLimits {
    /// 24 h, 8 channels, 384 kHz; `fin-container`'s default limits.
    fn default() -> Self {
        AudioLimits {
            container: Limits::default(),
            max_seconds: 24 * 3600,
            max_channels: 8,
            max_rate: 384_000,
        }
    }
}

/// Bytes read before any reader runs: the head for sniffing and the WAVE `fmt ` chunk, the tail
/// for Ogg's last page (one page is at most 65,307 bytes).
const HEAD: u64 = 64 << 10;
const TAIL: u64 = 66 << 10;

/// A `Read + Seek` source as symphonia's `MediaSource`.
struct Source<R> {
    r: R,
    len: u64,
}

impl<R: Read> Read for Source<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.r.read(buf)
    }
}

impl<R: Seek> Seek for Source<R> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.r.seek(pos)
    }
}

impl<R: Read + Seek + Send + Sync> MediaSource for Source<R> {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

enum Packets {
    Mp4 {
        src: Box<dyn mp4::Packets>,
        dec: Box<dyn AudioDecoder>,
        buf: Vec<u8>,
    },
    Symphonia {
        format: Box<dyn FormatReader>,
        track: u32,
        dec: SymphoniaDecoder,
    },
}

/// Drops `skip` frames from the start, then passes at most `left` frames (the presented
/// length, when the container states it exactly) and drops the rest: padding is never held.
#[derive(Debug, Default)]
struct Trim {
    skip: u64,
    left: Option<u64>,
}

impl Trim {
    fn push(&mut self, pcm: &[f32], channels: usize) -> Vec<f32> {
        let frames = (pcm.len() / channels) as u64;
        let drop = self.skip.min(frames);
        self.skip -= drop;
        let mut take = frames - drop;
        if let Some(left) = &mut self.left {
            take = take.min(*left);
            *left -= take;
        }
        let from = drop as usize * channels;
        pcm[from..from + take as usize * channels].to_vec()
    }

    /// Everything presented has been passed.
    fn done(&self) -> bool {
        self.left == Some(0)
    }
}

pub struct AudioReader {
    info: AudioTrackInfo,
    kind: Kind,
    packets: Packets,
    /// The WAVE layout of Decision 3, in place of symphonia's guess for a mask-less file.
    layout: Option<ChannelLayout>,
    declared: Option<u64>,
    /// `declared` is exact: fewer frames is a cut file.
    exact: bool,
    /// Frames decoded, before trimming.
    decoded: u64,
    max_frames: u64,
    trim: Trim,
    out: u64,
    ended: bool,
}

impl std::fmt::Debug for AudioReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AudioReader")
            .field("info", &self.info)
            .field("container", &self.kind.name())
            .field("declared", &self.declared)
            .finish_non_exhaustive()
    }
}

fn limit(name: &str, value: u64, max: u64) -> CoreError {
    fin_container::error::limit(name, value, max)
}

fn read_at<R: Read + Seek>(r: &mut R, at: u64, len: u64) -> Result<Vec<u8>, CoreError> {
    let io = |e: std::io::Error| fin_container::error::io(&e);
    r.seek(SeekFrom::Start(at)).map_err(io)?;
    let mut out = Vec::new();
    r.take(len).read_to_end(&mut out).map_err(io)?;
    Ok(out)
}

/// A known container a reader could not open is corrupt (it was cut or broken), unless the
/// reader said something more precise.
fn corrupt_unless_typed(e: CoreError, kind: Kind) -> CoreError {
    match e.code.as_str() {
        "MEDIA_LIMIT" | "MEDIA_CORRUPT" | "DECODE_FAILED" | "IO_ERROR" | "CODEC_UNAVAILABLE" => e,
        _ => CoreError::new(
            "MEDIA_CORRUPT",
            format!("the {} file could not be read: {}", kind.name(), e.message),
        )
        .with_details(json!({ "at": "header", "format": kind.name() })),
    }
}

/// The four-character code an MP4 sample entry would have for `codec` (non-MP4 sources).
fn fourcc(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "mp4a",
        AudioCodec::Opus => "Opus",
        AudioCodec::Mp3 => ".mp3",
        AudioCodec::Flac => "fLaC",
        AudioCodec::Alac => "alac",
        AudioCodec::Pcm => "lpcm",
        AudioCodec::Ac3 => "ac-3",
        AudioCodec::Eac3 => "ec-3",
        AudioCodec::Vorbis => "vorb",
        AudioCodec::Other => "????",
    }
}

impl AudioReader {
    /// Opens the first audio track of `r`: `UNSUPPORTED_CONTAINER` for a file that is no audio
    /// container this build reads, `MEDIA_CORRUPT` for a broken one, `CODEC_UNAVAILABLE` for a
    /// codec no backend here decodes, `MEDIA_LIMIT` past `limits`.
    pub fn open<R: Read + Seek + Send + Sync + 'static>(
        mut r: R,
        limits: &AudioLimits,
    ) -> Result<Self, CoreError> {
        let io = |e: std::io::Error| fin_container::error::io(&e);
        let len = r.seek(SeekFrom::End(0)).map_err(io)?;
        let head = read_at(&mut r, 0, HEAD)?;
        let kind = sniff(&head);
        r.seek(SeekFrom::Start(0)).map_err(io)?;
        let reader = match kind {
            Kind::Unknown => {
                return Err(fin_container::error::unsupported_container(
                    "unknown",
                    "not an audio container this build reads",
                ));
            }
            Kind::Mp4 => Self::open_mp4(r, limits)?,
            _ => {
                if kind == Kind::Ogg {
                    let tail = read_at(&mut r, len.saturating_sub(TAIL), TAIL)?;
                    ogg_end(&tail).map_err(|why| {
                        CoreError::new("MEDIA_CORRUPT", format!("the Ogg stream is cut: {why}"))
                            .with_details(json!({ "at": "end", "format": "ogg" }))
                    })?;
                    r.seek(SeekFrom::Start(0)).map_err(io)?;
                }
                let layout = match kind {
                    Kind::Wave => wave_layout(&mut r, len)?,
                    _ => None,
                };
                // Lengths symphonia reads from the stream are exact; an MP3's is an estimate
                // unless a Xing/Info or VBRI frame states it.
                let exact = kind.exact_length() || (kind == Kind::Mp3 && mp3_exact(&mut r)?);
                r.seek(SeekFrom::Start(0)).map_err(io)?;
                let mut reader = Self::open_symphonia(Source { r, len }, kind, layout, limits)
                    .map_err(|e| corrupt_unless_typed(e, kind))?;
                reader.exact = exact;
                reader
            }
        };
        reader.check(limits)
    }

    fn open_mp4<R: Read + Seek + Send + 'static>(
        r: R,
        limits: &AudioLimits,
    ) -> Result<Self, CoreError> {
        let (src, info, config, total) = mp4::open(r, &limits.container)?;
        // Untrusted edit lists: priming and padding must fit what the track holds, and what it
        // holds (all of which may be decoded) must fit the budget.
        let (priming, padding) = (info.priming as u64, info.padding as u64);
        let budget = limits.max_seconds.saturating_mul(info.sample_rate as u64);
        if total > budget && info.sample_rate > 0 {
            let rate = info.sample_rate as u64;
            return Err(limit("maxSeconds", total / rate, limits.max_seconds));
        }
        if priming + padding > total {
            return Err(CoreError::new(
                "MEDIA_CORRUPT",
                format!(
                    "the audio track's priming ({priming}) and padding ({padding}) do not fit its \
                     {total} samples"
                ),
            )
            .with_details(json!({
                "at": "edts", "format": "mp4", "priming": priming, "padding": padding,
                "samples": total,
            })));
        }
        let dec = open_audio_decoder(&info, &AudioConfig::Record(config))?;
        Ok(AudioReader {
            trim: Trim {
                skip: priming,
                left: Some(total - priming - padding),
            },
            info,
            kind: Kind::Mp4,
            packets: Packets::Mp4 {
                src,
                dec,
                buf: Vec::new(),
            },
            layout: None,
            declared: Some(total - priming - padding),
            exact: false,
            decoded: 0,
            max_frames: 0,
            out: 0,
            ended: false,
        })
    }

    fn open_symphonia<R: Read + Seek + Send + Sync + 'static>(
        src: Source<R>,
        kind: Kind,
        layout: Option<ChannelLayout>,
        limits: &AudioLimits,
    ) -> Result<Self, CoreError> {
        let format = guarded("reading the container", || {
            let mss = MediaSourceStream::new(Box::new(src), MediaSourceStreamOptions::default());
            symphonia::default::get_probe()
                .probe(
                    &Hint::new(),
                    mss,
                    FormatOptions::default(),
                    MetadataOptions::default(),
                )
                .map_err(|e| error(e, true))
        })?;
        let corrupt = |what: &str| {
            CoreError::new("MEDIA_CORRUPT", what.to_string())
                .with_details(json!({ "at": "header", "format": kind.name() }))
        };
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| corrupt("the file has no audio track"))?;
        let Some(CodecParameters::Audio(params)) = &track.codec_params else {
            return Err(corrupt("the audio track has no parameters"));
        };
        let codec = codec_of(params.codec);
        let info = AudioTrackInfo {
            codec,
            fourcc: fourcc(codec).to_string(),
            sample_rate: params.sample_rate.unwrap_or(0),
            channels: params.channels.as_ref().map_or(0, |c| c.count()) as u16,
            priming: track.delay.unwrap_or(0),
            padding: track.padding.unwrap_or(0),
        };
        let (id, declared) = (track.id, track.num_frames);
        check_bounds(&info, declared, limits)?;
        symphonia_decodes(info.codec)?;
        let dec = SymphoniaDecoder::open(params)?;
        Ok(AudioReader {
            layout: layout.filter(|l| l.channels() == info.channels as usize),
            info,
            kind,
            packets: Packets::Symphonia {
                format,
                track: id,
                dec,
            },
            declared,
            exact: false,
            decoded: 0,
            max_frames: 0,
            trim: Trim::default(),
            out: 0,
            ended: false,
        })
    }

    /// The track's own bounds, and the frame budget.
    fn check(mut self, limits: &AudioLimits) -> Result<Self, CoreError> {
        check_bounds(&self.info, self.declared, limits)?;
        self.max_frames = limits
            .max_seconds
            .saturating_mul(self.info.sample_rate as u64);
        Ok(self)
    }

    pub fn info(&self) -> &AudioTrackInfo {
        &self.info
    }

    /// The length the container states, in frames at the source rate, when it states one.
    pub fn declared_frames(&self) -> Option<u64> {
        self.declared
    }

    /// The backend that decodes (Decision 24's name).
    pub fn backend(&self) -> BackendId {
        match &self.packets {
            Packets::Mp4 { dec, .. } => dec.backend(),
            Packets::Symphonia { .. } => BACKEND,
        }
    }

    /// The decoder's vendor class (the cache key's `decoderClass`, Decision 7).
    pub fn decoder_class(&self) -> VendorClass {
        audio_backend(self.backend().0).map_or(VendorClass::Plugin, |b| b.class)
    }

    /// The next decoded frames, `None` after the last. Priming and padding are already trimmed;
    /// `pts` counts frames from the first audible one. After an error the reader is done.
    // The frozen interface's name; fallible, so not `Iterator::next`.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<DecodedAudio>, CoreError> {
        while !self.ended {
            let step = match self.read() {
                Ok(Some(d)) => self.emit(d),
                Ok(None) => {
                    self.ended = true;
                    return self.finish().map(|()| None);
                }
                Err(e) => Err(e),
            };
            if self.trim.done() {
                // The presented length is reached: the rest is padding, not decoded.
                self.ended = true;
            }
            match step {
                Ok(Some(out)) => return Ok(Some(out)),
                Ok(None) if self.ended => return self.finish().map(|()| None),
                Ok(None) => {}
                Err(e) => {
                    self.ended = true;
                    return Err(e);
                }
            }
        }
        Ok(None)
    }

    /// The next decoded chunk from the source, before trimming; `None` at the end.
    fn read(&mut self) -> Result<Option<DecodedAudio>, CoreError> {
        loop {
            match &mut self.packets {
                Packets::Mp4 { src, dec, buf } => {
                    if let Some(d) = dec.receive()? {
                        return Ok(Some(d));
                    }
                    match src.next(buf)? {
                        Some(pts) => dec.send(buf, pts)?,
                        None => {
                            dec.finish()?;
                            return dec.receive();
                        }
                    }
                }
                Packets::Symphonia { format, track, dec } => {
                    let packet = guarded("reading the container", || {
                        format.next_packet().map_err(|e| error(e, true))
                    })?;
                    let Some(packet) = packet else {
                        return Ok(None);
                    };
                    if packet.track_id != *track {
                        continue;
                    }
                    dec.decode(&packet)?;
                    if let Some(d) = dec.receive()? {
                        return Ok(Some(d));
                    }
                }
            }
        }
    }

    /// Checks, trims and numbers one chunk; `None` when trimming left nothing.
    fn emit(&mut self, mut d: DecodedAudio) -> Result<Option<DecodedAudio>, CoreError> {
        let n = d.layout.channels();
        if n != self.info.channels as usize || d.rate != self.info.sample_rate {
            return Err(CoreError::new(
                "DECODE_FAILED",
                "the decoded audio changed its channels or rate mid-stream",
            )
            .with_details(json!({
                "backend": self.backend().0, "channels": n, "rate": d.rate,
            })));
        }
        if let Some(l) = &self.layout {
            d.layout = l.clone();
        }
        // The budget counts what was decoded, before the trim drops anything.
        self.decoded += (d.interleaved.len() / n) as u64;
        if self.decoded > self.max_frames {
            let rate = self.info.sample_rate as u64;
            return Err(limit(
                "maxSeconds",
                self.decoded / rate,
                self.max_frames / rate,
            ));
        }
        d.interleaved = self.trim.push(&d.interleaved, n);
        let frames = (d.interleaved.len() / n) as u64;
        if frames == 0 {
            return Ok(None);
        }
        d.pts = self.out as i64;
        self.out += frames;
        if self.out > self.max_frames {
            let rate = self.info.sample_rate as u64;
            return Err(limit("maxSeconds", self.out / rate, self.max_frames / rate));
        }
        Ok(Some(d))
    }

    /// At the end: a container that stated its length must have delivered all of it.
    fn finish(&mut self) -> Result<(), CoreError> {
        match self.declared {
            Some(want) if self.exact && self.out < want => Err(CoreError::new(
                "MEDIA_CORRUPT",
                format!("the audio ends early: {} of {want} frames", self.out),
            )
            .with_details(json!({
                "at": "end", "format": self.kind.name(), "frames": self.out, "declared": want,
            }))),
            _ => Ok(()),
        }
    }
}

/// Channels, rate and stated length against `limits`; a track without a rate or channels is
/// corrupt.
fn check_bounds(
    info: &AudioTrackInfo,
    declared: Option<u64>,
    limits: &AudioLimits,
) -> Result<(), CoreError> {
    if info.sample_rate == 0 || info.channels == 0 {
        return Err(CoreError::new(
            "MEDIA_CORRUPT",
            "the audio track has no sample rate or no channels",
        )
        .with_details(json!({ "at": "header" })));
    }
    if info.channels > limits.max_channels {
        return Err(limit(
            "maxChannels",
            info.channels as u64,
            limits.max_channels as u64,
        ));
    }
    if info.sample_rate > limits.max_rate {
        return Err(limit(
            "maxRate",
            info.sample_rate as u64,
            limits.max_rate as u64,
        ));
    }
    let max = limits.max_seconds.saturating_mul(info.sample_rate as u64);
    if let Some(n) = declared.filter(|&n| n > max) {
        return Err(limit(
            "maxSeconds",
            n / info.sample_rate as u64,
            limits.max_seconds,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Trim;

    #[test]
    fn trim_skips_priming_and_stops_at_the_presented_length() {
        let mut t = Trim {
            skip: 3,
            left: Some(5),
        };
        let pcm: Vec<f32> = (0..10).map(|i| i as f32).collect();
        // Mono: frames 0..10 in chunks of 4, 4, 2.
        let mut out = t.push(&pcm[..4], 1);
        out.extend(t.push(&pcm[4..8], 1));
        out.extend(t.push(&pcm[8..], 1));
        assert_eq!(out, vec![3.0, 4.0, 5.0, 6.0, 7.0]);
        assert!(t.done());
    }
}
