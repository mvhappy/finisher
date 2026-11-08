//! The `media` family and `media.look` (plan 6A Task 11), driven over the envelope as a host
//! drives it (`fin-workers media --warm`), and without one (`--replay`). GPU tests pick the
//! adapter with `FIN_GPU` and skip without one unless `FIN_REQUIRE_GPU=1`.
// Tests build fixtures and read outputs by path (`scripts/clippy-allow.json`).
#![allow(clippy::disallowed_methods)]

mod media_support;
mod support;

use std::path::Path;
use std::time::Instant;

use fin_compositor::compare;
use fin_core::events::names_jobs::{JOB_KINDS, WORKER_FAMILIES};
use fin_jobs::{HostMsg, Role};
use fin_media::jobs::{LookFrame, LookResult};
use fin_media::testing::{Rect, read_id_rgba};
use media_support::{Fixture, OUT, gpu_or_skip, r, read_png, worker_env, write_params};
use serde_json::Value;
use support::{Ev, Job, Proc, Run, no_calls, spawn_with};

fn warm_worker() -> Proc {
    let mut p = spawn_with(&["media", "--warm"], &worker_env());
    let h = p.hello();
    assert!(h.warm);
    p
}

fn look(p: &mut Proc, job: &Job, params: &fin_media::jobs::LookParams) -> Run {
    let start = job.start(
        "media.look",
        serde_json::to_value(params).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    p.send(&HostMsg::Start(start));
    p.drive(no_calls)
}

fn result(run: &Run) -> LookResult {
    serde_json::from_value(run.result().clone()).unwrap()
}

#[test]
fn hello_lists_the_media_kinds() {
    let mut p = spawn_with(&["media"], &[]);
    let h = p.hello();
    assert_eq!(
        (h.role, h.family.as_str(), h.warm),
        (Role::Worker, "media", false)
    );
    let kinds: Vec<&str> = h.kinds.iter().map(|k| k.kind.as_ref()).collect();
    assert_eq!(kinds, ["media.look", "media.capabilities"]);
    assert!(WORKER_FAMILIES.get("media").is_some());
    for k in &h.kinds {
        assert!(
            JOB_KINDS.get(&k.kind).is_some(),
            "{} is not in fin-core's kinds",
            k.kind
        );
    }
    // A trailing `--warm` (the engine appends it) makes the family warm.
    warm_worker();
}

/// Five times including a midpoint tie (7/60 s lies halfway between frames 3 and 4) and one
/// tick either side of it.
fn times() -> Vec<fin_core::Rational> {
    vec![r("0"), r("7/60"), r("699/6000"), r("701/6000"), r("1")]
}

#[test]
fn frames_match_the_reference_and_the_nearest_frame() {
    if !gpu_or_skip("frames_match_the_reference_and_the_nearest_frame") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let params = fx.params(&times());
    let mut p = warm_worker();
    let run = look(&mut p, &job, &params);
    let res = result(&run);
    assert_eq!(run.items(), ["0", "1", "2", "3", "4"]);
    let begins = run
        .events
        .iter()
        .filter(|e| matches!(e, Ev::Begin(_)))
        .count();
    assert_eq!(begins, 5);
    assert!(res.skipped.is_empty(), "{:?}", res.skipped);
    assert_eq!(res.frames.len(), 5);
    let expected: Vec<u32> = times().iter().map(|&t| fx.expected_frame(t)).collect();
    assert_eq!(
        expected,
        [0, 3, 3, 4, 30],
        "frame_at: ties go to the earlier frame"
    );
    for (i, (frame, &t)) in res.frames.iter().zip(times().iter()).enumerate() {
        assert_eq!(frame.at, t);
        assert_eq!(frame.file, format!("look-{i}.png"));
        assert_eq!((frame.width, frame.height), (OUT.width, OUT.height));
        assert_eq!(frame.plan, params.plan_samples[i].blake3);
        let (size, rgba) = read_png(&job.out().join(&frame.file));
        assert_eq!(size, OUT);
        let whole = Rect {
            x: 0.0,
            y: 0.0,
            width: OUT.width as f64,
            height: OUT.height as f64,
        };
        assert_eq!(
            read_id_rgba(&rgba, OUT.width, OUT.height, whole),
            Some(expected[i]),
            "frame id at {t}"
        );
        let t1 = compare::t1(&rgba, &fx.reference(t));
        assert!(compare::passes(&t1), "T1 at {t}: {t1:?}");
    }
    // Each item's value is its frame.
    let values: Vec<LookFrame> = run
        .events
        .iter()
        .filter_map(|e| match e {
            Ev::Item(_, Some(v)) => Some(serde_json::from_value(v.clone()).unwrap()),
            _ => None,
        })
        .collect();
    assert_eq!(values, res.frames);
    assert!(!res.adapter.backend.is_empty());
    assert_eq!(res.stats.worker_jobs, 1);
    assert!(!res.stats.device_reused);
}

#[test]
fn a_wrong_or_missing_plan_sample_is_plan_mismatch() {
    if !gpu_or_skip("a_wrong_or_missing_plan_sample_is_plan_mismatch") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let mut p = warm_worker();
    let mut params = fx.params(&[r("1/2"), r("1")]);
    params.plan_samples[1].blake3 = "0".repeat(64);
    let run = look(&mut p, &job, &params);
    let e = run.error();
    assert_eq!(e.code, "PLAN_MISMATCH");
    assert_eq!(e.details["at"], serde_json::to_value(r("1")).unwrap());
    assert_eq!(e.details["reason"], "hash");
    // The first frame was done before the second's plan differed.
    assert_eq!(run.items(), ["0"]);
    params.plan_samples.pop();
    let run = look(&mut p, &job, &params);
    assert_eq!(run.error().code, "PLAN_MISMATCH");
    assert_eq!(run.error().details["reason"], "noSample");
}

#[test]
fn plan_samples_may_arrive_in_the_starts_extra() {
    if !gpu_or_skip("plan_samples_may_arrive_in_the_starts_extra") {
        return;
    }
    // A kind's `prepare` computes the samples and hands them over as `extra.planSamples`; the
    // params the submitter wrote carry none.
    let fx = Fixture::new();
    let job = fx.job(None);
    let mut p = warm_worker();
    let mut params = fx.params(&[r("1"), r("3/2")]);
    let samples = std::mem::take(&mut params.plan_samples);
    let mut start = job.start(
        "media.look",
        serde_json::to_value(&params).unwrap(),
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    start.job.extra = Some(serde_json::json!({ "planSamples": samples, "rev": 1 }));
    p.send(&HostMsg::Start(start.clone()));
    assert_eq!(result(&p.drive(no_calls)).frames.len(), 2);
    // The extra wins over any samples in the params (review L1: a submitter's would only be
    // ignored; `--replay` has no extra and reads the params'), and a wrong extra is a mismatch.
    let mut lying = params.clone();
    lying.plan_samples = samples
        .iter()
        .map(|s| fin_media::jobs::PlanSample {
            at: s.at,
            blake3: "f".repeat(64),
        })
        .collect();
    let mut with_lies = start.clone();
    with_lies.job.params = serde_json::to_value(&lying).unwrap();
    p.send(&HostMsg::Start(with_lies));
    assert_eq!(result(&p.drive(no_calls)).frames.len(), 2);
    start.job.extra = Some(
        serde_json::json!({ "planSamples": [{ "at": "1", "blake3": "0".repeat(64) }, { "at": "3/2", "blake3": "0".repeat(64) }] }),
    );
    p.send(&HostMsg::Start(start));
    let run = p.drive(no_calls);
    assert_eq!(run.error().code, "PLAN_MISMATCH");
    let mut kept = fx.params(&[r("1")]);
    kept.project_id = Some("p1".into());
    assert_eq!(result(&look(&mut p, &fx.job(None), &kept)).frames.len(), 1);
}

#[test]
fn an_h264_asset_is_skipped_as_codec_unavailable() {
    if !gpu_or_skip("an_h264_asset_is_skipped_as_codec_unavailable") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let mut p = warm_worker();
    let res = result(&look(&mut p, &job, &fx.params(&[r("5/2")])));
    assert_eq!(res.frames.len(), 1);
    assert_eq!(res.skipped.len(), 1, "{:?}", res.skipped);
    let s = &res.skipped[0];
    assert_eq!((s.code.as_str(), s.at), ("CODEC_UNAVAILABLE", r("5/2")));
    assert_eq!(s.details["asset"], fx.h264_id.as_str());
}

#[test]
fn a_missing_or_changed_original_is_skipped() {
    if !gpu_or_skip("a_missing_or_changed_original_is_skipped") {
        return;
    }
    let fx = Fixture::new();
    let mut m = fx.manifest();
    m.assets[0].original = None;
    let mut p = warm_worker();
    let res = result(&look(&mut p, &fx.job(Some(&m)), &fx.params(&[r("1")])));
    assert_eq!(res.skipped[0].code, "ORIGINALS_MISSING");
    assert_eq!(res.skipped[0].details["reason"], "missing");
    let mut m = fx.manifest();
    m.assets[0].original.as_mut().unwrap().size += 1;
    let res = result(&look(&mut p, &fx.job(Some(&m)), &fx.params(&[r("1")])));
    assert_eq!(res.skipped[0].details["reason"], "changed");
}

#[test]
fn a_warm_worker_reuses_its_device_and_sources_and_writes_the_same_bytes() {
    if !gpu_or_skip("a_warm_worker_reuses_its_device_and_sources_and_writes_the_same_bytes") {
        return;
    }
    let fx = Fixture::new();
    let params = fx.params(&[r("1/3"), r("3/2")]);
    let mut warm = warm_worker();
    let first_job = fx.job(None);
    let began = Instant::now();
    let first = result(&look(&mut warm, &first_job, &params));
    let first_ms = began.elapsed();
    // The fastest of three reused runs (one slow run under a loaded machine is noise).
    let mut second_ms = std::time::Duration::MAX;
    let mut second_job = fx.job(None);
    let mut second = None;
    for _ in 0..3 {
        let job = fx.job(None);
        let began = Instant::now();
        let res = result(&look(&mut warm, &job, &params));
        second_ms = second_ms.min(began.elapsed());
        if second.is_none() {
            second = Some(res);
            second_job = job;
        }
    }
    let second = second.unwrap();
    eprintln!("media.look of 2 frames: fresh worker {first_ms:?}, reused {second_ms:?}");
    assert!(!first.stats.device_reused);
    assert!(second.stats.device_reused, "the device is reused");
    assert_eq!(
        (first.stats.sources_opened, second.stats.sources_opened),
        (1, 0)
    );
    assert!(second.stats.sources_reused >= 2, "{:?}", second.stats);
    assert_eq!(second.stats.worker_jobs, 2);
    assert!(
        second_ms < first_ms,
        "a reused worker is faster: {second_ms:?} vs {first_ms:?}"
    );
    // cp §6.3: the same job on a fresh worker writes the same bytes.
    let mut fresh = warm_worker();
    let fresh_job = fx.job(None);
    let on_fresh = result(&look(&mut fresh, &fresh_job, &params));
    assert!(!on_fresh.stats.device_reused);
    for f in &second.frames {
        let reused = std::fs::read(second_job.out().join(&f.file)).unwrap();
        let fresh = std::fs::read(fresh_job.out().join(&f.file)).unwrap();
        assert!(
            reused == fresh,
            "{} differs between a reused and a fresh worker",
            f.file
        );
    }
}

#[test]
fn replay_runs_a_job_directory_without_a_host() {
    if !gpu_or_skip("replay_runs_a_job_directory_without_a_host") {
        return;
    }
    let fx = Fixture::new();
    let params = fx.params(&[r("7/60"), r("1")]);
    let hosted = fx.job(None);
    let mut p = warm_worker();
    let want = result(&look(&mut p, &hosted, &params));
    let job = fx.job(None);
    write_params(&job, &params);
    let out = std::process::Command::new(support::BIN)
        .args([
            "media",
            "--replay",
            job.dir.path().to_str().unwrap(),
            "--originals-anywhere",
        ])
        .env_clear()
        .envs(worker_env())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let got: LookResult = serde_json::from_value(answer["result"].clone()).unwrap();
    assert_eq!(got.frames, want.frames);
    for f in &got.frames {
        assert_eq!(
            std::fs::read(job.out().join(&f.file)).unwrap(),
            std::fs::read(hosted.out().join(&f.file)).unwrap()
        );
    }
    // A broken plan sample is the job's error, exit 1.
    let mut bad = params.clone();
    bad.plan_samples[0].blake3 = "f".repeat(64);
    write_params(&job, &bad);
    let out = std::process::Command::new(support::BIN)
        .args([
            "media",
            "--replay",
            job.dir.path().to_str().unwrap(),
            "--originals-anywhere",
        ])
        .env_clear()
        .envs(worker_env())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["error"]["code"], "PLAN_MISMATCH");
}

#[test]
fn bad_inputs_are_typed_errors() {
    let fx = Fixture::new();
    let mut p = spawn_with(&["media", "--warm"], &worker_env());
    p.hello();
    let job = fx.job(None);
    let mut params = serde_json::to_value(fx.params(&[r("1")])).unwrap();
    params["outName"] = "../x".into();
    let start = job.start(
        "media.look",
        params,
        &["in/snapshot.json", "in/manifest.json", "out/"],
    );
    p.send(&HostMsg::Start(start));
    assert_eq!(p.drive(no_calls).error().code, "JOB_PARAMS_INVALID");
    // A symlinked input is refused, and the refusal names the file relative to the job.
    #[cfg(unix)]
    {
        let real = job.path("in/real.json");
        std::fs::rename(job.path("in/manifest.json"), &real).unwrap();
        std::os::unix::fs::symlink(&real, job.path("in/manifest.json")).unwrap();
        let params = serde_json::to_value(fx.params(&[r("1")])).unwrap();
        let start = job.start(
            "media.look",
            params,
            &["in/snapshot.json", "in/manifest.json", "out/"],
        );
        p.send(&HostMsg::Start(start));
        let run = p.drive(no_calls);
        assert_eq!(run.error().code, "SHM_REFUSED");
        assert_eq!(run.error().details["file"], "in/manifest.json");
    }
    // No `in/manifest.json` grant.
    let params = serde_json::to_value(fx.params(&[r("1")])).unwrap();
    let start = job.start("media.look", params, &["in/snapshot.json", "out/"]);
    p.send(&HostMsg::Start(start));
    assert_eq!(p.drive(no_calls).error().code, "JOB_INPUT_INVALID");
}

/// Every `.rs` file below `dir`.
fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            sources(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn the_family_starts_no_process() {
    // Spec §2.9: the media family's sources never build a `Command`.
    let mut files = Vec::new();
    sources(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("src/media"),
        &mut files,
    );
    assert!(files.len() >= 6, "{files:?}");
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap();
            assert!(
                !code.contains("Command") && !code.contains("process::Child"),
                "{}:{} starts a process: {line}",
                f.display(),
                i + 1
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_running_look_has_no_children_and_holds_nothing_under_the_job_directory() {
    if !gpu_or_skip("a_running_look_has_no_children_and_holds_nothing_under_the_job_directory") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let mut p = warm_worker();
    let pid = p.pid();
    let watcher = std::thread::spawn(move || {
        let mut seen = 0;
        for _ in 0..200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            let Ok(tasks) = std::fs::read_dir(format!("/proc/{pid}/task")) else {
                break;
            };
            for t in tasks.flatten() {
                let children =
                    std::fs::read_to_string(t.path().join("children")).unwrap_or_default();
                assert!(children.trim().is_empty(), "children of {pid}: {children}");
                seen += 1;
            }
        }
        seen
    });
    let times: Vec<_> = (0..8).map(|i| r(&format!("{i}/4"))).collect();
    result(&look(&mut p, &job, &fx.params(&times)));
    assert!(watcher.join().unwrap() > 0, "the worker was observed");
    // After the job (and its `ready`) nothing under the job directory is open or mapped.
    std::thread::sleep(std::time::Duration::from_millis(100));
    let root = job.dir.path().canonicalize().unwrap();
    for fd in std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .flatten()
    {
        if let Ok(target) = std::fs::read_link(fd.path()) {
            assert!(
                !target.starts_with(&root),
                "fd {:?} → {}",
                fd.file_name(),
                target.display()
            );
        }
    }
    let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
    assert!(
        !maps.contains(root.to_str().unwrap()),
        "a mapping under the job directory"
    );
}

/// Review M3: the limit on a frame's side applies to the size actually drawn, so a timeline
/// larger than it needs a `size`.
#[test]
fn no_size_on_a_timeline_larger_than_the_limit_is_refused() {
    if !gpu_or_skip("no_size_on_a_timeline_larger_than_the_limit_is_refused") {
        return;
    }
    let mut fx = Fixture::new();
    fx.project.timelines[0].resolution = fin_core::Resolution {
        width: fin_media::jobs::LOOK_MAX_SIDE + 8,
        height: 64,
    };
    let mut params = fx.params(&[r("0")]);
    params.size = None;
    let job = fx.job(None);
    let mut p = warm_worker();
    let run = look(&mut p, &job, &params);
    let e = run.error();
    assert_eq!(e.code, "JOB_PARAMS_INVALID", "{e:?}");
    assert_eq!(e.details["field"], "size");
    assert!(
        std::fs::read_dir(job.out()).unwrap().next().is_none(),
        "no frame was drawn"
    );
    // With a size named, the same job renders.
    params.size = Some(OUT);
    let run = look(&mut p, &fx.job(None), &params);
    assert_eq!(result(&run).frames.len(), 1);
}

/// Review B3: a worker that finds `out/` replaced by a link writes nothing through it.
#[cfg(unix)]
#[test]
fn an_out_dir_that_is_a_link_is_refused() {
    if !gpu_or_skip("an_out_dir_that_is_a_link_is_refused") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let params = fx.params(&[r("0")]);
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::remove_dir(job.out()).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), job.out()).unwrap();
    let mut p = warm_worker();
    let run = look(&mut p, &job, &params);
    let e = run.error();
    assert_eq!(e.code, "SHM_REFUSED", "{e:?}");
    assert_eq!(e.details["reason"], "link");
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none(),
        "nothing was written through the link"
    );
}

