//! `fin` edit commands (plan 4C Task 23) against the `FIN_OFFLINE_SNAPSHOT` hook: each
//! command's dry-run document and its main refusal (exit code and `fix`), linked audio, mode
//! defaults and echoes, composed overwrites and transitions, and the SP2 output follow-ups.
//! `cli_edit_online.rs` commits the same commands through a live server.

mod support;

use serde_json::{Value, json};
use support::{Run, offline};

/// Rate 30. Assets: a1 media with video and audio (60 s), a2 video-only media, a3 an image, a4
/// a Lottie template (3 s; slots `title` text, `tint` colour), a5 unused media. Timeline tl1:
/// - t1 video: c1 [0, 5) and c2 [5, 10), both a1 (c2 from 5 s), linked to c3 / c4;
/// - t2 audio: c3 [0, 5), c4 [5, 10), crossfade tr1 at their cut;
/// - t3 video, locked: c5 [0, 2) a2;
/// - t4 audio, empty;
/// - t6 video: img1 [0, 3) a3, g1 [3, 6) a4.
///
/// Marker m1 at 2 s. Timeline tl2 (rate 25): t5 video, empty. Bins b1, b2 inside b1.
fn snapshot() -> Value {
    let clip = |id: &str, start: &str, source: &str, asset: &str, link: Option<&str>| {
        let mut c = json!({ "id": id, "kind": "clip", "asset": asset, "start": start,
                            "sourceIn": source, "duration": "5" });
        if let Some(l) = link {
            c["link"] = json!(l);
        }
        c
    };
    let mut c5 = clip("c5", "0", "0", "a2", None);
    c5["duration"] = json!("2");
    let media = |id: &str, audio: bool| {
        let mut a = json!({ "id": id, "kind": "media", "path": format!("/m/{id}.mp4"), "duration": "60",
                            "video": { "width": 1920, "height": 1080, "rate": "30" }, "fingerprint": id });
        if audio {
            a["audio"] = json!({ "sampleRate": 48000, "channels": 2 });
        }
        a
    };
    json!({
        "rev": 7,
        "project": {
            "id": "p1", "name": "Demo", "schemaVersion": 2, "rate": "30",
            "timelines": [
                {
                    "id": "tl1", "name": "Timeline 1", "rate": "30",
                    "resolution": { "width": 1080, "height": 1920 },
                    "markers": [{ "id": "m1", "at": "2", "label": "drop" }],
                    "tracks": [
                        { "id": "t1", "kind": "video", "name": "V1", "items": [
                            clip("c1", "0", "0", "a1", Some("lk1")), clip("c2", "5", "5", "a1", Some("lk2")) ] },
                        { "id": "t2", "kind": "audio", "name": "A1", "items": [
                            clip("c3", "0", "0", "a1", Some("lk1")), clip("c4", "5", "5", "a1", Some("lk2")) ],
                          "transitions": [{ "id": "tr1", "between": ["c3", "c4"], "kind": "crossfade", "duration": "1" }] },
                        { "id": "t3", "kind": "video", "name": "V2", "lock": true, "items": [c5] },
                        { "id": "t4", "kind": "audio", "name": "A2", "items": [] },
                        { "id": "t6", "kind": "video", "name": "V3", "items": [
                            { "id": "img1", "kind": "image", "asset": "a3", "start": "0", "duration": "3" },
                            { "id": "g1", "kind": "graphic", "asset": "a4", "start": "3", "duration": "3" } ] },
                    ],
                },
                {
                    "id": "tl2", "name": "Alt", "rate": "25",
                    "resolution": { "width": 1920, "height": 1080 },
                    "tracks": [{ "id": "t5", "kind": "video", "name": "V1", "items": [] }],
                },
            ],
            "bins": { "b1": { "id": "b1", "name": "Footage" }, "b2": { "id": "b2", "name": "Sub", "parent": "b1" } },
            "assets": {
                "a1": media("a1", true),
                "a2": media("a2", false),
                "a3": { "id": "a3", "kind": "image", "path": "/m/a3.png", "fingerprint": "a3",
                        "image": { "width": 640, "height": 480 } },
                "a4": { "id": "a4", "kind": "lottieTemplate", "path": "/m/a4.json", "fingerprint": "a4",
                        "duration": "3",
                        "lottie": { "width": 1080, "height": 1920, "rate": "30", "duration": "3",
                                    "slots": [{ "id": "title", "kind": "text" }, { "id": "tint", "kind": "color" }] } },
                "a5": media("a5", false),
            },
        },
    })
}

