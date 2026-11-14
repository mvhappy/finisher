//! The proxy's audio track (6C plan Task 20; sub-project 6 spec §7.1, proxy recipe v2).
//!
//! The source's first audio track through [`DecodedSource`] (48 kHz, its canonical layout: a
//! mono source stays one channel, 3 to 8 channels are downmixed to stereo), encoded with libopus
//! through [`open_audio_encoder`] under `EncodePurpose::Proxy` at
//! [`ProxyAudio::STEREO_BITRATE`] or [`ProxyAudio::MONO_BITRATE`]. The whole track is encoded
//! before the video, because the muxer takes its presented length when the track is added
//! (Decision 22: `presented = EncodedLength.valid`, media time = the encoder's priming). The
//! packets go to a scratch file in the job's `out/` (beside the entry's directory, never in
//! it: 5C empties `out/` before each attempt and removes the job directory after the job), so
//! only their sizes are held in memory (4 bytes a 20 ms packet); `encode_pipeline` reads them
//! back in order beside the video, again from the first one when the video encode restarts.
//!
//! The decoder opened must be of the class the key names (`ProxySourceAudio::Decoded`, else
//! `PLAN_MISMATCH {reason: cacheKey}`). A decode that fails (`CODEC_UNAVAILABLE`: a profile or
//! layout the decoder refuses; `UNSUPPORTED_CONTAINER`; `MEDIA_LIMIT`; `MEDIA_CORRUPT`;
//! `DECODE_FAILED`) gives a video-only proxy that says why ([`Outcome::Skipped`]): the preview
//! keeps its frames, and the mix reads `pcm48`, which reports the failure as its own.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use fin_codecs::audio::{AudioEncoder, AudioEncoderConfig, open_audio_encoder};
use fin_container::AudioTrackConfig;
use fin_core::CoreError;
use fin_media::AudioCodec;
use fin_media::backend::{EncodePurpose, VendorClass};
use fin_media::jobs::{PcmLayout, PcmRecipe, ProxyAudio, ProxyAudioCodec};
use fin_shm::JobDir;
use serde_json::json;

use super::audio_src::DecodedSource;
use super::inputs;
use super::proxy::io_error;

/// The proxy's rate (spec §7.1: 48 kHz).
const RATE: u32 = 48_000;

/// The encoded track: its muxer configuration and its packets, in order, in `path`.
#[derive(Debug)]
pub struct AudioPackets {
    pub track: AudioTrackConfig,
    /// Samples each packet carries (its duration in the track's 48 kHz timescale).
    pub frame: u32,
    pub result: ProxyAudio,
    path: PathBuf,
    job: JobDir,
    /// Each packet's size, in order.
    sizes: Vec<u32>,
}

impl AudioPackets {
    pub fn len(&self) -> usize {
        self.sizes.len()
    }

    /// The packets from the first, read back in order.
    pub fn reader(&self) -> Result<PacketReader<'_>, CoreError> {
        let bytes = self.sizes.iter().map(|s| *s as u64).sum();
        let f =
            fin_shm::checked_open(&self.path, bytes).map_err(|e| inputs::relative(&self.job, e))?;
        Ok(PacketReader {
            a: self,
            r: BufReader::with_capacity(1 << 16, f),
            buf: Vec::new(),
        })
    }

    /// Removes the scratch file (best effort: 5C removes the job directory anyway).
    pub fn remove(self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// [`AudioPackets`] read back one packet at a time.
pub struct PacketReader<'a> {
    a: &'a AudioPackets,
    r: BufReader<File>,
    buf: Vec<u8>,
}

impl PacketReader<'_> {
    /// Packet `i`, which must be the one after the last read (`i` from 0).
    pub fn next(&mut self, i: usize) -> Result<&[u8], CoreError> {
        self.buf.resize(self.a.sizes[i] as usize, 0);
        self.r
            .read_exact(&mut self.buf)
            .map_err(|e| inputs::relative(&self.a.job, io_error(&self.a.path, e)))?;
        Ok(&self.buf)
    }
}

/// When the audio pass's next progress report is due: at once, then at most once an `every`.
pub struct Throttle {
    every: std::time::Duration,
    last: Option<std::time::Instant>,
}

impl Throttle {
    pub fn new(every: std::time::Duration) -> Throttle {
        Throttle { every, last: None }
    }

