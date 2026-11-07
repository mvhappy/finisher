//! `fin param|keyframe|effect|style|schema param|eval` (plan 4C Task 24) against the
//! `FIN_OFFLINE_SNAPSHOT` hook: dry-run documents, values typed by the registry, refusals with
//! their exit codes and fixes, and `eval` needing a server. The captions and fonts commands are
//! in `cli_captions.rs` and `cli_fonts.rs`; live commits of these in `cli_settings_online.rs`.

mod support;

use serde_json::{Value, json};
use support::settings::snapshot;
use support::{Run, offline};

fn run(args: &[&str]) -> Run {
    offline(&snapshot(), args)
}

fn dry(args: &[&str]) -> Value {
    let r = run(args);
    assert_eq!(r.code, 0, "{args:?}: {}", r.stdout);
    assert_eq!(r.json["dryRun"], true, "{args:?}");
    r.json
}

fn refused(args: &[&str], exit: i32, code: &str, fix: &str) -> Value {
    let r = run(args);
    assert_eq!(r.code, exit, "{args:?}: {}", r.stdout);
    assert_eq!(r.json["error"]["code"], code, "{args:?}: {}", r.stdout);
    let f = r.json["error"]["fix"].as_str().unwrap_or_default();
    assert!(f.contains(fix), "{args:?}: fix {f:?} lacks {fix:?}");
    r.json["error"].clone()
}

/// The ops a dry run's diff came from are not echoed, so tests read the changes.
fn changes(doc: &Value) -> &Vec<Value> {
    doc["diff"]["changes"].as_array().unwrap()
}

#[test]
fn param_set_types_values_by_the_registry() {
    let doc = dry(&["param", "set", "g1", "opacity", "50%"]);
    assert_eq!(
        doc["resolved"],
        json!({ "item": "g1", "path": "opacity", "value": 0.5, "unit": null })
    );
    assert_eq!(changes(&doc)[0]["path"], "opacity");
    let doc = dry(&["param", "set", "c1", "transform.rotation", "45deg"]);
    assert_eq!(doc["resolved"]["value"], 45.0);
    assert_eq!(doc["resolved"]["unit"], "deg");
    let doc = dry(&["param", "set", "c1", "transform.position", "0.25,0.75"]);
    assert_eq!(doc["resolved"]["value"], json!([0.25, 0.75]));
    let doc = dry(&["param", "set", "c1", "effects.fx1.radius", "12px"]);
    assert_eq!(doc["resolved"]["value"], 12.0);
    let doc = dry(&["param", "set", "g1", "slots.title", "Hi there"]);
    assert_eq!(doc["resolved"]["value"], "Hi there");
    let doc = dry(&["param", "set", "c1", "blend", "--reset"]);
    assert_eq!(doc["resolved"]["value"], Value::Null);
}

#[test]
fn bad_value_errors_name_the_spec_range() {
    let e = refused(
        &["param", "set", "g1", "opacity", "2"],
        3,
        "INVALID_RANGE",
        "opacity",
    );
    assert_eq!(e["details"]["path"], "opacity");
    assert_eq!(e["details"]["min"], 0.0);
    assert_eq!(e["details"]["max"], 1.0);
    let e = refused(
        &["param", "set", "g1", "opacity", "lots"],
        3,
        "PARAM_TYPE",
        "e.g.",
    );
    assert_eq!(e["details"]["path"], "opacity");
    let e = refused(
        &["param", "set", "c1", "opacty", "1"],
        3,
        "PARAM_UNKNOWN",
        "",
    );
    assert!(
        e["details"]["alternatives"]
            .as_array()
            .unwrap()
            .contains(&json!("opacity"))
    );
    refused(
        &["param", "set", "g1", "slots.subtitle", "x"],
        3,
        "SLOT_UNKNOWN",
        "",
    );
    refused(
        &["param", "set", "c9", "opacity", "1"],
        3,
        "NOT_FOUND",
        "fin view",
    );
    refused(&["param", "set", "c1", "opacity"], 2, "USAGE", "usage");
}

#[test]
fn keyframe_set_and_remove() {
    let doc = dry(&[
        "keyframe", "set", "c1", "opacity", "--at", "f15", "0.5", "--interp", "easeOut",
    ]);
    assert_eq!(
        doc["resolved"],
        json!({ "item": "c1", "path": "opacity", "at": "1/2", "value": 0.5,
                "interp": { "type": "bezier", "x1": 0.0, "y1": 0.0, "x2": 0.58, "y2": 1.0 } })
    );
    let doc = dry(&[
        "keyframe",
        "set",
        "c1",
        "transform.scale",
        "--at",
        "0",
        "2,2",
    ]);
    assert_eq!(doc["resolved"]["interp"], json!({ "type": "linear" }));
    let doc = dry(&["keyframe", "remove", "c1", "opacity", "--at", "1s"]);
    assert_eq!(doc["resolved"]["at"], "1");
    refused(
        &["keyframe", "set", "c1", "blend", "--at", "0", "multiply"],
        3,
        "PARAM_NOT_ANIMATABLE",
        "",
    );
    refused(
        &[
            "keyframe", "set", "c1", "opacity", "--at", "0", "1", "--interp", "wobbly",
        ],
        2,
        "USAGE",
        "easeInOut",
    );
    // Static set of an animated path.
    refused(
        &["param", "set", "c1", "opacity", "1"],
        3,
        "PARAM_ANIMATED",
        "",
    );
}