fn run(args: &[&str]) -> Run {
    offline(&snapshot(), args)
}

/// A successful dry run; returns the document.
fn dry(args: &[&str]) -> Value {
    let r = run(args);
    assert_eq!(r.code, 0, "{args:?}: {}", r.stdout);
    assert_eq!(r.json["dryRun"], true, "{args:?}");
    r.json
}

/// A refusal with `code` (exit `exit`) whose fix contains `fix`.
fn refused(args: &[&str], exit: i32, code: &str, fix: &str) -> Value {
    let r = run(args);
    assert_eq!(r.code, exit, "{args:?}: {}", r.stdout);
    assert_eq!(r.json["error"]["code"], code, "{args:?}: {}", r.stdout);
    let f = r.json["error"]["fix"].as_str().unwrap_or_default();
    assert!(f.contains(fix), "{args:?}: fix {f:?} lacks {fix:?}");
    r.json["error"].clone()
}

/// `subset`'s keys all hold the same values in `value`.
fn has(value: &Value, subset: Value) {
    for (k, v) in subset.as_object().unwrap() {
        assert_eq!(&value[k], v, "{k} in {value}");
    }
}

#[test]
fn clip_insert_links_audio_by_default() {
    let doc = dry(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a1", "--dur", "2s",
    ]);
    assert_eq!(
        doc["bindings"],
        json!({ "$clip": "c6", "$audio": "c7", "$link": "lk3" })
    );
    has(
        &doc["resolved"],
        json!({ "at": "10", "duration": "2", "mode": "ripple",
                "audio": { "track": "t2", "id": "$audio" }, "link": "$link" }),
    );
    let added: Vec<&str> = doc["diff"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["change"] == "added")
        .map(|c| c["id"].as_str().unwrap())
        .collect();
    assert_eq!(added, ["c6", "c7", "lk3"]);
    let named = dry(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--audio-track",
        "t4",
    ]);
    assert_eq!(named["resolved"]["audio"]["track"], "t4");
    // A video-only asset gets no audio.
    let video = dry(&[
        "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a2", "--dur", "2s",
    ]);
    assert_eq!(video["bindings"], json!({ "$clip": "c6" }));
}

#[test]
fn clip_insert_no_audio() {
    let doc = dry(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--no-audio",
    ]);
    assert_eq!(doc["bindings"], json!({ "$clip": "c6" }));
    assert!(
        doc["resolved"].get("audio").is_none(),
        "{}",
        doc["resolved"]
    );
    let both = run(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "end",
        "--asset",
        "a1",
        "--dur",
        "2s",
        "--no-audio",
        "--audio-track",
        "t4",
    ]);
    assert_eq!(both.code, 2, "{}", both.stdout);
}

#[test]
fn clip_insert_without_audio_track_is_usage() {
    let e = refused(
        &[
            "clip", "insert", "--track", "t5", "--at", "0", "--asset", "a1", "--dur", "1s",
        ],
        2,
        "USAGE",
        "--no-audio",
    );
    assert_eq!(e["details"]["timeline"], "tl2");
    // The audio landing inside an item says where, and how to avoid it.
    refused(
        &[
            "clip", "insert", "--track", "t6", "--at", "3s", "--asset", "a1", "--dur", "1s",
        ],
        3,
        "INSERT_INSIDE_CLIP",
        "--no-audio",
    );
    // `--audio-track` that would place no audio is refused, not ignored: a video-only asset,
    // or a clip on an audio track.
    for (track, asset) in [("t1", "a2"), ("t4", "a1")] {
        let e = refused(
            &[
                "clip",
                "insert",
                "--track",
                track,
                "--at",
                "end",
                "--asset",
                asset,
                "--dur",
                "1s",
                "--audio-track",
                "t4",
            ],
            2,
            "USAGE",
            "drop --audio-track",
        );
        assert_eq!(e["details"]["audioTrack"], "t4", "{track} {asset}");
    }
}