/// Review B3: a retried attempt's earlier output, or a link planted at an output's name, is
/// replaced, never written through.
#[cfg(unix)]
#[test]
fn an_output_name_that_is_a_link_is_replaced_not_followed() {
    if !gpu_or_skip("an_output_name_that_is_a_link_is_replaced_not_followed") {
        return;
    }
    let fx = Fixture::new();
    let job = fx.job(None);
    let params = fx.params(&[r("0")]);
    let victim = tempfile::tempdir().unwrap().keep().join("victim");
    std::fs::write(&victim, b"keep me").unwrap();
    std::os::unix::fs::symlink(&victim, job.out().join("look-0.png")).unwrap();
    let mut p = warm_worker();
    let run = look(&mut p, &job, &params);
    assert_eq!(result(&run).frames.len(), 1);
    assert_eq!(std::fs::read(&victim).unwrap(), b"keep me");
    let (size, _) = read_png(&job.out().join("look-0.png"));
    assert_eq!(size, OUT);
}

fn replay(job: &Job, extra: &[&str]) -> std::process::Output {
    std::process::Command::new(support::BIN)
        .args(["media", "--replay", job.dir.path().to_str().unwrap()])
        .args(extra)
        .env_clear()
        .envs(worker_env())
        .output()
        .unwrap()
}

