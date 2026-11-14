//! The `media.capabilities` v2 report this worker can give before 6B-1 Task 8's capability
//! probe (plan 6B-1 Task 19): the rows of the backends it has (`ProbeResult`, 6A's dav1d and
//! SVT-AV1 everywhere until Tasks 4–8 add VideoToolbox and Media Foundation) per codec, profile
//! and bit depth, the guidance the plan fixes for this batch (Linux H.264, HEVC and VP9 decode:
//! 6B-2; AAC everywhere: 6C) and the encoders as they are, with no test clip run (`intraFps` and
//! `sessionLimit` absent: Task 8 measures them). It feeds the journal's `media.capabilities` v2;
//! the kind still answers v1 until Task 8 replaces this module with `probe::capabilities()`.

use fin_codecs::{ProbeResult, StreamDesc};
use fin_media::jobs::{
    AudioCodecCaps, Batch, CapBackend, CapLevel, CodecCaps, Direction, EncoderCaps, GpuAdapter,
    Guidance, GuidanceCode, MediaCapabilities, Profile, WorkerInfo, WorkerSandbox,
};
use fin_media::{AudioCodec, Chroma, RENDER_VERSION, VideoCodec};

/// The rows reported: `(codec, profile, bit depth, chroma)`, the formats spec §6.2 names.
const ROWS: &[(VideoCodec, Profile, u8, Chroma)] = &[
    (VideoCodec::Av1, Profile::Main, 8, Chroma::Yuv420),
    (VideoCodec::Av1, Profile::Main, 10, Chroma::Yuv420),
    (VideoCodec::H264, Profile::High, 8, Chroma::Yuv420),
    (VideoCodec::Hevc, Profile::Main, 8, Chroma::Yuv420),
    (VideoCodec::Hevc, Profile::Main10, 10, Chroma::Yuv420),
    (VideoCodec::Vp9, Profile::Profile0, 8, Chroma::Yuv420),
    (VideoCodec::ProRes, Profile::ProRes422, 10, Chroma::Yuv422),
];

fn level(hw: Option<bool>) -> CapLevel {
    match hw {
        Some(true) => CapLevel::Hw,
        Some(false) => CapLevel::Sw,
        None => CapLevel::None,
    }
}

/// The guidance of a gap this batch knows the answer to (Task 8's `probe/guidance.rs` holds the
/// full table).
fn guidance(codec: VideoCodec, decode: CapLevel) -> Vec<Guidance> {
    let linux_later = cfg!(target_os = "linux")
        && decode == CapLevel::None
        && matches!(codec, VideoCodec::H264 | VideoCodec::Hevc | VideoCodec::Vp9);
    if linux_later {
        return vec![Guidance {
            code: GuidanceCode::LaterBatch,
            batch: Some(Batch::B6b2),
            fix: "Linux decodes this codec from sub-project 6B-2 (VA-API)".into(),
        }];
    }
    if codec == VideoCodec::ProRes && decode == CapLevel::None && !cfg!(target_os = "macos") {
        return vec![Guidance {
            code: GuidanceCode::CodecPlugin,
            batch: None,
            fix: "the codec plugin decodes ProRes on this OS".into(),
        }];
    }
    Vec::new()
}

fn row(
    probe: &ProbeResult,
    (codec, profile, depth, chroma): (VideoCodec, Profile, u8, Chroma),
) -> CodecCaps {
    let s = StreamDesc {
        codec,
        profile: Some(profile),
        bit_depth: depth,
        chroma,
    };
    let dec = probe.decoder_candidates(&s, None);
    let enc = probe.encoder_candidates(codec, Some(profile), None);
    let backends = dec
        .iter()
        .map(|d| (d.entry(), d.hw(), Direction::Decode))
        .chain(enc.iter().map(|e| (e.entry(), e.hw(), Direction::Encode)))
        .map(|(entry, hw, direction)| CapBackend {
            name: entry.name.to_string(),
            class: entry.class,
            direction,
            hw,
        })
        .collect();
    let decode = level(dec.iter().map(|d| d.hw()).max());
    CodecCaps {
        codec,
        profile: Some(profile),
        bit_depth: Some(depth),
        decode,
        encode: level(enc.iter().map(|e| e.hw()).max()),
        backends,
        guidance: guidance(codec, decode),
    }
}