#[test]
fn mode_default_and_echo() {
    let r = |args: &[&str]| dry(args)["resolved"]["mode"].clone();
    assert_eq!(r(&["clip", "delete", "c2"]), "ripple");
    assert_eq!(r(&["clip", "delete", "c2", "--mode", "lift"]), "lift");
    assert_eq!(r(&["clip", "trim", "c2", "--end", "-1s"]), "ripple");
    assert_eq!(r(&["item", "delete", "c2"]), "ripple");
    assert_eq!(r(&["item", "delete", "img1"]), "lift");
    assert_eq!(r(&["item", "trim", "img1", "--end", "-1s"]), "overwrite");
    assert_eq!(r(&["item", "trim", "c2", "--start", "+1s"]), "ripple");
    assert_eq!(r(&["item", "move", "c2", "--to", "20s"]), "ripple");
    assert_eq!(r(&["item", "move", "img1", "--to", "20s"]), "overwrite");
    assert_eq!(
        r(&["item", "move", "img1", "--to", "20s", "--mode", "ripple"]),
        "ripple"
    );
    assert_eq!(
        r(&[
            "image", "insert", "--track", "t6", "--at", "end", "--asset", "a3", "--dur", "1s"
        ]),
        "overwrite"
    );
    // A ripple delete of a lifted kind closes the gap: the shift says so.
    let lifted = dry(&["item", "delete", "img1"]);
    assert_eq!(lifted["diff"]["shifts"], json!([]));
    let rippled = dry(&["item", "delete", "img1", "--mode", "ripple"]);
    assert_eq!(rippled["diff"]["shifts"][0]["by"], "-3");
}

#[test]
fn overwrite_inside_item_composes_split() {
    let doc = dry(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "1s",
        "--asset",
        "a2",
        "--dur",
        "1s",
        "--mode",
        "overwrite",
    ]);
    assert_eq!(doc["bindings"], json!({ "$split": "c6", "$clip": "c7" }));
    has(
        &doc["resolved"],
        json!({ "mode": "overwrite", "splits": [{ "item": "c1", "offset": "1", "id": "$split" }] }),
    );
    // With its audio: both tracks split first.
    let av = dry(&[
        "clip",
        "insert",
        "--track",
        "t1",
        "--at",
        "1s",
        "--asset",
        "a1",
        "--dur",
        "1s",
        "--mode",
        "overwrite",
    ]);
    let splits: Vec<&str> = av["resolved"]["splits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["item"].as_str().unwrap())
        .collect();
    assert_eq!(splits, ["c1", "c3"]);
    // An image overwrite inside an item splits it too.
    let img = dry(&[
        "image", "insert", "--track", "t6", "--at", "1s", "--asset", "a3", "--dur", "1s",
    ]);
    assert_eq!(img["resolved"]["split"]["item"], "img1");
    assert_eq!(
        img["bindings"],
        json!({ "$split": "img2", "$image": "img3" })
    );
}

