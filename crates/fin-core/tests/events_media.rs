//! The media diagnostics kinds (6A plan Task 14, 6B-1 plan Task 19): the optional `media` object
//! of `job.done` (`kv` 2), `media.capabilities` (`kv` 2), `render.stats`, the reserved
//! `preview.stats` and `render.sample`, and the `MEDIA_CODECS` / `MEDIA_BACKENDS` /
//! `MEDIA_PROFILES` / `MEDIA_AUDIO_CODECS` tables. The emitters are the
//! `media` worker family (`crates/fin-workers/tests/media_diag.rs`).

#[path = "events_samples/mod.rs"]
mod samples;

use std::collections::BTreeSet;

use fin_core::events::names::TABLES;
use fin_core::events::names_jobs;
use fin_core::events::names_media::{
    BackendClass, MEDIA_AUDIO_CODECS, MEDIA_BACKENDS, MEDIA_CODECS, MEDIA_PROFILES, backend_class,
};
use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

fn walk<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            out.push(k);
            walk(x, out)
        }),
        _ => {}
    }
}

#[test]
fn media_kinds_are_registered() {
    // `media.capabilities` is `kv` 2 (6B-1 Decision 19) with no upcaster, as `job.done` 1 went.
    for (name, v) in [("media.capabilities", 2), ("render.stats", 1)] {
        let k = kind(name, v).unwrap_or_else(|| panic!("{name} v{v}"));
        assert!(!k.reserved && !k.training_only, "{name}");
        // No new `Source` variant: workers' diagnostics go through `finisherd` (spec §15).
        assert_eq!(k.sources, [Source::Finisherd], "{name}");
        assert!(
            kind(name, 0).is_none() && kind(name, v + 1).is_none(),
            "{name}"
        );
    }
    assert!(kind("media.capabilities", 1).is_none());
    let preview = kind("preview.stats", 0).expect("preview.stats v0");
    assert!(preview.reserved && !preview.training_only);
    let sample = kind("render.sample", 0).expect("render.sample v0");
    assert!(sample.reserved && sample.training_only);
    // `job.done` moved to `kv` 2 with no upcaster.
    assert!(kind("job.done", 1).is_none());
    assert!(kind("job.done", 2).is_some());
}

#[test]
fn media_payloads_round_trip_and_strip_content_is_a_no_op() {
    let ctx = ScrubCtx::default();
    let key = |d: &[u8]| samples::key(d);
    let all: Vec<AnyPayload> = vec![
        samples::job_done_failed().into(),
        JobDone {
            media: Some(samples::job_media()),
            ..samples::job_done_failed()
        }
        .into(),
        samples::media_capabilities().into(),
        samples::render_stats().into(),
        MediaCapabilitiesEvent {
            gpu: None,
            audio: Vec::new(),
            encoders: Vec::new(),
            ..samples::media_capabilities()
        }
        .into(),
        RenderStats {
            phase: RenderPhase::Encode,
            backend: MEDIA_BACKENDS.get("svt-av1").unwrap(),
            ..samples::render_stats()
        }
        .into(),
    ];
    for p in all {
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(
            decode_payload(p.kind(), &v).unwrap(),
            p,
            "{}",
            p.kind().name
        );
        assert_eq!(validate_payload(p.kind(), &v, &key, &ctx).unwrap(), p);
        // No content field: stripping changes nothing, and redaction keeps the payload.
        let mut stripped = p.clone();
        stripped.strip_content();
        assert_eq!(stripped, p, "{}", p.kind().name);
        let mut redacted = v.clone();
        redact(p.kind(), &mut redacted, Purposes::DIAGNOSTICS);
        assert_eq!(redacted, v, "{}", p.kind().name);
        assert!(p.approx_bytes() > 0);
    }
}

#[test]
fn job_done_media_is_omitted_when_absent_and_all_meta_when_present() {
    let plain = serde_json::to_value(samples::job_done_failed()).unwrap();
    assert!(plain.get("media").is_none(), "{plain}");
    let with = serde_json::to_value(JobDone {
        media: Some(samples::job_media()),
        ..samples::job_done_failed()
    })
    .unwrap();
    assert_eq!(
        with["media"],
        json!({
            "codec": "av1", "backend": "dav1d", "hw": false, "resolution": "hd1080",
            "frames": 3, "rtFactorMilli": 4200, "cacheHit": true, "copyPath": "copy",
        })
    );
}

#[test]
fn media_schemas_have_no_free_strings() {
    for name in ["job.done", "media.capabilities", "render.stats"] {
        let k = kinds().iter().find(|k| k.name == name).unwrap();
        let schema = (k.schema)();
        let text = schema.to_string();
        // Adapter and driver names, and anything path-like, have no field at all.
        for banned in [
            "adapterName",
            "driverName",
            "driverInfo",
            "path",
            "file",
            "project",
        ] {
            let hit = text.contains(&format!("\"{banned}\":"));
            assert!(!hit, "{name} has a {banned} property: {schema}");
        }
    }
    // Every string in a sample payload is a table name, an enum value or a key.
    let mut all = Vec::new();
    let caps = serde_json::to_value(samples::media_capabilities()).unwrap();
    let stats = serde_json::to_value(samples::render_stats()).unwrap();
    walk(&caps, &mut all);
    walk(&stats, &mut all);
    for s in all {
        let word = s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        assert!(word, "{s:?} is not a plain name");
    }
}

