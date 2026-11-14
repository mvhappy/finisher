//! The `media` family's diagnostics (spec §15, 6A plan Task 14, 6B-1 plan Task 19): the `media`
//! object of `job.done` (carried in `LookStats.media`, and in the result's `media` for
//! `media.proxy`, `media.thumbs` and `media.probeDeep`; read by `finisherd` when the job ends),
//! the `media.capabilities` v2 event and `render.stats` (`encode` rows from proxies). Everything here is built only when the
//! job's `start.diagnostics` is on; with it off, nothing is measured into a payload, nothing
//! is built and nothing is sent.
//!
//! No path, project or file name, no adapter or driver name string: a codec and a backend are
//! names from `fin-core`'s tables, the GPU is its PCI ids and the driver's major version.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use fin_codecs::Support;
use fin_core::Rational;
use fin_core::events::names_media::{MEDIA_AUDIO_CODECS, MEDIA_PROFILES};
use fin_core::events::names_media::{MEDIA_BACKENDS, MEDIA_CODECS, backend_class};
use fin_core::events::payloads::{
    AdapterIds, AdapterType, AudioCodecEntry, BackendEntry, CodecDirection, CodecEntry, CopyPath,
    EncoderEntry, GapCode as EventGuidance, GpuEntry, GuidanceBatch, GuidanceEntry, Histogram,
    JobMedia, MediaCapabilitiesEvent, RenderPhase, RenderStats, ResolutionBucket, Sandbox,
    SupportClass,
};
use fin_jobs::worker::JobCtx;
use fin_media::jobs::{
    Batch, CapBackend, CapLevel, Direction, GpuAdapter, Guidance, GuidanceCode, LookFrame,
    MediaCapabilities, WorkerSandbox,
};
use fin_media::{AudioCodec, VideoCodec};

/// A `render.stats` window: one minute.
pub const WINDOW_MS: u64 = 60_000;

/// The stage samples a warm worker has measured since its last flush, by minute, phase and
/// backend.
#[derive(Default)]
pub struct RenderBook {
    cells: BTreeMap<(u64, RenderPhase, String), Histogram>,
}

impl RenderBook {
    /// Adds one sample taken now.
    pub fn record(&mut self, phase: RenderPhase, backend: &str, micros: u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
        self.record_at(now, phase, backend, micros);
    }

    /// Adds one sample taken at `now_ms` (ms since the epoch).
    pub fn record_at(&mut self, now_ms: u64, phase: RenderPhase, backend: &str, micros: u64) {
        let start = now_ms - now_ms % WINDOW_MS;
        let h = self
            .cells
            .entry((start, phase, backend.to_owned()))
            .or_insert_with(|| Histogram {
                start,
                window_ms: WINDOW_MS,
                ..Histogram::default()
            });
        h.count += 1;
        h.max_micros = h.max_micros.max(micros);
        let bucket = Histogram::bucket(micros);
        if h.buckets.len() <= bucket {
            h.buckets.resize(bucket + 1, 0);
        }
        h.buckets[bucket] += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Everything recorded, oldest window first; the book is empty afterwards.
    pub fn take(&mut self) -> Vec<RenderStats> {
        std::mem::take(&mut self.cells)
            .into_iter()
            .map(|((_, phase, backend), histogram)| RenderStats {
                phase,
                backend: MEDIA_BACKENDS.get_or_other(&backend),
                histogram,
            })
            .collect()
    }
}

/// Sends what the book holds as `render.stats` (diagnostics on), or drops it (off).
pub fn flush(book: &mut RenderBook, ctx: &mut JobCtx<'_>) {
    if book.is_empty() {
        return;
    }
    if !ctx.diagnostics() {
        book.take();
        return;
    }
    for stats in book.take() {
        ctx.diag(
            "render.stats",
            serde_json::to_value(&stats).expect("render stats serialise"),
        );
    }
}

/// `VideoCodec`'s name in `MEDIA_CODECS`.
fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::Av1 => "av1",
        VideoCodec::H264 => "h264",
        VideoCodec::Hevc => "hevc",
        VideoCodec::Vp9 => "vp9",
        VideoCodec::Vp8 => "vp8",
        VideoCodec::ProRes => "prores",
        VideoCodec::Other => "other",
    }
}

