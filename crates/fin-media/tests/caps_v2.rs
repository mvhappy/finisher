//! `media.capabilities` v2 types (plan 6B-1 Task 2): wire names and round trips.

use fin_media::backend::VendorClass;
use fin_media::jobs::{
    AdapterId, AudioCodecCaps, Batch, CapBackend, CapLevel, CodecCaps, Direction, EncoderCaps,
    Guidance, GuidanceCode, MediaCapabilities, Profile, WorkerInfo, WorkerSandbox,
};
use fin_media::{AudioCodec, VideoCodec};
use serde_json::json;

fn report() -> MediaCapabilities {
    MediaCapabilities {
        codecs: vec![
            CodecCaps {
                codec: VideoCodec::Hevc,
                profile: Some(Profile::Main10),
                bit_depth: Some(10),
                decode: CapLevel::Hw,
                encode: CapLevel::None,
                backends: vec![CapBackend {
                    name: "mf-hw".into(),
                    class: VendorClass::Gpu,
                    direction: Direction::Decode,
                    hw: true,
                }],
                guidance: vec![Guidance {
                    code: GuidanceCode::CodecPlugin,
                    batch: None,
                    fix: "install the codec plugin".into(),
                }],
            },
            CodecCaps {
                codec: VideoCodec::H264,
                profile: None,
                bit_depth: None,
                decode: CapLevel::None,
                encode: CapLevel::None,
                backends: vec![],
                guidance: vec![Guidance {
                    code: GuidanceCode::LaterBatch,
                    batch: Some(Batch::B6b2),
                    fix: "arrives with VA-API".into(),
                }],
            },
        ],
        audio: vec![AudioCodecCaps {
            codec: AudioCodec::Aac,
            decode: CapLevel::None,
            encode: CapLevel::None,
            backends: vec![],
            guidance: vec![Guidance {
                code: GuidanceCode::LaterBatch,
                batch: Some(Batch::B6c),
                fix: "arrives with the audio batch".into(),
            }],
        }],
        encoders: vec![EncoderCaps {
            backend: "mf-hw".into(),
            class: VendorClass::Gpu,
            codec: VideoCodec::H264,
            hw: true,
            intra_only: true,
            intra_fps: Some(480),
            session_limit: Some(2),
            adapter: Some(AdapterId {
                vendor_id: 0x1002,
                device_id: 0x7550,
                driver_major: Some(32),
            }),
        }],
        gpu: None,
        worker: WorkerInfo {
            version: "0.0.0".into(),
            render_version: 1,
            sandbox: WorkerSandbox::Landlock,
        },
    }
}

#[test]
fn v2_round_trips_with_camel_case_wire_names() {
    let r = report();
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["codecs"][0]["profile"], "main10");
    assert_eq!(v["codecs"][0]["bitDepth"], 10);
    assert_eq!(v["codecs"][0]["decode"], "hw");
    assert_eq!(v["codecs"][0]["backends"][0]["class"], "gpu");
    assert_eq!(
        v["codecs"][1]["guidance"][0],
        json!({"code": "laterBatch", "batch": "6B-2", "fix": "arrives with VA-API"})
    );
    assert!(v["codecs"][1].get("profile").is_none());
    assert_eq!(v["encoders"][0]["sessionLimit"], 2);
    assert_eq!(
        v["encoders"][0]["adapter"],
        json!({"vendorId": 4098, "deviceId": 30032, "driverMajor": 32})
    );
    assert_eq!(v["worker"]["sandbox"], "landlock");
    assert_eq!(v["audio"][0]["codec"], "aac");
    assert_eq!(v["audio"][0]["guidance"][0]["batch"], "6C");
    assert_eq!(v["encoders"][0]["hw"], true);
    assert_eq!(
        serde_json::to_value(Profile::Main422_10).unwrap(),
        "main422_10"
    );
    let back: MediaCapabilities = serde_json::from_value(v).unwrap();
    assert_eq!(back, r);
    assert_eq!(back.rows(VideoCodec::Hevc).count(), 1);
}

#[test]
fn empty_guidance_is_left_out_and_reads_back() {
    let mut r = report();
    r.codecs[0].guidance.clear();
    let v = serde_json::to_value(&r).unwrap();
    assert!(v["codecs"][0].get("guidance").is_none());
    assert_eq!(serde_json::from_value::<MediaCapabilities>(v).unwrap(), r);
}

#[test]
fn the_schema_names_the_v2_types() {
    let s = serde_json::to_value(schemars::schema_for!(MediaCapabilities)).unwrap();
    let defs = s["$defs"].as_object().unwrap();
    for name in [
        "CodecCaps",
        "EncoderCaps",
        "Guidance",
        "GuidanceCode",
        "CapBackend",
        "WorkerInfo",
        "AdapterId",
        "Batch",
    ] {
        assert!(defs.contains_key(name), "{name}");
    }
}

#[test]
fn profiles_from_stream_numbers() {
    use fin_media::Chroma::{Yuv420, Yuv422};
    let of = |c, f: &[u8; 4], p, d, ch| Profile::of(c, *f, p, d, ch);
    assert_eq!(
        of(VideoCodec::H264, b"avc1", Some(100), 8, Yuv420),
        Some(Profile::High)
    );
    assert_eq!(
        of(VideoCodec::H264, b"avc1", Some(122), 10, Yuv422),
        Some(Profile::High422)
    );
    assert_eq!(
        of(VideoCodec::H264, b"avc1", Some(244), 8, Yuv420),
        Some(Profile::Other)
    );
    assert_eq!(
        of(VideoCodec::Hevc, b"hvc1", Some(2), 10, Yuv420),
        Some(Profile::Main10)
    );
    assert_eq!(
        of(VideoCodec::Hevc, b"hvc1", Some(4), 10, Yuv422),
        Some(Profile::Main422_10)
    );
    assert_eq!(
        of(VideoCodec::Hevc, b"hvc1", Some(4), 12, Yuv422),
        Some(Profile::Other)
    );
    assert_eq!(
        of(VideoCodec::Av1, b"av01", Some(0), 10, Yuv420),
        Some(Profile::Main)
    );
    assert_eq!(
        of(VideoCodec::Vp9, b"vp09", Some(2), 10, Yuv420),
        Some(Profile::Profile2)
    );
    assert_eq!(
        of(VideoCodec::ProRes, b"ap4x", None, 12, Yuv422),
        Some(Profile::ProRes4444)
    );
    assert_eq!(
        of(VideoCodec::ProRes, b"apcn", None, 10, Yuv422),
        Some(Profile::ProRes422)
    );
    assert_eq!(of(VideoCodec::H264, b"avc1", None, 8, Yuv420), None);
}