#[test]
fn media_capabilities_v2_wire_shape() {
    let v = serde_json::to_value(samples::media_capabilities()).unwrap();
    assert_eq!(
        v["codecs"][0],
        json!({
            "codec": "av1", "profile": "main", "bitDepth": 10, "decode": "sw", "encode": "sw",
            "backends": [
                { "name": "dav1d", "class": "software", "direction": "decode", "hw": false },
                { "name": "svt-av1", "class": "software", "direction": "encode", "hw": false },
            ],
        })
    );
    assert_eq!(
        v["codecs"][1]["guidance"],
        json!([{ "code": "laterBatch", "batch": "6B-2" }, { "code": "codecPlugin" }])
    );
    assert_eq!(
        v["audio"][0],
        json!({ "codec": "aac", "decode": "none", "encode": "none", "backends": [],
                "guidance": [{ "code": "laterBatch", "batch": "6C" }] })
    );
    assert_eq!(
        v["encoders"][0],
        json!({
            "backend": "mf-hw", "class": "gpu", "codec": "h264", "hw": true, "intraOnly": true,
            "intraFps": 480, "sessionLimit": 2,
            "adapter": { "vendorId": 4318, "deviceId": 9860, "driverMajor": 560 },
        })
    );
    assert_eq!(v["sandbox"], "landlock");
    // Guidance is a code, never its text: the `fix` stays in the capability answer.
    assert!(!v.to_string().contains("\"fix\""), "{v}");
}

#[test]
fn media_capabilities_names_outside_the_tables_read_as_other() {
    // A report whose strings are an adapter's name, a path and an MFT's friendly name.
    let hostile = json!({
        "codecs": [{
            "codec": "/home/someone/clip.mp4", "profile": "C:\\Users\\x\\High", "decode": "hw",
            "encode": "none",
            "backends": [{ "name": "NVIDIA H.264 Encoder MFT", "class": "gpu", "direction": "decode", "hw": true }],
        }],
        "audio": [{ "codec": "/tmp/aac", "decode": "none", "encode": "none", "backends": [] }],
        "encoders": [{
            "backend": "Intel® Quick Sync Video H.264 Encoder MFT", "class": "gpu",
            "codec": "h264", "hw": true, "intraOnly": false,
        }],
        "copyPath": "copy", "sandbox": "none", "renderVersion": 2,
    });
    let spec = kind("media.capabilities", 2).unwrap();
    let p = decode_payload(spec, &hostile).unwrap();
    let text = serde_json::to_string(&p).unwrap();
    for banned in ["someone", "Users", "NVIDIA", "Intel", "MFT", "tmp"] {
        assert!(
            !text.contains(banned),
            "{banned} reached the payload: {text}"
        );
    }
    let AnyPayload::MediaCapabilities(e) = p else {
        panic!("not a capabilities payload")
    };
    assert_eq!(e.codecs[0].codec, Name::OTHER);
    assert_eq!(e.codecs[0].profile, Some(Name::OTHER));
    assert_eq!(e.codecs[0].backends[0].name, Name::OTHER);
    assert_eq!(e.audio[0].codec, Name::OTHER);
    assert_eq!(e.encoders[0].backend, Name::OTHER);
    // An unknown guidance code or batch is not a payload at all (closed enums).
    let mut bad = hostile.clone();
    bad["codecs"][0]["guidance"] = json!([{ "code": "install /usr/lib/x.so" }]);
    assert!(decode_payload(spec, &bad).is_err());
}

#[test]
fn media_tables_are_registered_and_unknown_values_read_as_other() {
    for t in [
        &MEDIA_CODECS,
        &MEDIA_BACKENDS,
        &MEDIA_PROFILES,
        &MEDIA_AUDIO_CODECS,
    ] {
        assert!(TABLES.iter().any(|x| std::ptr::eq(*x, t)), "{}", t.table);
        let unique: BTreeSet<&str> = t.entries().iter().copied().collect();
        assert_eq!(unique.len(), t.entries().len(), "{} repeats", t.table);
    }
    let media: JobMedia = serde_json::from_value(json!({
        "codec": "/home/someone/clip.mp4", "backend": "NVIDIA GeForce RTX 5090", "hw": true,
        "resolution": "uhd4k", "frames": 1, "rtFactorMilli": 0, "cacheHit": false,
        "copyPath": "zeroCopy",
    }))
    .unwrap();
    assert_eq!((media.codec, media.backend), (Name::OTHER, Name::OTHER));
    let gpu: GpuEntry = serde_json::from_value(json!({
        "backend": "Quantum Adapter", "adapterType": "discrete", "vendorId": 4318,
        "deviceId": 9999, "software": false,
    }))
    .unwrap();
    assert_eq!(gpu.backend, Name::OTHER);
    assert_eq!(gpu.driver_major, None);
}