/// What a finished look knows about itself.
pub struct JobFacts<'a> {
    pub frames: &'a [LookFrame],
    /// The first source decoded: its codec and the decoder's backend.
    pub decode: Option<(VideoCodec, &'static str)>,
    pub hw: bool,
    /// The timeline's frame rate.
    pub rate: Option<Rational>,
    /// Decode + composite + readback, in micros.
    pub render_micros: u64,
    pub sources_opened: u32,
    pub sources_reused: u32,
}

/// Media seconds per wall second in thousandths: `frames` at `rate` over `micros`; `0` with no
/// rate or no measurable time.
pub fn rt_factor_milli(frames: u64, rate: Option<Rational>, micros: u64) -> u32 {
    match rate {
        Some(rate) if micros > 0 && rate.to_f64() > 0.0 => {
            let media_s = frames as f64 / rate.to_f64();
            let wall_s = micros as f64 / 1e6;
            (media_s / wall_s * 1000.0)
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32
        }
        _ => 0,
    }
}

/// The `media` object of the look's `job.done`.
pub fn job_media(f: &JobFacts<'_>) -> JobMedia {
    let largest = f
        .frames
        .iter()
        .max_by_key(|fr| u64::from(fr.width) * u64::from(fr.height))
        .map(|fr| (fr.width, fr.height));
    let frames = u32::try_from(f.frames.len()).unwrap_or(u32::MAX);
    JobMedia {
        cache_hit: f.sources_reused > 0 && f.sources_opened == 0,
        ..run_media(&RunFacts {
            decode: f.decode,
            hw: f.hw,
            largest,
            frames,
            rt_factor_milli: rt_factor_milli(u64::from(frames), f.rate, f.render_micros),
        })
    }
}

/// What a cold media job (`media.proxy`, `media.thumbs`, `media.probeDeep`; 6B-1 Task 19) knows
/// about itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct RunFacts {
    /// The stream decoded: its codec and the decoder's backend.
    pub decode: Option<(VideoCodec, &'static str)>,
    /// The decoder ran on hardware.
    pub hw: bool,
    /// The largest picture the job produced.
    pub largest: Option<(u32, u32)>,
    pub frames: u32,
    pub rt_factor_milli: u32,
}

/// The `media` object of a cold job's `job.done`: no warm worker, so never a cache hit.
pub fn run_media(f: &RunFacts) -> JobMedia {
    let (codec, backend) = f
        .decode
        .map_or(("none", "none"), |(c, b)| (codec_name(c), b));
    JobMedia {
        codec: MEDIA_CODECS.get_or_other(codec),
        backend: MEDIA_BACKENDS.get_or_other(backend),
        hw: f.hw,
        resolution: f
            .largest
            .map_or(ResolutionBucket::Sd, |(w, h)| ResolutionBucket::of(w, h)),
        frames: f.frames,
        rt_factor_milli: f.rt_factor_milli,
        cache_hit: false,
        copy_path: CopyPath::Copy,
    }
}

/// What an audio job (`media.pcm`, `media.waveform`, `media.listen`) knows about itself.
pub struct AudioFacts<'a> {
    /// The source's codec (`MEDIA_AUDIO_CODECS`; `pcm` for a feed read back from the cache).
    pub codec: &'a str,
    /// The decoder's backend (`MEDIA_BACKENDS`; `none` when nothing was decoded).
    pub backend: &'a str,
    /// Samples per channel the job produced or measured.
    pub samples: u64,
    /// Their rate.
    pub rate: u32,
    /// Decode + mix + measure (or write), in micros.
    pub wall_micros: u64,
    /// Every source was read from a cache entry.
    pub cache_hit: bool,
}