    /// Whether a report is due at `now` (and if so, counts it as sent).
    pub fn due(&mut self, now: std::time::Instant) -> bool {
        if self
            .last
            .is_some_and(|t| now.saturating_duration_since(t) < self.every)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

/// What became of the source's audio.
#[derive(Debug)]
pub enum Outcome {
    Track(AudioPackets),
    /// Not decoded: the decode's error code (module docs).
    Skipped(String),
    /// A track with no samples.
    Empty,
}

/// The codes of a decode that failed (module docs).
fn skippable(e: &CoreError) -> bool {
    matches!(
        e.code.as_str(),
        "CODEC_UNAVAILABLE"
            | "UNSUPPORTED_CONTAINER"
            | "MEDIA_LIMIT"
            | "MEDIA_CORRUPT"
            | "DECODE_FAILED"
    )
}

fn encode_failed(message: String, details: serde_json::Value) -> CoreError {
    CoreError::new("ENCODE_FAILED", message).with_details(details)
}

/// A decode's failure as `Err(code)`, anything else (I/O, cancel) as the error it is.
fn decode<T>(r: Result<T, CoreError>) -> Result<Result<T, String>, CoreError> {
    match r {
        Ok(v) => Ok(Ok(v)),
        Err(e) if skippable(&e) => Ok(Err(e.code)),
        Err(e) => Err(e),
    }
}

/// `file`'s audio as the proxy's Opus track, its packets in `scratch` (a new file in `job`'s
/// `out/`, removed again when the audio is skipped); its decoder must be of `class`.
/// `between(frames)` runs before each decoded chunk with the 48 kHz frames done so far
/// (progress, cancel).
pub fn encode(
    file: File,
    class: VendorClass,
    job: &JobDir,
    scratch: &Path,
    mut between: impl FnMut(u64) -> Result<(), CoreError>,
) -> Result<Outcome, CoreError> {
    let mut src = match decode(DecodedSource::open(file, PcmRecipe::Pcm48))? {
        Ok(s) => s,
        Err(code) => return Ok(Outcome::Skipped(code)),
    };
    if src.decoder_class() != class {
        return Err(CoreError::new(
            "PLAN_MISMATCH",
            "the proxy's cache key does not match the audio decoder this worker opened",
        )
        .with_details(json!({
            "reason": "cacheKey", "audioDecoder": src.backend(),
            "audioClass": src.decoder_class(), "expected": class,
        })));
    }
    let (channels, bitrate) = match src.feed_layout() {
        PcmLayout::Mono => (1u16, ProxyAudio::MONO_BITRATE),
        PcmLayout::Stereo => (2, ProxyAudio::STEREO_BITRATE),
    };
    let cfg = AudioEncoderConfig {
        codec: AudioCodec::Opus,
        rate: RATE,
        channels,
        bitrate,
    };
    let mut enc = open_audio_encoder(&cfg, EncodePurpose::Proxy)
        .map_err(|e| encode_failed(e.message, json!({ "backend": "libopus", "cause": e.code })))?;
    let out = fin_shm::create_output(scratch).map_err(|e| inputs::relative(job, e))?;
    let mut w = Spill {
        w: BufWriter::with_capacity(1 << 16, out),
        path: scratch,
        job,
        sizes: Vec::new(),
    };
    let r = w.pump(&mut src, &mut *enc, channels, &mut between);
    let sent = match r {
        Ok(Ok(sent)) if sent > 0 => sent,
        other => {
            drop(w);
            let _ = std::fs::remove_file(scratch);
            return match other? {
                Ok(_) => Ok(Outcome::Empty),
                Err(code) => Ok(Outcome::Skipped(code)),
            };
        }
    };
    let len = enc.finish()?;
    w.take(&mut *enc)?;
    let sizes = w.finish()?;
    let frame = enc.frame_size();
    let decoded = sizes.len() as u64 * frame as u64;
    if len.valid != sent
        || decoded != len.priming as u64 + len.valid + len.padding as u64
        || len.priming != enc.priming()
    {
        return Err(encode_failed(
            format!(
                "the encoder's length ({} priming, {} valid, {} padding) is not the {sent} samples \
                 sent in {} packets",
                len.priming,
                len.valid,
                len.padding,
                sizes.len()
            ),
            json!({ "backend": "libopus" }),
        ));
    }
    Ok(Outcome::Track(AudioPackets {
        // Read after the stream: `dOps` carries the encoder's pre-skip.
        track: AudioTrackConfig {
            config: enc.config(),
            sample_rate: RATE,
            channels,
            priming: len.priming,
            presented: Some(len.valid),
        },
        frame,
        result: ProxyAudio {
            codec: ProxyAudioCodec::Opus,
            bitrate,
            priming: len.priming,
        },
        path: scratch.to_path_buf(),
        job: job.clone(),
        sizes,
    }))
}

/// The scratch file being written.
struct Spill<'a> {
    w: BufWriter<File>,
    path: &'a Path,
    job: &'a JobDir,
    sizes: Vec<u32>,
}

impl Spill<'_> {
    fn io(&self, e: impl std::fmt::Display) -> CoreError {
        inputs::relative(self.job, io_error(self.path, e))
    }

    /// Decodes and encodes every frame: the frames sent, or the decode's failure code.
    fn pump(
        &mut self,
        src: &mut DecodedSource,
        enc: &mut dyn AudioEncoder,
        channels: u16,
        between: &mut dyn FnMut(u64) -> Result<(), CoreError>,
    ) -> Result<Result<u64, String>, CoreError> {
        let mut sent = 0u64;
        loop {
            between(sent)?;
            let pcm = match decode(src.next_interleaved())? {
                Ok(Some(pcm)) => pcm,
                Ok(None) => return Ok(Ok(sent)),
                Err(code) => return Ok(Err(code)),
            };
            enc.send(&pcm, sent as i64)?;
            sent += (pcm.len() / channels as usize) as u64;
            self.take(enc)?;
        }
    }

    fn take(&mut self, enc: &mut dyn AudioEncoder) -> Result<(), CoreError> {
        while let Some(p) = enc.receive()? {
            let size = u32::try_from(p.data.len())
                .map_err(|_| encode_failed("a packet past 4 GiB".into(), json!({})))?;
            self.w.write_all(&p.data).map_err(|e| self.io(e))?;
            self.sizes.push(size);
        }
        Ok(())
    }

    fn finish(self) -> Result<Vec<u32>, CoreError> {
        let f = self
            .w
            .into_inner()
            .map_err(|e| inputs::relative(self.job, io_error(self.path, e.error())))?;
        f.sync_all()
            .map_err(|e| inputs::relative(self.job, io_error(self.path, e)))?;
        Ok(self.sizes)
    }
}

#[cfg(test)]
#[path = "proxy_audio_tests.rs"]
mod tests;