#[test]
fn transition_set_crossfades_linked_audio() {
    let doc = dry(&["transition", "set", "--between", "c1", "c2", "--dur", "1s"]);
    assert_eq!(doc["bindings"], json!({ "$transition": "tr2" }));
    has(
        &doc["resolved"],
        json!({ "kind": "dissolve", "duration": "1", "curve": "linear" }),
    );
    let t = &doc["resolved"]["transitions"];
    assert_eq!(
        t[0],
        json!({ "id": "$transition", "track": "t1", "between": ["c1", "c2"] })
    );
    // The partners' cut has tr1 already: it is updated, not refused TRANSITION_EXISTS.
    assert_eq!(
        t[1],
        json!({ "id": "tr1", "track": "t2", "between": ["c3", "c4"], "updated": true })
    );
    let alone = dry(&[
        "transition",
        "set",
        "--between",
        "c1",
        "c2",
        "--dur",
        "1s",
        "--no-linked",
        "--curve",
        "equalPower",
    ]);
    assert_eq!(
        alone["resolved"]["transitions"].as_array().unwrap().len(),
        1
    );
    assert_eq!(alone["resolved"]["curve"], "equalPower");
    refused(
        &["transition", "set", "--between", "c2", "c1", "--dur", "1s"],
        3,
        "NOT_ADJACENT",
        "--between",
    );
    refused(
        &["transition", "set", "--between", "c1", "c2", "--dur", "20s"],
        3,
        "INVALID_RANGE",
        "",
    );
}

#[test]
fn trim_echoes_resulting_edge() {
    let end = dry(&["clip", "trim", "c2", "--end", "-1s"]);
    assert_eq!(
        end["resolved"],
        json!({ "clip": "c2", "edge": "end", "delta": "-1", "mode": "ripple", "edgeAt": "9" })
    );
    let start = dry(&["item", "trim", "c2", "--start", "+1s"]);
    has(
        &start["resolved"],
        json!({ "edgeAt": "5", "items": ["c2", "c4"] }),
    );
    let img = dry(&["item", "trim", "img1", "--end", "+1s"]);
    has(
        &img["resolved"],
        json!({ "edgeAt": "4", "mode": "overwrite" }),
    );
}

#[test]
fn out_before_in_wording() {
    let e = refused(
        &[
            "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a2", "--in", "5s",
            "--out", "2s",
        ],
        3,
        "INVALID_RANGE",
        "--out later than 5s",
    );
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .starts_with("--out must be after --in")
    );
    assert_eq!(
        e["details"],
        json!({ "field": "out", "in": "5", "out": "2" })
    );
}

#[test]
fn negative_max_wording() {
    let e = refused(
        &[
            "clip", "insert", "--track", "t1", "--at", "end", "--asset", "a2", "--in", "70s",
            "--dur", "1s",
        ],
        3,
        "INVALID_RANGE",
        "below 60s, the duration of asset a2",
    );
    assert!(
        e["message"]
            .as_str()
            .unwrap()
            .contains("sourceIn is at or past the asset's end")
    );
    assert!(e["details"].get("max").is_none());
}

#[test]
fn project_open_dry_run_is_usage() {
    let r = run(&["project", "open", ".", "--dry-run"]);
    assert_eq!(r.code, 2, "{}", r.stdout);
    assert_eq!(r.json["error"]["code"], "USAGE");
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("without --dry-run")
    );
}