/// The `media` object of an audio job's `job.done` (6C Task 22): `frames` is samples ÷ 1,024
/// (the canonical block), `hw` is always false (no audio hardware path), the resolution `sd`
/// (there is no picture) and `rtFactorMilli` the media seconds over the wall seconds.
pub fn audio_media(f: &AudioFacts<'_>) -> JobMedia {
    let rt_factor_milli = if f.wall_micros > 0 && f.rate > 0 {
        let media_s = f.samples as f64 / f64::from(f.rate);
        (media_s / (f.wall_micros as f64 / 1e6) * 1000.0)
            .round()
            .clamp(0.0, f64::from(u32::MAX)) as u32
    } else {
        0
    };
    JobMedia {
        codec: MEDIA_AUDIO_CODECS.get_or_other(f.codec),
        backend: MEDIA_BACKENDS.get_or_other(f.backend),
        hw: false,
        resolution: ResolutionBucket::Sd,
        frames: u32::try_from(f.samples / 1024).unwrap_or(u32::MAX),
        rt_factor_milli,
        cache_hit: f.cache_hit,
        copy_path: CopyPath::Copy,
    }
}

/// `AudioCodec`'s name in `MEDIA_AUDIO_CODECS` (`other`: the table's fallback).
pub fn audio_codec_name(codec: AudioCodec) -> &'static str {
    match codec {
        AudioCodec::Aac => "aac",
        AudioCodec::Opus => "opus",
        AudioCodec::Mp3 => "mp3",
        AudioCodec::Flac => "flac",
        AudioCodec::Alac => "alac",
        AudioCodec::Pcm => "pcm",
        AudioCodec::Ac3 => "ac3",
        AudioCodec::Eac3 => "eac3",
        AudioCodec::Vorbis => "vorbis",
        AudioCodec::Other => "other",
    }
}

/// Whether the machine decodes `codec` in hardware.
pub fn decodes_in_hardware(caps: &fin_codecs::Capabilities, codec: VideoCodec) -> bool {
    caps.get(codec)
        .is_some_and(|c| c.decode == Support::Hardware)
}

fn support(s: CapLevel) -> SupportClass {
    match s {
        CapLevel::Hw => SupportClass::Hw,
        CapLevel::Sw => SupportClass::Sw,
        CapLevel::None => SupportClass::None,
    }
}

/// A name as its camel-case serde form (`Profile`, `AudioCodec`): the tables use the same.
fn wire_name<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|v| v.as_str().map(String::from))
        .unwrap_or_default()
}

fn backend_entry(b: &CapBackend) -> BackendEntry {
    BackendEntry {
        name: MEDIA_BACKENDS.get_or_other(&b.name),
        // The journal's class is the name table's, so a name outside it is `other` throughout.
        class: backend_class(&b.name),
        direction: match b.direction {
            Direction::Decode => CodecDirection::Decode,
            Direction::Encode => CodecDirection::Encode,
        },
        hw: b.hw,
    }
}

/// A gap's code and batch: the `fix` text is the answer's, never the journal's.
fn guidance_entry(g: &Guidance) -> GuidanceEntry {
    GuidanceEntry {
        code: match g.code {
            GuidanceCode::HevcExtensions => EventGuidance::HevcExtensions,
            GuidanceCode::Av1Extension => EventGuidance::Av1Extension,
            GuidanceCode::MediaFeaturePack => EventGuidance::MediaFeaturePack,
            GuidanceCode::LtscStoreCodecs => EventGuidance::LtscStoreCodecs,
            GuidanceCode::GpuDriver => EventGuidance::GpuDriver,
            GuidanceCode::CodecPlugin => EventGuidance::CodecPlugin,
            GuidanceCode::LaterBatch => EventGuidance::LaterBatch,
        },
        batch: g.batch.map(|b| match b {
            Batch::B6b2 => GuidanceBatch::B6b2,
            Batch::B6c => GuidanceBatch::B6c,
            Batch::B6d => GuidanceBatch::B6d,
            Batch::B6e => GuidanceBatch::B6e,
            Batch::B6f => GuidanceBatch::B6f,
            Batch::B6g => GuidanceBatch::B6g,
        }),
    }
}

