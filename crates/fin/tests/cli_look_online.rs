//! `fin look` and `fin media caps` against a real finisherd and its `fin-workers media` (plan
//! 6A Task 13): three times of a project rendered to PNGs, what a frame left out in `skipped`
//! (the fixture clip is H.264, which 6A cannot decode), the error exits, `--no-wait`, and the
//! worker's capabilities. Rendering needs a GPU adapter: `FIN_GPU=software` (lavapipe, WARP)
//! is set for the server; a machine with none skips the rendering checks unless
//! `FIN_REQUIRE_GPU=1`.

mod support;

use std::ffi::OsString;

use serde_json::{Value, json};
use support::{Server, assert_error, fin, media, ok};

fn gpu_required() -> bool {
    std::env::var("FIN_REQUIRE_GPU").is_ok_and(|v| v == "1")
}

/// Whether `run` failed only because the machine has no adapter (and that is allowed).
fn no_gpu(run: &support::Run) -> bool {
    let missing = run.code != 0 && run.json["error"]["code"] == "GPU_UNAVAILABLE";
    if missing && !gpu_required() {
        eprintln!("SKIPPED rendering: no GPU adapter here (set FIN_REQUIRE_GPU=1 to fail)");
        return true;
    }
    assert!(!missing, "FIN_REQUIRE_GPU=1 but no adapter: {}", run.stdout);
    false
}

#[test]
fn look_renders_times_and_reports_what_it_skipped() {
    let env = vec![("FIN_GPU".to_string(), OsString::from("software"))];
    let Some(server) = Server::start_with("look_renders_times", env, None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media(&dir.join("clip.mp4"), 4);
    let run = |args: &[&str]| ok(fin(sd, &dir, args));
    run(&["asset", "add", "clip.mp4"]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "2s",
    ]);

    let looked = fin(sd, &dir, &["look", "--at", "0,0.5s,f30", "--size", "96x54"]);
    if no_gpu(&looked) {
        return;
    }
    let doc = ok(looked);
    // Times are echoed resolved, canonical.
    assert_eq!(doc["timeline"], "tl1");
    assert_eq!(doc["at"], json!(["0", "1/2", "1"]), "{doc}");
    let frames = doc["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 3, "{doc}");
    let look_dir = dir.join("workspace").join("shared").join("look");
    for (f, at) in frames.iter().zip(["0", "1/2", "1"]) {
        assert_eq!(f["at"], at);
        assert_eq!(
            (f["width"].as_u64(), f["height"].as_u64()),
            (Some(96), Some(54))
        );
        let path = std::path::PathBuf::from(f["file"].as_str().unwrap());
        assert_eq!(path.parent().unwrap(), look_dir);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }
    // Information, not a failure: exit 0 with the H.264 clip left out of every frame.
    let skipped = doc["skipped"].as_array().unwrap();
    assert_eq!(skipped.len(), 3, "{doc}");
    assert!(
        skipped
            .iter()
            .all(|s| s["code"] == "CODEC_UNAVAILABLE" && s["item"] == "c1"),
        "{doc}"
    );
    assert!(doc["adapter"]["software"].is_boolean());

    // `--out`: a folder of the caller's choosing (relative to where `fin` runs).
    let out = run(&["look", "--at", "0", "--size", "8x8", "--out", "shots"]);
    assert_eq!(
        std::path::Path::new(out["frames"][0]["file"].as_str().unwrap())
            .parent()
            .unwrap(),
        dir.join("shots")
    );

    // A time past the timeline's end, and a timeline that does not exist.
    assert_error(&fin(sd, &dir, &["look", "--at", "3s"]), 3, "INVALID_RANGE");
    assert_error(
        &fin(sd, &dir, &["--timeline", "tl9", "look", "--at", "0"]),
        3,
        "NOT_FOUND",
    );
    assert_error(
        &fin(sd, &dir, &["look", "--at", "0", "--size", "wide"]),
        2,
        "USAGE",
    );
    assert_error(
        &fin(sd, &dir, &["look", "--at", "0", "--size", "9000x10"]),
        3,
        "INVALID_RANGE",
    );
    assert_error(
        &fin(sd, &dir, &["look", "--at", "0", "--out", "assets/x"]),
        3,
        "EXPORT_DESTINATION",
    );

    // `--no-wait`: the job at once.
    let job = run(&["look", "--at", "0", "--size", "12x12", "--no-wait"]);
    assert_eq!(job["job"]["kind"], "media.look", "{job}");
    assert_eq!(job["at"], json!(["0"]));
    assert!(job.get("frames").is_none());
}

#[test]
fn media_caps_reports_codecs_gpu_and_versions() {
    let env = vec![("FIN_GPU".to_string(), OsString::from("software"))];
    let Some(server) = Server::start_with("media_caps", env, None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    let caps = fin(sd, root, &["media", "caps"]);
    if no_gpu(&caps) {
        return;
    }
    let doc: Value = ok(caps);
    let video = doc["codecs"]["video"].as_array().unwrap();
    let av1 = video.iter().find(|c| c["codec"] == "av1").unwrap();
    assert_eq!(
        (av1["decode"].clone(), av1["encode"].clone()),
        (json!("software"), json!("software"))
    );
    assert_eq!(doc["worker"]["renderVersion"], 1);
    assert!(
        doc["gpu"].is_null() || doc["gpu"]["vendor"].is_number(),
        "{doc}"
    );
}

#[test]
fn look_and_caps_are_unavailable_offline() {
    let snapshot = json!({ "rev": 1, "project": fin_core::new_project("p".into(), "P".into(), "30".parse().unwrap()).unwrap() });
    let r = support::offline(&snapshot, &["look", "--at", "0"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["media", "caps"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