#[test]
fn backend_classes() {
    assert_eq!(backend_class("dav1d"), BackendClass::Software);
    assert_eq!(backend_class("svt-av1"), BackendClass::Software);
    assert_eq!(backend_class("videotoolbox"), BackendClass::Os);
    assert_eq!(backend_class("vaapi"), BackendClass::Gpu);
    assert_eq!(backend_class("ffmpeg"), BackendClass::Plugin);
    assert_eq!(backend_class("vulkan"), BackendClass::Other);
    assert_eq!(backend_class("anything else"), BackendClass::Other);
    // Every codec backend in the table has a class; only the GPU APIs and `none` are `other`.
    for b in MEDIA_BACKENDS.entries() {
        let gpu_api = matches!(
            *b,
            "vulkan" | "metal" | "dx12" | "gl" | "browserwebgpu" | "none"
        );
        assert_eq!(backend_class(b) == BackendClass::Other, gpu_api, "{b}");
    }
}

#[test]
fn resolution_buckets() {
    use ResolutionBucket::*;
    for ((w, h), want) in [
        ((0, 0), Sd),
        ((640, 360), Sd),
        ((960, 540), Sd),
        ((961, 540), Hd720),
        ((1280, 720), Hd720),
        ((1920, 1080), Hd1080),
        ((1080, 1920), Hd1080),
        ((2048, 1080), Hd1080),
        ((2560, 1440), Qhd),
        ((3840, 2160), Uhd4k),
        ((4096, 2160), Uhd4k),
        ((7680, 4320), Uhd8k),
        ((u32::MAX, u32::MAX), Uhd8k),
    ] {
        assert_eq!(ResolutionBucket::of(w, h), want, "{w}x{h}");
    }
}

#[test]
fn adapter_types_read_wgpus_names() {
    for (name, want) in [
        ("Cpu", AdapterType::Cpu),
        ("DiscreteGpu", AdapterType::Discrete),
        ("IntegratedGpu", AdapterType::Integrated),
        ("VirtualGpu", AdapterType::Virtual),
        ("Other", AdapterType::Other),
        ("Something Else", AdapterType::Other),
    ] {
        assert_eq!(AdapterType::from_wgpu(name), want, "{name}");
    }
}

#[test]
fn audio_codecs_and_phases_are_in_the_closed_schema() {
    for c in ["aac", "opus", "mp3", "flac", "alac", "vorbis", "pcm"] {
        assert!(MEDIA_CODECS.get(c).is_some(), "MEDIA_CODECS lacks {c}");
    }
    for (phase, name) in [
        (RenderPhase::DecodeAudio, "decodeAudio"),
        (RenderPhase::Mix, "mix"),
        (RenderPhase::EncodeAudio, "encodeAudio"),
    ] {
        assert_eq!(serde_json::to_value(phase).unwrap(), json!(name));
        // One `render.stats` entry per phase round-trips through the registered kind.
        let stats = RenderStats {
            phase,
            backend: MEDIA_BACKENDS.get("symphonia").unwrap(),
            histogram: samples::hist(),
        };
        let any: AnyPayload = stats.into();
        let v = serde_json::to_value(&any).unwrap();
        assert_eq!(v["phase"], json!(name));
        assert_eq!(decode_payload(any.kind(), &v).unwrap(), any, "{name}");
        let schema = (kind("render.stats", 1).unwrap().schema)().to_string();
        assert!(schema.contains(name), "render.stats schema lacks {name}");
    }
}

#[test]
fn job_done_with_an_audio_media_object_round_trips() {
    let done = JobDone {
        kind: names_jobs::JOB_KINDS.get("media.listen").unwrap(),
        family: names_jobs::WORKER_FAMILIES.get("media").unwrap(),
        outcome: JobOutcome::Succeeded,
        code: None,
        media: Some(JobMedia {
            codec: MEDIA_CODECS.get("opus").unwrap(),
            backend: MEDIA_BACKENDS.get("libopus").unwrap(),
            hw: false,
            resolution: ResolutionBucket::Sd,
            frames: 469,
            rt_factor_milli: 250_000,
            cache_hit: true,
            copy_path: CopyPath::Copy,
        }),
        ..samples::job_done_failed()
    };
    let any: AnyPayload = done.into();
    let v = serde_json::to_value(&any).unwrap();
    assert_eq!(v["media"]["codec"], json!("opus"));
    assert_eq!(v["media"]["backend"], json!("libopus"));
    assert_eq!(decode_payload(any.kind(), &v).unwrap(), any);
    // A codec outside the table still reads as `other`.
    assert_eq!(MEDIA_CODECS.get_or_other("wavpack"), Name::OTHER);
}
