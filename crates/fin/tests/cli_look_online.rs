//! `fin look`, `fin media caps` and `fin media probe` against a real finisherd and its `fin-workers media` (plan
//! 6A Task 13): three times of a project rendered to PNGs, what a frame left out in `skipped`
//! (the fixture clip is H.264, which 6A cannot decode), the error exits, `--no-wait`, and the
//! worker's capabilities, the decode-level probe of an asset and `fin asset reprobe` (6B-1 Task 11),
//! and `fin media proxy|status` (6B-1 Task 14: the fixture's H.264 has no decoder on Linux). Rendering needs a GPU adapter: `FIN_GPU=software` (lavapipe, WARP)
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
    assert_eq!(doc["worker"]["renderVersion"], 2);
    assert!(
        doc["gpu"].is_null() || doc["gpu"]["vendor"].is_number(),
        "{doc}"
    );
}

#[test]
fn media_probe_reports_the_decoder_and_commits_nothing() {
    let Some(server) = Server::start_with("media_probe", Vec::new(), None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media(&dir.join("clip.mp4"), 1);
    ok(fin(sd, &dir, &["asset", "add", "clip.mp4"]));
    let rev = ok(fin(sd, &dir, &["status"]))["rev"].clone();

    let doc = ok(fin(sd, &dir, &["media", "probe", "a1"]));
    assert_eq!(
        (&doc["asset"], &doc["codec"]),
        (&json!("a1"), &json!("h264"))
    );
    assert!(doc["fingerprint"].is_string(), "{doc}");
    if doc["decodable"] == true {
        // An OS decoder took it (macOS, Windows): it says which and of what class.
        assert!(
            doc["backend"].is_string() && doc["class"].is_string(),
            "{doc}"
        );
        assert!(doc["firstFrame"]["width"].is_number(), "{doc}");
    } else {
        // No H.264 decoder here (Linux until 6B-2): an answer, not an error.
        assert_eq!(doc["failure"]["code"], "CODEC_UNAVAILABLE", "{doc}");
        assert!(doc.get("backend").is_none(), "{doc}");
    }
    // Nothing was committed.
    assert_eq!(ok(fin(sd, &dir, &["status"]))["rev"], rev);

    assert_error(
        &fin(sd, &dir, &["media", "probe", "a9"]),
        3,
        "ASSET_NOT_FOUND",
    );
}

/// `fin asset reprobe` (6B-1 Task 11): `fin asset add` of a portrait clip (a 90° `tkhd`
/// matrix) already records it 180x320, rotated 90, so a reprobe finds nothing new and commits
/// nothing (`unchanged`; `apps/finisherd/test/asset-reprobe.test.ts` shows one filling a
/// schema 3 asset). `--deep` decodes first: a codec this machine cannot decode is
/// `CODEC_UNAVAILABLE` (exit 5). A changed file is `ASSET_CHANGED` (exit 3).
#[test]
fn asset_reprobe_reports_the_display_geometry() {
    let Some(server) = Server::start_with("asset_reprobe", Vec::new(), None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    let clip = dir.join("portrait.mp4");
    fin_probe::fixtures::write_fixture_mp4_rotated(
        &clip,
        fin_probe::fixtures::FixtureOpts::default(),
        fin_core::Rotation::R90,
    );
    ok(fin(sd, &dir, &["asset", "add", "portrait.mp4"]));
    let rev = ok(fin(sd, &dir, &["status"]))["rev"].as_u64().unwrap();

    let doc = ok(fin(sd, &dir, &["asset", "reprobe", "a1"]));
    assert_eq!(doc["asset"], "a1", "{doc}");
    assert_eq!(
        (&doc["video"]["rotation"], &doc["video"]["display"]),
        (&json!(90), &json!({ "width": 180, "height": 320 })),
        "{doc}"
    );
    assert_eq!(
        (&doc["video"]["width"], &doc["video"]["height"]),
        (&json!(320), &json!(180))
    );
    assert_eq!(
        (&doc["unchanged"], &doc["rev"]),
        (&json!(true), &json!(rev)),
        "{doc}"
    );
    assert_eq!(ok(fin(sd, &dir, &["status"]))["rev"], rev);

    // A dry run probes and commits nothing.
    let dry = ok(fin(sd, &dir, &["asset", "reprobe", "a1", "--dry-run"]));
    assert_eq!(dry["dryRun"], true, "{dry}");
    assert_eq!(ok(fin(sd, &dir, &["status"]))["rev"], rev);

    // The fixture is H.264: decodable here or not, the answer is typed.
    let deep = fin(sd, &dir, &["asset", "reprobe", "a1", "--deep"]);
    if deep.code == 0 {
        assert_eq!(deep.json["deep"]["decodable"], true, "{}", deep.stdout);
        assert_eq!(deep.json["unchanged"], true, "{}", deep.stdout);
    } else {
        assert_error(&deep, 5, "CODEC_UNAVAILABLE");
        assert_eq!(
            deep.json["error"]["details"]["asset"], "a1",
            "{}",
            deep.stdout
        );
    }
    assert_eq!(ok(fin(sd, &dir, &["status"]))["rev"], rev);

    assert_error(
        &fin(sd, &dir, &["asset", "reprobe", "a9"]),
        3,
        "ASSET_NOT_FOUND",
    );
    // Another file at the asset's path: a relink, not a reprobe.
    media(&clip, 3);
    assert_error(
        &fin(sd, &dir, &["asset", "reprobe", "a1"]),
        3,
        "ASSET_CHANGED",
    );
}

#[test]
fn media_proxy_and_status_answer_typed() {
    let Some(server) = Server::start_with("media_proxy", Vec::new(), None) else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media(&dir.join("clip.mp4"), 1);
    ok(fin(sd, &dir, &["asset", "add", "clip.mp4"]));
    let rev = ok(fin(sd, &dir, &["status"]))["rev"].clone();
    let made = fin(
        sd,
        &dir,
        &["media", "proxy", "a1", "--tier", "540", "--no-wait"],
    );
    let status = fin(sd, &dir, &["media", "status", "a1"]);
    if made.code == 0 {
        // A decoder for H.264 here (macOS, Windows): a proxy is ready or being made.
        assert!(
            made.json["state"] == "ready" || made.json["state"] == "making",
            "{}",
            made.stdout
        );
        assert_eq!(ok(status)["tiers"].as_array().unwrap().len(), 2);
    } else {
        // No H.264 decoder (Linux until 6B-2): the key cannot be planned.
        assert_error(&made, 5, "CODEC_UNAVAILABLE");
        assert_eq!(made.json["error"]["details"]["direction"], "decode");
        assert_error(&status, 5, "CODEC_UNAVAILABLE");
    }
    // Asking commits nothing.
    assert_eq!(ok(fin(sd, &dir, &["status"]))["rev"], rev);
    assert_error(
        &fin(sd, &dir, &["media", "proxy", "a9"]),
        3,
        "ASSET_NOT_FOUND",
    );
    let bad = fin(sd, &dir, &["media", "proxy", "a1", "--codec", "hevc"]);
    assert_eq!(bad.code, 2, "{}", bad.stdout);
}

#[test]
fn look_and_caps_are_unavailable_offline() {
    let snapshot = json!({ "rev": 1, "project": fin_core::new_project("p".into(), "P".into(), "30".parse().unwrap()).unwrap() });
    let r = support::offline(&snapshot, &["look", "--at", "0"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["media", "caps"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["media", "probe", "a1"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["asset", "reprobe", "a1"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["media", "proxy", "a1"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
    let r = support::offline(&snapshot, &["media", "status", "a1"]);
    assert_error(&r, 5, "SERVER_UNAVAILABLE");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
