//! The `media` family's diagnostics (6A plan Task 14): the `media` object of `job.done`
//! (`LookStats.media`), `media.capabilities` and `render.stats`, driven over the envelope as a
//! host drives the worker. With `start.diagnostics` off the family sends nothing and builds
//! nothing; with it on every payload decodes as its registered kind and holds no path and no
//! adapter or driver name.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use fin_core::events::{decode_payload, kind};
use fin_jobs::HostMsg;
use fin_media::jobs::{LookResult, MediaCapabilities};
use media_support::{Fixture, gpu_or_skip, r, worker_env};
use serde_json::json;
use support::{Job, Proc, Run, no_calls, spawn_with};

fn warm_worker() -> Proc {
    let mut p = spawn_with(&["media", "--warm"], &worker_env());
    p.hello();
    p
}

fn look(p: &mut Proc, job: &Job, params: &fin_media::jobs::LookParams, diagnostics: bool) -> Run {
    let mut start = job.start(
        "media.look",
        serde_json::to_value(params).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    start.diagnostics = diagnostics;
    p.send(&HostMsg::Start(start));
    p.drive(no_calls)
}

fn caps(p: &mut Proc, job: &Job, diagnostics: bool) -> Run {
    let mut start = job.start("media.capabilities", json!({}), &[]);
    start.diagnostics = diagnostics;
    p.send(&HostMsg::Start(start));
    p.drive(no_calls)
}

fn result(run: &Run) -> LookResult {
    serde_json::from_value(run.result().clone()).unwrap()
}

/// Every diag payload decodes as its kind (the journal's own decode).
fn decodes(run: &Run) {
    for (k, v) in run.events.iter().filter_map(|e| match e {
        support::Ev::Diag(k, v) => Some((k, v)),
        _ => None,
    }) {
        let spec = kind(k, 1).unwrap_or_else(|| panic!("{k} is not a v1 kind"));
        decode_payload(spec, v).unwrap_or_else(|e| panic!("{k}: {e}: {v}"));
    }
}

#[test]
fn diagnostics_off_sends_nothing_and_builds_no_media_object() {
    if !gpu_or_skip("diagnostics_off_sends_nothing_and_builds_no_media_object") {
        return;
    }
    let fx = Fixture::new();
    let mut p = warm_worker();
    let run = look(&mut p, &fx.job(None), &fx.params(&[r("0"), r("1")]), false);
    assert_eq!(result(&run).frames.len(), 2);
    assert!(result(&run).stats.media.is_none());
    assert!(
        !run.result()["stats"]
            .as_object()
            .unwrap()
            .contains_key("media")
    );
    let run = caps(&mut p, &Job::new(), false);
    assert!(run.result()["gpu"].is_object());
    let all: Vec<_> = run
        .events
        .iter()
        .filter(|e| matches!(e, support::Ev::Diag(..)))
        .collect();
    assert!(all.is_empty(), "{all:?}");
    // A second look after the first two: still nothing, and nothing left over to flush.
    let run = look(&mut p, &fx.job(None), &fx.params(&[r("1")]), false);
    assert!(
        !run.events
            .iter()
            .any(|e| matches!(e, support::Ev::Diag(..)))
    );
}

#[test]
fn a_look_reports_its_media_object_and_stage_timings() {
    if !gpu_or_skip("a_look_reports_its_media_object_and_stage_timings") {
        return;
    }
    let fx = Fixture::new();
    let mut p = warm_worker();
    let params = fx.params(&[r("0"), r("1/3")]);
    let run = look(&mut p, &fx.job(None), &params, true);
    decodes(&run);
    let res = result(&run);
    let media = res.stats.media.expect("a media object with diagnostics on");
    assert_eq!(media.codec.as_str(), "av1");
    assert_eq!(media.backend.as_str(), "dav1d");
    assert!(!media.hw);
    assert_eq!(media.frames, 2);
    assert!(!media.cache_hit, "the first look opened its source");
    assert_eq!(media.copy_path, fin_core::events::payloads::CopyPath::Copy);
    assert!(media.rt_factor_milli > 0, "{media:?}");
    // 320×180 fits the smallest bucket.
    assert_eq!(
        media.resolution,
        fin_core::events::payloads::ResolutionBucket::Sd
    );
    let stats = run.diags("render.stats");
    let phases: Vec<(&str, &str, u64)> = stats
        .iter()
        .map(|s| {
            (
                s["phase"].as_str().unwrap(),
                s["backend"].as_str().unwrap(),
                s["histogram"]["count"].as_u64().unwrap(),
            )
        })
        .collect();
    let gpu = res.adapter.backend.as_str();
    assert!(phases.contains(&("decode", "dav1d", 2)), "{phases:?}");
    assert!(phases.contains(&("composite", gpu, 2)), "{phases:?}");
    assert!(phases.contains(&("readback", gpu, 2)), "{phases:?}");
    for s in &stats {
        assert_eq!(s["histogram"]["windowMs"], 60_000);
        assert_eq!(s["histogram"]["start"].as_u64().unwrap() % 60_000, 0);
    }
    // The same source again on the same warm worker: a cache hit, and the stats are only
    // what that job measured.
    let again = look(&mut p, &fx.job(None), &params, true);
    decodes(&again);
    let media = result(&again).stats.media.unwrap();
    assert!(media.cache_hit, "{media:?}");
    let total: u64 = again
        .diags("render.stats")
        .iter()
        .filter(|s| s["phase"] == "readback")
        .map(|s| s["histogram"]["count"].as_u64().unwrap())
        .sum();
    assert_eq!(total, 2);
}

#[test]
fn a_look_that_decodes_nothing_says_none() {
    if !gpu_or_skip("a_look_that_decodes_nothing_says_none") {
        return;
    }
    let fx = Fixture::new();
    let mut p = warm_worker();
    // 5/2 s is on the H.264 clip, which this build cannot decode: skipped, nothing decoded.
    let run = look(&mut p, &fx.job(None), &fx.params(&[r("5/2")]), true);
    let res = result(&run);
    assert_eq!(res.skipped.len(), 1);
    let media = res.stats.media.unwrap();
    assert_eq!(
        (media.codec.as_str(), media.backend.as_str()),
        ("none", "none")
    );
    assert!(!media.hw && !media.cache_hit);
    assert!(
        run.diags("render.stats")
            .iter()
            .all(|s| s["phase"] != "decode")
    );
}

#[test]
fn capabilities_name_the_gpu_by_numbers_only() {
    if !gpu_or_skip("capabilities_name_the_gpu_by_numbers_only") {
        return;
    }
    let mut p = warm_worker();
    let run = caps(&mut p, &Job::new(), true);
    decodes(&run);
    let answer: MediaCapabilities = serde_json::from_value(run.result().clone()).unwrap();
    let sent = run.diags("media.capabilities");
    assert_eq!(sent.len(), 1);
    let event = sent[0];
    let gpu = answer.gpu.expect("an adapter");
    assert_eq!(event["gpu"]["vendorId"], gpu.vendor);
    assert_eq!(event["gpu"]["deviceId"], gpu.device);
    assert_eq!(event["gpu"]["software"], gpu.software);
    assert_eq!(event["gpu"]["backend"], gpu.backend.as_str());
    assert_eq!(event["copyPath"], "copy");
    assert_eq!(event["sandbox"], "none");
    assert_eq!(event["renderVersion"], answer.worker.render_version);
    let av1 = event["codecs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["codec"] == "av1")
        .unwrap();
    assert_eq!(
        (&av1["decode"], &av1["encode"]),
        (&json!("sw"), &json!("sw"))
    );
    assert_eq!(
        av1["backends"],
        json!([{ "name": "dav1d", "class": "software" }, { "name": "svt-av1", "class": "software" }])
    );
    let h264 = event["codecs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["codec"] == "h264")
        .unwrap();
    assert_eq!(h264["decode"], "none");
    // Neither the adapter's name nor a path is in anything the family sent.
    let text = event.to_string();
    assert!(!text.contains(&gpu.name), "{text}");
    assert!(!text.contains('/') && !text.contains('\\'), "{text}");
}

#[test]
fn nothing_sent_holds_a_path_a_project_name_or_an_adapter_name() {
    if !gpu_or_skip("nothing_sent_holds_a_path_a_project_name_or_an_adapter_name") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let mut p = warm_worker();
    let run = look(&mut p, &job, &fx.params(&[r("0"), r("1")]), true);
    let res = result(&run);
    let mut sent = String::new();
    for e in &run.events {
        if let support::Ev::Diag(_, v) = e {
            sent.push_str(&v.to_string());
        }
    }
    sent.push_str(&serde_json::to_string(&res.stats.media).unwrap());
    assert!(!sent.is_empty());
    for banned in [
        res.adapter.name.as_str(),
        fx.dir.path().to_str().unwrap(),
        job.out().to_str().unwrap(),
        "look-0",
    ] {
        assert!(!sent.contains(banned), "{banned:?} in {sent}");
    }
}