/// The GPU as `media.capabilities` and `render.stats` name it: numeric ids, no name strings.
pub fn gpu_entry(a: &GpuAdapter) -> GpuEntry {
    GpuEntry {
        backend: MEDIA_BACKENDS.get_or_other(&a.backend),
        adapter_type: AdapterType::from_wgpu(&a.device_type),
        vendor_id: a.vendor,
        device_id: a.device,
        driver_major: a.driver_major,
        software: a.software,
    }
}

/// The `media.capabilities` v2 event for a v2 report: names through `fin-core`'s tables,
/// guidance as codes, adapters as numeric ids; no free-form string of the report (an adapter's
/// or MFT's name, a guidance text, the worker's version) reaches it.
pub fn capabilities_event(caps: &MediaCapabilities) -> MediaCapabilitiesEvent {
    MediaCapabilitiesEvent {
        codecs: caps
            .codecs
            .iter()
            .map(|c| CodecEntry {
                codec: MEDIA_CODECS.get_or_other(codec_name(c.codec)),
                profile: c
                    .profile
                    .map(|p| MEDIA_PROFILES.get_or_other(&wire_name(&p))),
                bit_depth: c.bit_depth,
                decode: support(c.decode),
                encode: support(c.encode),
                backends: c.backends.iter().map(backend_entry).collect(),
                guidance: c.guidance.iter().map(guidance_entry).collect(),
            })
            .collect(),
        audio: caps
            .audio
            .iter()
            .map(|a| AudioCodecEntry {
                codec: MEDIA_AUDIO_CODECS.get_or_other(audio_codec_name(a.codec)),
                decode: support(a.decode),
                encode: support(a.encode),
                backends: a.backends.iter().map(backend_entry).collect(),
                guidance: a.guidance.iter().map(guidance_entry).collect(),
            })
            .collect(),
        encoders: caps
            .encoders
            .iter()
            .map(|e| EncoderEntry {
                backend: MEDIA_BACKENDS.get_or_other(&e.backend),
                class: backend_class(&e.backend),
                codec: MEDIA_CODECS.get_or_other(codec_name(e.codec)),
                hw: e.hw,
                intra_only: e.intra_only,
                intra_fps: e.intra_fps,
                session_limit: e.session_limit,
                adapter: e.adapter.map(|a| AdapterIds {
                    vendor_id: a.vendor_id,
                    device_id: a.device_id,
                    driver_major: a.driver_major,
                }),
            })
            .collect(),
        gpu: caps.gpu.as_ref().map(gpu_entry),
        copy_path: CopyPath::Copy,
        // sp6 §15: the answer's (`caps::sandbox`).
        sandbox: match caps.worker.sandbox {
            WorkerSandbox::None => Sandbox::None,
            WorkerSandbox::Landlock => Sandbox::Landlock,
            WorkerSandbox::Worker => Sandbox::Worker,
        },
        render_version: caps.worker.render_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_land_in_their_minute_phase_and_backend() {
        let mut book = RenderBook::default();
        let minute = 7 * WINDOW_MS;
        book.record_at(minute + 5, RenderPhase::Decode, "dav1d", 0);
        book.record_at(minute + 59_999, RenderPhase::Decode, "dav1d", 1_000);
        book.record_at(minute + 60_000, RenderPhase::Decode, "dav1d", 1_000);
        book.record_at(minute, RenderPhase::Composite, "vulkan", 3);
        book.record_at(minute, RenderPhase::Decode, "not a backend", 3);
        let out = book.take();
        assert!(book.is_empty());
        assert_eq!(out.len(), 4);
        let first = out
            .iter()
            .find(|s| s.phase == RenderPhase::Decode && s.backend.as_str() == "dav1d")
            .unwrap();
        assert_eq!(first.histogram.start, minute);
        assert_eq!(first.histogram.window_ms, WINDOW_MS);
        assert_eq!(first.histogram.count, 2);
        assert_eq!(first.histogram.max_micros, 1_000);
        // 0 µs is bucket 0; 1,000 µs is 2⁹ ≤ 1000 < 2¹⁰, bucket 9; trailing buckets are not kept.
        assert_eq!(first.histogram.buckets.len(), 10);
        assert_eq!(
            (first.histogram.buckets[0], first.histogram.buckets[9]),
            (1, 1)
        );
        assert!(out.iter().any(|s| s.backend.as_str() == "other"));
        assert!(out.iter().any(|s| s.histogram.start == minute + WINDOW_MS));
    }

    #[test]
    fn rt_factor_is_media_seconds_over_wall_seconds() {
        let frame = |w, h| LookFrame {
            at: Rational::new(0, 1).unwrap(),
            file: "x".into(),
            width: w,
            height: h,
            plan: String::new(),
            source: fin_media::jobs::LookSource::Original,
            proxy_fallbacks: Vec::new(),
        };
        let frames = [frame(1920, 1080), frame(640, 360)];
        let facts = JobFacts {
            frames: &frames,
            decode: Some((VideoCodec::Av1, "dav1d")),
            hw: false,
            rate: Some(Rational::new(30, 1).unwrap()),
            render_micros: 10_000,
            sources_opened: 0,
            sources_reused: 2,
        };
        let m = job_media(&facts);
        // 2 frames at 30 fps = 1/15 s of media in 10 ms of wall time.
        assert_eq!(m.rt_factor_milli, 6_667);
        assert_eq!((m.codec.as_str(), m.backend.as_str()), ("av1", "dav1d"));
        assert_eq!(
            (m.resolution, m.frames, m.cache_hit),
            (ResolutionBucket::Hd1080, 2, true)
        );
        let none = job_media(&JobFacts {
            decode: None,
            render_micros: 0,
            sources_reused: 0,
            ..facts
        });
        assert_eq!(none.rt_factor_milli, 0);
        assert_eq!(
            (none.codec.as_str(), none.backend.as_str()),
            ("none", "none")
        );
        assert!(!none.cache_hit);
    }

    #[test]
    fn audio_media_counts_samples_over_1024_and_real_time_over_wall_time() {
        // One second of 48 kHz audio decoded in 10 ms: 100x real time.
        let m = audio_media(&AudioFacts {
            codec: "aac",
            backend: "symphonia",
            samples: 48_000,
            rate: 48_000,
            wall_micros: 10_000,
            cache_hit: false,
        });
        assert_eq!((m.codec.as_str(), m.backend.as_str()), ("aac", "symphonia"));
        assert_eq!(
            (m.hw, m.resolution, m.frames, m.rt_factor_milli, m.cache_hit),
            (false, ResolutionBucket::Sd, 46, 100_000, false)
        );
        // A feed read back from the cache decoded nothing; no measurable time reads 0.
        let hit = audio_media(&AudioFacts {
            codec: "pcm",
            backend: "none",
            samples: 16_000,
            rate: 16_000,
            wall_micros: 0,
            cache_hit: true,
        });
        assert_eq!((hit.codec.as_str(), hit.backend.as_str()), ("pcm", "none"));
        assert_eq!(
            (hit.frames, hit.rt_factor_milli, hit.cache_hit),
            (15, 0, true)
        );
        // A codec outside the table reads as `other`.
        let odd = audio_media(&AudioFacts {
            codec: "wavpack",
            backend: "symphonia",
            samples: 1,
            rate: 48_000,
            wall_micros: 1,
            cache_hit: false,
        });
        assert_eq!(odd.codec.as_str(), "other");
    }

    #[test]
    fn audio_phases_land_in_render_stats() {
        let mut book = RenderBook::default();
        for phase in [
            RenderPhase::DecodeAudio,
            RenderPhase::Mix,
            RenderPhase::EncodeAudio,
        ] {
            book.record_at(WINDOW_MS, phase, "symphonia", 5);
        }
        let out = book.take();
        assert_eq!(out.len(), 3);
        assert!(out.iter().any(|s| s.phase == RenderPhase::Mix));
    }
}