#[test]
fn item_commands_dry_run_and_refuse() {
    has(
        &dry(&["item", "move", "c2", "--to", "20s"])["resolved"],
        json!({ "item": "c2", "track": "t1", "at": "20", "items": ["c2", "c4"] }),
    );
    refused(
        &["item", "move", "c1", "--to", "2s"],
        3,
        "MOVE_INTO_ITSELF",
        "--mode overwrite",
    );
    // c1's partner c3 is on t2 already: both legs would land there.
    refused(
        &["item", "move", "c1", "--to", "20s", "--track", "t2"],
        3,
        "LINK_LEGS_OVERLAP",
        "fin item unlink c1",
    );
    refused(
        &["item", "move", "img1", "--to", "20s", "--track", "t2"],
        3,
        "TRACK_KIND_MISMATCH",
        "",
    );

    let split = dry(&["item", "split", "c1", "--at", "2s"]);
    assert_eq!(
        split["bindings"],
        json!({ "$split": "c6", "$split2": "c7", "$link": "lk3" })
    );
    assert_eq!(split["resolved"]["link"], "$link");
    refused(
        &["item", "split", "c1", "--at", "0"],
        3,
        "INVALID_RANGE",
        "",
    );

    has(
        &dry(&["item", "delete", "c2"])["resolved"],
        json!({ "items": ["c2", "c4"] }),
    );
    refused(
        &["item", "delete", "c5"],
        3,
        "TRACK_LOCKED",
        "fin track set t3 lock false",
    );

    refused(
        &["item", "trim", "c2", "--end", "+1s", "--mode", "roll"],
        3,
        "NOT_ADJACENT",
        "--mode ripple",
    );

    has(
        &dry(&["item", "slip", "c2", "-1s"])["resolved"],
        json!({ "delta": "-1", "sourceIn": "4" }),
    );
    refused(&["item", "slip", "c2", "1s"], 3, "INVALID_TIME", "+1s");

    has(
        &dry(&["item", "slide", "img1", "+1s"])["resolved"],
        json!({ "start": "1" }),
    );
    refused(&["item", "slide", "img1", "-1s"], 3, "INVALID_RANGE", "");

    let link = dry(&["item", "link", "img1", "g1"]);
    assert_eq!(link["bindings"], json!({ "$link": "lk3" }));
    refused(
        &["item", "link", "img1", "c2"],
        3,
        "ALREADY_LINKED",
        "fin item unlink lk2",
    );
    assert_eq!(run(&["item", "link", "img1"]).code, 2);

    assert_eq!(
        dry(&["item", "unlink", "lk1"])["resolved"],
        json!({ "group": "lk1", "items": ["c1", "c3"] })
    );
    assert_eq!(dry(&["item", "unlink", "c3"])["resolved"]["group"], "lk1");
    refused(
        &["item", "unlink", "img1"],
        3,
        "INVALID_OP",
        "fin item link",
    );
}

#[test]
fn media_commands_dry_run_and_refuse() {
    let img = dry(&[
        "image", "insert", "--track", "t6", "--at", "end", "--asset", "a3", "--dur", "2s",
    ]);
    assert_eq!(img["bindings"], json!({ "$image": "img2" }));
    has(
        &img["resolved"],
        json!({ "at": "6", "duration": "2", "asset": "a3" }),
    );
    refused(
        &[
            "image", "insert", "--track", "t6", "--at", "end", "--asset", "a1", "--dur", "2s",
        ],
        3,
        "INVALID_OP",
        "",
    );

    let g = dry(&[
        "graphic",
        "insert",
        "--track",
        "t6",
        "--at",
        "end",
        "--template",
        "a4",
        "--slot",
        "title=Hi",
        "--slot",
        "tint=#F80",
    ]);
    assert_eq!(g["bindings"], json!({ "$graphic": "g2" }));
    has(
        &g["resolved"],
        json!({ "duration": "3", "slots": { "title": "Hi", "tint": "#ff8800" } }),
    );
    refused(
        &[
            "graphic",
            "insert",
            "--track",
            "t6",
            "--at",
            "end",
            "--template",
            "a4",
            "--slot",
            "nope=1",
        ],
        3,
        "SLOT_UNKNOWN",
        "title",
    );
    refused(
        &[
            "graphic",
            "insert",
            "--track",
            "t6",
            "--at",
            "end",
            "--template",
            "a4",
            "--slot",
            "tint=red",
        ],
        3,
        "PARAM_TYPE",
        "#",
    );
    refused(
        &[
            "graphic",
            "insert",
            "--track",
            "t6",
            "--at",
            "end",
            "--template",
            "a3",
        ],
        3,
        "INVALID_OP",
        "lottieTemplate",
    );
}

#[test]
fn transition_remove_dry_run_and_refuse() {
    assert_eq!(
        dry(&["transition", "remove", "tr1"])["diff"]["changes"][0]["change"],
        "removed"
    );
    refused(&["transition", "remove", "tr9"], 3, "NOT_FOUND", "");
}

