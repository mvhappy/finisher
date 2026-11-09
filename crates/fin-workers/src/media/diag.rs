//! The `media` family's diagnostics (spec §15, 6A plan Task 14): the `media` object of
//! `job.done` (carried in `LookStats.media`, read by `finisherd` when the job ends), the
//! `media.capabilities` event and `render.stats`. Everything here is built only when the
//! job's `start.diagnostics` is on; with it off, nothing is measured into a payload, nothing
//! is built and nothing is sent.
//!
//! No path, project or file name, no adapter or driver name string: a codec and a backend are
//! names from `fin-core`'s tables, the GPU is its PCI ids and the driver's major version.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use fin_codecs::Support;
use fin_core::Rational;
use fin_core::events::names_media::{MEDIA_BACKENDS, MEDIA_CODECS, backend_class};
use fin_core::events::payloads::{
    AdapterType, BackendEntry, CodecEntry, CopyPath, GpuEntry, Histogram, JobMedia,
    MediaCapabilitiesEvent, RenderPhase, RenderStats, ResolutionBucket, Sandbox, SupportClass,
};
use fin_jobs::worker::JobCtx;
use fin_media::VideoCodec;
use fin_media::jobs::{CapSupport, GpuAdapter, LookFrame, MediaCapabilities};

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

/// The `media` object of the look's `job.done`.
pub fn job_media(f: &JobFacts<'_>) -> JobMedia {
    let (codec, backend) = f
        .decode
        .map_or(("none", "none"), |(c, b)| (codec_name(c), b));
    let largest = f
        .frames
        .iter()
        .max_by_key(|fr| u64::from(fr.width) * u64::from(fr.height));
    let frames = u32::try_from(f.frames.len()).unwrap_or(u32::MAX);
    let rt_factor_milli = match f.rate {
        Some(rate) if f.render_micros > 0 && rate.to_f64() > 0.0 => {
            let media_s = f64::from(frames) / rate.to_f64();
            let wall_s = f.render_micros as f64 / 1e6;
            (media_s / wall_s * 1000.0)
                .round()
                .clamp(0.0, f64::from(u32::MAX)) as u32
        }
        _ => 0,
    };
    JobMedia {
        codec: MEDIA_CODECS.get_or_other(codec),
        backend: MEDIA_BACKENDS.get_or_other(backend),
        hw: f.hw,
        resolution: largest.map_or(ResolutionBucket::Sd, |fr| {
            ResolutionBucket::of(fr.width, fr.height)
        }),
        frames,
        rt_factor_milli,
        cache_hit: f.sources_reused > 0 && f.sources_opened == 0,
        copy_path: CopyPath::Copy,
    }
}

/// Whether the machine decodes `codec` in hardware.
pub fn decodes_in_hardware(caps: &fin_codecs::Capabilities, codec: VideoCodec) -> bool {
    caps.get(codec)
        .is_some_and(|c| c.decode == Support::Hardware)
}

fn support(s: CapSupport) -> SupportClass {
    match s {
        CapSupport::Hardware => SupportClass::Hw,
        CapSupport::Software => SupportClass::Sw,
        CapSupport::None => SupportClass::None,
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

/// The `media.capabilities` event for a capabilities answer.
pub fn capabilities_event(caps: &MediaCapabilities) -> MediaCapabilitiesEvent {
    MediaCapabilitiesEvent {
        codecs: caps
            .codecs
            .video
            .iter()
            .map(|c| CodecEntry {
                codec: MEDIA_CODECS.get_or_other(codec_name(c.codec)),
                decode: support(c.decode),
                encode: support(c.encode),
                backends: c
                    .backends
                    .iter()
                    .map(|b| BackendEntry {
                        name: MEDIA_BACKENDS.get_or_other(b),
                        class: backend_class(b),
                    })
                    .collect(),
            })
            .collect(),
        gpu: caps.gpu.as_ref().map(gpu_entry),
        copy_path: CopyPath::Copy,
        // sp6 §15: `landlock` once the worker profile's Landlock phase restricted this process
        // (Linux, no seccomp yet); `worker` once the full profile did (macOS Seatbelt: files,
        // no network, no process creation).
        sandbox: if fin_sandbox::self_apply::applied() {
            if cfg!(target_os = "macos") {
                Sandbox::Worker
            } else {
                Sandbox::Landlock
            }
        } else {
            Sandbox::None
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
}
