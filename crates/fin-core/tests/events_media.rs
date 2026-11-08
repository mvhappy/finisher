//! The media diagnostics kinds (6A plan Task 14): the optional `media` object of `job.done`
//! (`kv` 2), `media.capabilities`, `render.stats`, the reserved `preview.stats` and
//! `render.sample`, and the `MEDIA_CODECS` / `MEDIA_BACKENDS` tables. The emitters are the
//! `media` worker family (`crates/fin-workers/tests/media_diag.rs`).

#[path = "events_samples/mod.rs"]
mod samples;

use std::collections::BTreeSet;

use fin_core::events::names::TABLES;
use fin_core::events::names_media::{BackendClass, MEDIA_BACKENDS, MEDIA_CODECS, backend_class};
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
    for name in ["media.capabilities", "render.stats"] {
        let k = kind(name, 1).unwrap_or_else(|| panic!("{name} v1"));
        assert!(!k.reserved && !k.training_only, "{name}");
        // No new `Source` variant: workers' diagnostics go through `finisherd` (spec §15).
        assert_eq!(k.sources, [Source::Finisherd], "{name}");
        assert!(kind(name, 0).is_none() && kind(name, 2).is_none(), "{name}");
    }
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
            ..samples::media_capabilities()
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
        let word = s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        assert!(word, "{s:?} is not a plain name");
    }
}

#[test]
fn media_tables_are_registered_and_unknown_values_read_as_other() {
    for t in [&MEDIA_CODECS, &MEDIA_BACKENDS] {
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