#[test]
fn track_and_timeline_commands_dry_run_and_refuse() {
    let add = dry(&["track", "add", "--kind", "caption"]);
    assert_eq!(add["bindings"], json!({ "$track": "t7" }));
    has(
        &add["resolved"],
        json!({ "timeline": "tl1", "kind": "caption", "index": 5, "name": "C1" }),
    );
    let on = dry(&[
        "--timeline",
        "tl2",
        "track",
        "add",
        "--kind",
        "audio",
        "--index",
        "0",
        "--name",
        "Music",
    ]);
    has(
        &on["resolved"],
        json!({ "timeline": "tl2", "index": 0, "name": "Music" }),
    );
    refused(
        &["--timeline", "tl9", "track", "add", "--kind", "video"],
        3,
        "NOT_FOUND",
        "",
    );

    has(
        &dry(&["track", "remove", "t4"])["resolved"],
        json!({ "track": "t4", "withItems": false, "items": 0 }),
    );
    refused(
        &["track", "remove", "t1"],
        3,
        "TRACK_NOT_EMPTY",
        "fin track remove t1 --with-items",
    );
    assert_eq!(
        dry(&["track", "remove", "t1", "--with-items"])["resolved"]["items"],
        2
    );

    assert_eq!(
        dry(&["track", "move", "t4", "--index", "0"])["resolved"]["index"],
        0
    );
    refused(
        &["track", "move", "t4", "--index", "99"],
        3,
        "INVALID_RANGE",
        "",
    );

    assert_eq!(
        dry(&["track", "set", "t1", "gain", "-6dB"])["resolved"]["value"],
        -6.0
    );
    assert_eq!(
        dry(&["track", "set", "t1", "mute", "on"])["resolved"]["value"],
        true
    );
    assert_eq!(
        dry(&["track", "set", "t3", "lock", "--reset"])["resolved"]["value"],
        Value::Null
    );
    refused(
        &["track", "set", "t1", "gain", "loud"],
        3,
        "PARAM_TYPE",
        "e.g.",
    );
    refused(
        &["track", "set", "t1", "gain", "+50dB"],
        3,
        "INVALID_RANGE",
        "",
    );
    let unknown = refused(
        &["track", "set", "t1", "volume", "1"],
        3,
        "PARAM_UNKNOWN",
        "",
    );
    assert!(unknown["details"]["alternatives"].is_array());

    let tl = dry(&[
        "timeline",
        "add",
        "--name",
        "Square",
        "--rate",
        "25",
        "--resolution",
        "1080x1080",
    ]);
    assert_eq!(tl["bindings"], json!({ "$timeline": "tl3" }));
    has(
        &tl["resolved"],
        json!({ "rate": "25", "resolution": { "width": 1080, "height": 1080 } }),
    );
    assert_eq!(
        dry(&["timeline", "add", "--name", "Same"])["resolved"]["rate"],
        "30"
    );
    refused(
        &["timeline", "add", "--name", "Bad", "--rate", "0"],
        3,
        "INVALID_RANGE",
        "",
    );

    assert_eq!(
        dry(&["timeline", "set", "tl1", "name", "Main"])["resolved"]["value"],
        "Main"
    );
    assert_eq!(
        dry(&["timeline", "set", "tl2", "rate", "30000/1001"])["resolved"]["value"],
        "30000/1001"
    );
    refused(
        &["timeline", "set", "tl1", "rate", "25"],
        3,
        "TIMELINE_NOT_EMPTY",
        "",
    );

    assert_eq!(
        dry(&["timeline", "remove", "tl2"])["resolved"],
        json!({ "timeline": "tl2" })
    );
    refused(&["timeline", "remove", "tl9"], 3, "NOT_FOUND", "");
}