/// Review B3: `--replay` trusts nothing in the directory: a linked `out/`, an existing output
/// and originals outside the directory are refused.
#[cfg(unix)]
#[test]
fn replay_refuses_links_leftovers_and_outside_originals() {
    if !gpu_or_skip("replay_refuses_links_leftovers_and_outside_originals") {
        return;
    }
    let fx = Fixture::new();
    let params = fx.params(&[r("0")]);
    let error_of = |out: &std::process::Output| -> Value {
        assert_eq!(
            out.status.code(),
            Some(1),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["error"].clone()
    };

    // Originals elsewhere on the machine (the fixture's are in its own temp dir): refused as
    // missing, the frame still drawn and the skip reported, unless the flag allows them.
    let job = fx.job(None);
    write_params(&job, &params);
    let out = replay(&job, &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let skipped = answer["result"]["skipped"].as_array().unwrap();
    assert!(
        skipped
            .iter()
            .any(|s| s["details"]["reason"] == "outsideReplay"),
        "{answer}"
    );
    let job = fx.job(None);
    write_params(&job, &params);
    let out = replay(&job, &["--originals-anywhere"]);
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        answer["result"]["skipped"].as_array().unwrap().is_empty(),
        "{answer}"
    );

    // An output already there is never overwritten.
    let job = fx.job(None);
    write_params(&job, &params);
    std::fs::write(job.out().join("look-0.png"), b"mine").unwrap();
    let e = error_of(&replay(&job, &["--originals-anywhere"]));
    assert_eq!(
        (e["code"].as_str(), e["details"]["reason"].as_str()),
        (Some("SHM_REFUSED"), Some("exists"))
    );
    assert_eq!(
        std::fs::read(job.out().join("look-0.png")).unwrap(),
        b"mine"
    );

    // A linked `out/`: nothing is written through it.
    let job = fx.job(None);
    write_params(&job, &params);
    let elsewhere = tempfile::tempdir().unwrap();
    std::fs::remove_dir(job.out()).unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), job.out()).unwrap();
    let e = error_of(&replay(&job, &["--originals-anywhere"]));
    assert_eq!(
        (e["code"].as_str(), e["details"]["reason"].as_str()),
        (Some("SHM_REFUSED"), Some("link"))
    );
    assert!(
        std::fs::read_dir(elsewhere.path())
            .unwrap()
            .next()
            .is_none()
    );
}
