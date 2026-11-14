//! `fin param|keyframe|effect|style` committed through a test-hooks finisherd, and `fin eval
//! frame|audio` reading the result (plan 4C Task 24): the frame plan carries the committed
//! settings and a caption laid out from its pinned, cached font; the audio plan the gain.

mod support;

use serde_json::json;
use support::fontserver::{self, CatalogFile, FontServer};
use support::{Server, assert_error, fin, media_av, ok};

#[test]
fn settings_commit_and_eval_reads_them() {
    let fonts = FontServer::start();
    let bytes = fin_testfonts::test_sans();
    let url = fonts.serve("sans.ttf", &bytes);
    let catalog = fontserver::catalog_json(
        "2026.11.1",
        &[CatalogFile {
            id: "sans",
            family: "Test Sans",
            weight: 400,
            bytes: &bytes,
            url,
        }],
        &["sans"],
    );
    let Some(f) = fontserver::start("settings_commit_and_eval_reads_them", &catalog) else {
        return;
    };
    let (sd, root) = (&f.server.sd, &f.server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media_av(&dir.join("av.mp4"), 10);
    let run = |args: &[&str]| ok(fin(sd, &dir, args));
    run(&["asset", "add", "av.mp4"]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s",
    ]);
    // c1 video, c2 its linked audio.
    let p = run(&["param", "set", "c1", "transform.rotation", "90deg"]);
    assert_eq!(p["resolved"]["value"], 90.0);
    run(&["param", "set", "c2", "audio.gain", "-6dB"]);
    run(&["keyframe", "set", "c1", "opacity", "--at", "0", "0"]);
    run(&[
        "keyframe", "set", "c1", "opacity", "--at", "2s", "1", "--interp", "ease",
    ]);
    let fx = run(&["effect", "add", "c1", "blur", "--set", "radius=6px"]);
    assert_eq!(fx["bindings"], json!({ "$fx": "fx1" }));
    run(&["effect", "add", "c1", "colorAdjust"]);
    run(&["effect", "move", "c1", "fx2", "--to", "0"]);
    let r = fin(
        sd,
        &dir,
        &["keyframe", "set", "c1", "blend", "--at", "0", "screen"],
    );
    assert_error(&r, 3, "PARAM_NOT_ANIMATABLE");

    // A caption in a project style, its font fetched first so eval measures it.
    run(&["track", "add", "--kind", "caption"]);
    let st = run(&[
        "style",
        "add",
        "--name",
        "S",
        "--from",
        "preset:clean",
        "--set",
        "text.family=Test Sans",
        "--set",
        "text.weight=400",
    ]);
    assert_eq!(st["bindings"], json!({ "$style": "st1" }));
    run(&["fonts", "fetch", "sans"]);
    std::fs::write(
        dir.join("w.json"),
        json!([{ "text": "Hello", "start": 0, "end": 1 }, { "text": "world", "start": 1, "end": 2 }]).to_string(),
    )
    .unwrap();
    let import = run(&[
        "caption", "import", "--track", "t3", "--words", "w.json", "--style", "st1",
    ]);
    assert_eq!(
        import["resolved"]["fonts"]["planned"][0]["key"], "Test Sans|400|normal",
        "{import}"
    );

    let frame = run(&["eval", "frame", "--at", "1s"]);
    assert_eq!(frame["timeline"], "tl1");
    let plan = &frame["plan"];
    // The timeline fin resolved is the one the server evaluated (review #52).
    assert_eq!(plan["timeline"], frame["timeline"], "{frame}");
    assert_eq!(plan["t"], "1");
    let layers = plan["layers"].as_array().unwrap();
    let clip = layers.iter().find(|l| l["item"] == "c1").unwrap();
    assert_eq!(clip["transform"]["rotation"], 90.0);
    let kinds: Vec<&str> = clip["effects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["colorAdjust", "blur"]);
    let opacity = clip["opacity"].as_f64().unwrap();
    assert!(opacity > 0.0 && opacity < 1.0, "{opacity}");
    let caption = layers.iter().find(|l| l["item"] == "cap1").unwrap();
    assert!(caption["caption"]["layout"].is_object(), "{caption}");
    assert!(caption["caption"].get("unmeasured").is_none(), "{caption}");

    let audio = run(&["eval", "audio", "--range", "0..2s"]);
    let item = &audio["plan"]["items"][0];
    assert_eq!(item["item"], "c2", "{audio}");
    // The server's JSON writes -6.0 as -6.
    assert_eq!(item["gain"][0]["db"][0].as_f64(), Some(-6.0), "{audio}");
    assert_eq!(item["gain"][0]["db"][1].as_f64(), Some(-6.0), "{audio}");
    let r = fin(sd, &dir, &["eval", "audio", "--range", "2s..1s"]);
    assert_error(&r, 3, "INVALID_RANGE");
    let r = fin(
        sd,
        &dir,
        &["--timeline", "tl9", "eval", "frame", "--at", "0"],
    );
    assert_error(&r, 3, "NOT_FOUND");

    // The import's undo also drops its planned pin, though st1 (added before it) still names
    // the face: the internal unpin skips the use check.
    run(&["undo"]);
    let pins = run(&["fonts", "ls", "--pinned"]);
    assert_eq!(pins["pins"], json!([]), "{pins}");
    let view = run(&["view", "--track", "t3"]);
    assert_eq!(support::view_items(&view), [], "{view}");
}

/// The `kf:` part of c1's line in `fin view --detail` (its link part cut).
fn keys(run: &dyn Fn(&[&str]) -> serde_json::Value) -> String {
    let v = run(&["view", "--detail", "--track", "t1"]);
    let lines = v["lines"].as_array().unwrap();
    let line = lines
        .iter()
        .filter_map(|l| l.as_str())
        .find(|l| l.contains(" c1 "))
        .unwrap_or_else(|| panic!("{v}"));
    let kf = line.split_once("kf: ").map_or("", |(_, k)| k);
    kf.split("  ⟷").next().unwrap_or_default().to_string()
}

/// Spec §5.12.3: every keyframe write echoes its key's id; an insert's is the id the server
/// minted (after an undo it is not the one the working copy would guess: removed ids are not
/// reused), a dry run's the one it would mint. Keys are edited, moved and removed by id.
#[test]
fn keyframe_ids_are_echoed_and_named() {
    let Some(server) = Server::start("keyframe_ids_are_echoed_and_named") else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    media_av(&dir.join("av.mp4"), 10);
    let run = |args: &[&str]| ok(fin(sd, &dir, args));
    run(&["asset", "add", "av.mp4"]);
    run(&[
        "clip", "insert", "--track", "t1", "--at", "0", "--asset", "a1", "--dur", "4s",
    ]);

    let k = run(&["keyframe", "set", "c1", "opacity", "--at", "0", "0"]);
    assert_eq!(k["bindings"], json!({ "$kf0": "kf1" }), "{k}");
    assert_eq!(k["resolved"]["key"], "kf1", "{k}");
    let k = run(&["keyframe", "set", "c1", "opacity", "--at", "0", "0.5"]);
    assert_eq!(k["resolved"]["key"], "kf1", "an edit keeps the id: {k}");
    run(&["keyframe", "set", "c1", "opacity", "--at", "2s", "1"]);
    run(&["undo"]);
    let dry = run(&[
        "--dry-run",
        "keyframe",
        "set",
        "c1",
        "opacity",
        "--at",
        "3s",
        "1",
    ]);
    assert_eq!(dry["resolved"]["key"], dry["bindings"]["$kf0"], "{dry}");
    let k = run(&["keyframe", "set", "c1", "opacity", "--at", "3s", "1"]);
    assert_eq!(k["bindings"], json!({ "$kf0": "kf3" }), "{k}");
    assert_eq!(k["resolved"]["key"], "kf3", "the minted id: {k}");
    assert_eq!(keys(&run), "opacity[kf1@0.0s,kf3@3.0s]");

    let r = fin(
        sd,
        &dir,
        &[
            "keyframe", "set", "c1", "opacity", "--key", "kf3", "--at", "0", "1",
        ],
    );
    assert_error(&r, 3, "KEYFRAME_EXISTS");
    assert_eq!(
        r.json["error"]["details"]["existing"], "kf1",
        "{}",
        r.stdout
    );
    let k = run(&[
        "keyframe", "set", "c1", "opacity", "--key", "kf3", "--at", "1s", "1",
    ]);
    assert_eq!(k["resolved"]["key"], "kf3", "{k}");
    assert_eq!(k["resolved"]["at"], "1", "{k}");
    let k = run(&["keyframe", "set", "c1", "opacity", "--key", "kf1", "0.25"]);
    assert_eq!(k["resolved"]["at"], "0", "--key alone keeps the time: {k}");
    assert_eq!(keys(&run), "opacity[kf1@0.0s,kf3@1.0s]");

    let k = run(&["keyframe", "remove", "c1", "opacity", "--key", "kf1"]);
    assert_eq!(
        k["resolved"],
        json!({ "item": "c1", "path": "opacity", "key": "kf1", "at": "0" })
    );
    let k = run(&["keyframe", "remove", "c1", "opacity", "--at", "1s"]);
    assert_eq!(k["resolved"]["key"], "kf3", "{k}");
    assert_eq!(keys(&run), "");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