#[test]
fn marker_bin_and_asset_commands_dry_run_and_refuse() {
    let m = dry(&[
        "marker", "add", "--at", "f45", "--label", "hit", "--dur", "1s", "--color", "#F00",
    ]);
    assert_eq!(m["bindings"], json!({ "$marker": "m2" }));
    assert_eq!(
        m["resolved"],
        json!({ "timeline": "tl1", "at": "3/2", "duration": "1", "label": "hit", "color": "#ff0000" })
    );
    // Frames count at the timeline's rate.
    assert_eq!(
        dry(&[
            "--timeline",
            "tl2",
            "marker",
            "add",
            "--at",
            "f50",
            "--label",
            "x"
        ])["resolved"]["at"],
        "2"
    );
    refused(
        &[
            "marker", "add", "--at", "1s", "--label", "x", "--color", "red",
        ],
        3,
        "PARAM_TYPE",
        "#ff8800",
    );

    assert_eq!(
        dry(&["marker", "set", "m1", "at", "4s"])["resolved"]["value"],
        "4"
    );
    refused(
        &["marker", "set", "m1", "colour", "#fff"],
        3,
        "PARAM_UNKNOWN",
        "",
    );
    assert_eq!(
        dry(&["marker", "remove", "m1"])["resolved"],
        json!({ "marker": "m1" })
    );
    refused(&["marker", "remove", "m9"], 3, "NOT_FOUND", "");

    assert_eq!(
        dry(&["bin", "add", "--name", "B", "--parent", "b1"])["bindings"],
        json!({ "$bin": "b3" })
    );
    refused(
        &["bin", "add", "--name", "B", "--parent", "b9"],
        3,
        "NOT_FOUND",
        "",
    );
    assert_eq!(
        dry(&["bin", "move", "b2"])["resolved"],
        json!({ "bin": "b2", "parent": null })
    );
    refused(&["bin", "move", "b1", "--parent", "b2"], 3, "BIN_CYCLE", "");
    assert_eq!(
        dry(&["bin", "remove", "b2"])["resolved"],
        json!({ "bin": "b2" })
    );
    refused(&["bin", "remove", "b1"], 3, "BIN_NOT_EMPTY", "fin bin move");

    assert_eq!(
        dry(&["asset", "move", "a2", "--bin", "b1"])["resolved"],
        json!({ "asset": "a2", "bin": "b1" })
    );
    refused(&["asset", "move", "a2", "--bin", "b9"], 3, "NOT_FOUND", "");
    assert_eq!(
        dry(&["asset", "remove", "a5"])["resolved"],
        json!({ "asset": "a5" })
    );
    let used = refused(&["asset", "remove", "a2"], 3, "ASSET_IN_USE", "");
    assert_eq!(used["details"]["items"], json!(["c5"]));
}