#[test]
fn effect_add_remove_move() {
    let doc = dry(&[
        "effect",
        "add",
        "c1",
        "blur",
        "--set",
        "radius=8px",
        "--at",
        "0",
    ]);
    assert_eq!(doc["bindings"], json!({ "$fx": "fx3" }));
    assert_eq!(
        doc["resolved"],
        json!({ "item": "c1", "kind": "blur", "effect": "$fx", "at": 0, "params": { "radius": 8.0 } })
    );
    let doc = dry(&["effect", "move", "c1", "fx2", "--to", "0"]);
    assert_eq!(
        doc["resolved"],
        json!({ "item": "c1", "effect": "fx2", "to": 0 })
    );
    let doc = dry(&["effect", "remove", "c1", "fx1"]);
    assert_eq!(doc["resolved"], json!({ "item": "c1", "effect": "fx1" }));
    refused(
        &["effect", "add", "c1", "glow"],
        3,
        "PARAM_UNKNOWN",
        "colorAdjust",
    );
    refused(
        &["effect", "add", "c1", "blur", "--set", "radius=-1"],
        3,
        "INVALID_RANGE",
        "radius",
    );
    refused(
        &["effect", "add", "c1", "blur", "--set", "radius"],
        2,
        "USAGE",
        "--set",
    );
    refused(&["effect", "remove", "c1", "fx9"], 3, "NOT_FOUND", "");
}

#[test]
fn style_add_set_remove_ls() {
    let doc = dry(&[
        "style",
        "add",
        "--name",
        "Big",
        "--from",
        "preset:bold",
        "--set",
        "text.size=96px",
    ]);
    assert_eq!(doc["bindings"], json!({ "$style": "st2" }));
    assert_eq!(doc["resolved"]["set"], json!({ "text.size": 96.0 }));
    assert_eq!(doc["resolved"]["font"], "Inter|700|normal");
    // Offline: no pins are planned, and the echo says so.
    assert_eq!(
        doc["resolved"]["fonts"],
        json!({ "planned": [], "measured": false })
    );
    let doc = dry(&["style", "set", "st1", "color", "#FFF"]);
    assert_eq!(doc["resolved"]["value"], "#ffffff");
    assert_eq!(doc["resolved"]["font"], "TestSans|400|normal");
    refused(&["style", "remove", "st1"], 3, "STYLE_IN_USE", "");
    refused(
        &["style", "set", "st1", "text.size", "huge"],
        3,
        "PARAM_TYPE",
        "e.g.",
    );
    let ls = run(&["style", "ls"]);
    assert_eq!(ls.code, 0, "{}", ls.stdout);
    let styles = ls.json["styles"].as_array().unwrap();
    assert_eq!(styles.len(), 1);
    assert_eq!(
        styles[0],
        json!({ "id": "st1", "name": "Mine", "font": "TestSans|400|normal", "pinned": true,
                "size": styles[0]["size"], "safeZone": styles[0]["safeZone"],
                "maxChars": styles[0]["maxChars"], "maxLines": styles[0]["maxLines"],
                "captions": ["cap1"] })
    );
    let presets: Vec<&Value> = ls.json["presets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| &p["id"])
        .collect();
    assert!(presets.contains(&&json!("preset:bold")), "{presets:?}");
}

#[test]
fn schema_param_lists_the_registry() {
    let all = run(&["schema", "param"]);
    assert_eq!(all.code, 0, "{}", all.stdout);
    let count = all.json["count"].as_u64().unwrap() as usize;
    assert_eq!(count, fin_core::params::specs().len());
    assert_eq!(all.json["params"].as_array().unwrap().len(), count);
    let style = run(&["schema", "param", "--scope", "style", "--path", "text."]);
    let paths: Vec<&str> = style.json["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["path"].as_str().unwrap())
        .collect();
    assert!(paths.contains(&"text.family"), "{paths:?}");
    assert!(paths.iter().all(|p| p.starts_with("text.")), "{paths:?}");
    let blur = run(&["schema", "param", "--scope", "blur"]);
    assert!(
        blur.json["params"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["path"] == "blur.radius" && p["unit"] == "px")
    );
    let captions = run(&["schema", "param", "--scope", "caption"]);
    assert!(captions.json["count"].as_u64().unwrap() > 0);
    let bad = run(&["schema", "param", "--scope", "planet"]);
    assert_eq!(bad.code, 2);
    assert_eq!(bad.json["error"]["code"], "USAGE");
    // The command schema of param.set is still `fin schema param set`.
    let set = run(&["schema", "param", "set"]);
    assert_eq!(set.json["title"], "param.set");
    assert_eq!(run(&["schema", "param.set", "--scope", "style"]).code, 2);
}

#[test]
fn eval_needs_a_server() {
    for args in [
        &["eval", "frame", "--at", "1s"][..],
        &["eval", "audio", "--range", "0..2s"][..],
    ] {
        let e = refused(args, 5, "SERVER_UNAVAILABLE", "finisherd");
        assert_eq!(e["details"]["mode"], "offline");
    }
}