/// The report for `probe`, `gpu` (the adapter a render would use) and `sandbox`.
pub fn report(
    probe: &ProbeResult,
    gpu: Option<GpuAdapter>,
    sandbox: WorkerSandbox,
) -> MediaCapabilities {
    let codecs = ROWS.iter().map(|r| row(probe, *r)).collect();
    let encoders = probe
        .encoders()
        .iter()
        .flat_map(|e| {
            let entry = e.entry();
            entry
                .encode
                .iter()
                .filter(|c| e.encodes(**c, None))
                .map(move |codec| EncoderCaps {
                    backend: entry.name.to_string(),
                    class: entry.class,
                    codec: *codec,
                    hw: e.hw(),
                    // Not measured before Task 8's test clip: no intra-only claim.
                    intra_only: false,
                    intra_fps: None,
                    session_limit: None,
                    adapter: e.adapter().map(|a| a.id),
                })
        })
        .collect();
    MediaCapabilities {
        codecs,
        // Plan 6B-1 Decision 1: AAC is 6C's on every OS.
        audio: vec![AudioCodecCaps {
            codec: AudioCodec::Aac,
            decode: CapLevel::None,
            encode: CapLevel::None,
            backends: Vec::new(),
            guidance: vec![Guidance {
                code: GuidanceCode::LaterBatch,
                batch: Some(Batch::B6c),
                fix: "AAC arrives with the audio batch (sub-project 6C)".into(),
            }],
        }],
        encoders,
        gpu,
        worker: WorkerInfo {
            version: env!("CARGO_PKG_VERSION").to_string(),
            render_version: RENDER_VERSION,
            sandbox,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_probe_rows() {
        let r = report(&ProbeResult::software_only(), None, WorkerSandbox::None);
        assert_eq!(r.codecs.len(), ROWS.len());
        let av1: Vec<_> = r.rows(VideoCodec::Av1).collect();
        assert_eq!(av1.len(), 2);
        for row in av1 {
            assert_eq!((row.decode, row.encode), (CapLevel::Sw, CapLevel::Sw));
            let names: Vec<_> = row
                .backends
                .iter()
                .map(|b| (b.name.as_str(), b.direction))
                .collect();
            assert_eq!(
                names,
                [("dav1d", Direction::Decode), ("svt-av1", Direction::Encode)]
            );
            assert!(row.guidance.is_empty());
        }
        let h264 = r.rows(VideoCodec::H264).next().unwrap();
        assert_eq!((h264.decode, h264.encode), (CapLevel::None, CapLevel::None));
        if cfg!(target_os = "linux") {
            assert_eq!(h264.guidance[0].code, GuidanceCode::LaterBatch);
            assert_eq!(h264.guidance[0].batch, Some(Batch::B6b2));
        }
        assert_eq!(r.encoders.len(), 1);
        assert_eq!(
            (
                r.encoders[0].backend.as_str(),
                r.encoders[0].codec,
                r.encoders[0].hw
            ),
            ("svt-av1", VideoCodec::Av1, false)
        );
        assert_eq!(r.audio[0].guidance[0].batch, Some(Batch::B6c));
        assert_eq!(r.worker.render_version, RENDER_VERSION);
    }

    #[test]
    fn the_event_holds_no_name_or_path_of_the_report() {
        use crate::media::diag::capabilities_event;
        let gpu = GpuAdapter {
            backend: "vulkan".into(),
            device_type: "Cpu".into(),
            name: "llvmpipe (/home/someone/.local/lib/libvulkan_lvp.so)".into(),
            vendor: 0x10005,
            device: 0,
            driver_major: Some(25),
            software: true,
        };
        let mut r = report(
            &ProbeResult::software_only(),
            Some(gpu),
            WorkerSandbox::Landlock,
        );
        // A future backend reporting its friendly name, and guidance text with a path in it.
        r.encoders[0].backend = "Intel® Quick Sync Video H.264 Encoder MFT".into();
        r.codecs[0].guidance.push(Guidance {
            code: GuidanceCode::GpuDriver,
            batch: None,
            fix: "install C:\\Drivers\\gpu.inf".into(),
        });
        let text = serde_json::to_string(&capabilities_event(&r)).unwrap();
        for banned in [
            "someone",
            "llvmpipe",
            "Intel",
            "MFT",
            "Drivers",
            "install",
            env!("CARGO_PKG_VERSION"),
        ] {
            assert!(!text.contains(banned), "{banned}: {text}");
        }
        assert!(text.contains(r#""sandbox":"landlock""#), "{text}");
        assert!(text.contains(r#""code":"gpuDriver""#), "{text}");
    }
}