#[test]
fn edit_commands_are_in_help_and_schema() {
    for (group, name) in [
        ("item", "item.move"),
        ("transition", "transition.set"),
        ("track", "track.set"),
        ("timeline", "timeline.add"),
        ("marker", "marker.add"),
        ("bin", "bin.move"),
        ("asset", "asset.remove"),
        ("image", "image.insert"),
        ("graphic", "graphic.insert"),
    ] {
        let help = run(&["help", group]);
        assert_eq!(help.code, 0, "{}", help.stdout);
        let names: Vec<&str> = help.json["commands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&name), "{group}: {names:?}");
        let schema = run(&["schema", name]);
        assert_eq!(schema.json["type"], "object", "{name}");
        assert_eq!(schema.json["title"], name);
    }
    let clip = run(&["schema", "clip.insert"]);
    for p in ["mode", "noAudio", "audioTrack"] {
        assert!(clip.json["properties"][p].is_object(), "{p}");
    }
}

/// `fin batch` against the offline snapshot, `lines` on stdin.
fn batch(lines: &str) -> Run {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("snapshot.json");
    std::fs::write(&file, snapshot().to_string()).unwrap();
    let mut cmd = support::fin_cmd(&dir.path().join("no-server"), dir.path(), &["batch"]);
    cmd.env("FIN_OFFLINE_SNAPSHOT", &file)
        .stdin(std::process::Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(lines.as_bytes()).unwrap();
    drop(stdin);
    support::finish(&["batch"], child.wait_with_output().unwrap())
}

/// Review #49: a batch line's mode is one of the values the flag takes; anything else is
/// `USAGE` naming the field and the allowed values, never a silent default.
#[test]
fn batch_modes_are_closed() {
    let ok_line = r#"{"cmd":"clip.delete","args":{"clip":"c2","mode":"lift"}}"#;
    for (bad, allowed) in [
        (
            r#"{"cmd":"clip.delete","args":{"clip":"c1","mode":"liftt"}}"#,
            ["ripple", "lift"].as_slice(),
        ),
        (
            r#"{"cmd":"clip.trim","args":{"clip":"c1","end":"-1s","mode":"rol"}}"#,
            &["ripple", "roll", "overwrite"],
        ),
        (
            r#"{"cmd":"clip.insert","args":{"track":"t1","at":"end","asset":"a2","dur":"1s","mode":"overwrit"}}"#,
            &["ripple", "overwrite"],
        ),
    ] {
        let r = batch(&format!("{ok_line}\n{bad}\n"));
        assert_eq!(r.code, 2, "{bad}: {}", r.stdout);
        let e = &r.json["error"];
        assert_eq!(e["code"], "USAGE", "{bad}: {}", r.stdout);
        assert_eq!(e["details"]["line"], 2, "{bad}: {}", r.stdout);
        let message = e["message"].as_str().unwrap();
        assert!(message.contains("mode"), "{message}");
        for v in allowed {
            assert!(message.contains(v), "{message} lacks {v}");
        }
    }
    // The lowercase spellings the flags take are what a batch line takes.
    let r = batch(ok_line);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_eq!(r.json["resolved"][0]["resolved"]["mode"], "lift");
}

/// Review #49: every edit command's `mode` schema lists its values.
#[test]
fn mode_schemas_list_their_values() {
    for (name, values) in [
        ("clip.insert", json!(["ripple", "overwrite"])),
        ("clip.delete", json!(["ripple", "lift"])),
        ("clip.trim", json!(["ripple", "roll", "overwrite"])),
        ("item.move", json!(["ripple", "overwrite"])),
        ("item.delete", json!(["ripple", "lift"])),
        ("item.trim", json!(["ripple", "roll", "overwrite"])),
        ("image.insert", json!(["ripple", "overwrite"])),
        ("graphic.insert", json!(["ripple", "overwrite"])),
    ] {
        let schema = run(&["schema", name]).json;
        let mut values = values;
        values.as_array_mut().unwrap().push(Value::Null);
        assert_eq!(schema["properties"]["mode"]["enum"], values, "{name}");
    }
}

/// Review #49 follow-up: a track's and a transition's kinds and a transition's curve are closed
/// too.
#[test]
fn kind_and_curve_schemas_list_their_values() {
    let prop = |name: &str, p: &str| run(&["schema", name]).json["properties"][p].clone();
    assert_eq!(
        prop("track.add", "kind")["enum"],
        json!(["video", "audio", "caption"])
    );
    assert_eq!(
        prop("transition.set", "kind")["enum"],
        json!(["dissolve", "crossfade", null])
    );
    assert_eq!(
        prop("transition.set", "curve")["enum"],
        json!(["linear", "equalPower", null])
    );
    // The flags take the same spellings, and the echo is the value applied.
    let set = dry(&[
        "transition",
        "set",
        "--between",
        "c1",
        "c2",
        "--dur",
        "1s",
        "--curve",
        "equalPower",
    ]);
    assert_eq!(set["resolved"]["curve"], "equalPower", "{set}");
    let add = dry(&["track", "add", "--kind", "caption"]);
    assert_eq!(add["resolved"]["kind"], "caption", "{add}");
    refused(&["track", "add", "--kind", "vidoe"], 2, "USAGE", "");
    refused(
        &[
            "transition",
            "set",
            "--between",
            "c1",
            "c2",
            "--dur",
            "1s",
            "--curve",
            "equalpower",
        ],
        2,
        "USAGE",
        "",
    );
}
