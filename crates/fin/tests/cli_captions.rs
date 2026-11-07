//! `fin caption import|set-text|split|merge|style` (plan 4C Task 24): offline dry runs against
//! the shared snapshot (`support::settings`), and online imports through a test-hooks
//! finisherd whose catalog is served by a loopback font server: pins planned once per tx, and
//! a block that does not fit refused (`CAPTION_OVERFLOW`) once its font is cached.

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::fontserver::{self, CatalogFile, FontServer};
use support::settings::snapshot;
use support::{Run, fin, offline, ok};

fn run_in(dir: &Path, args: &[&str]) -> Run {
    let file = dir.join("snapshot.json");
    std::fs::write(&file, snapshot().to_string()).unwrap();
    let mut cmd = support::fin_cmd(&dir.join("no-server"), dir, args);
    cmd.env("FIN_OFFLINE_SNAPSHOT", &file);
    support::finish(args, cmd.output().unwrap())
}

fn dry(args: &[&str]) -> Value {
    let r = offline(&snapshot(), args);
    assert_eq!(r.code, 0, "{args:?}: {}", r.stdout);
    r.json
}

fn refused_in(dir: &Path, args: &[&str], exit: i32, code: &str) -> Value {
    let r = run_in(dir, args);
    assert_eq!(r.code, exit, "{args:?}: {}", r.stdout);
    assert_eq!(r.json["error"]["code"], code, "{args:?}: {}", r.stdout);
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .is_some_and(|f| !f.is_empty()),
        "{}",
        r.stdout
    );
    r.json["error"].clone()
}

/// Two groups of words 3 s apart: two blocks in every preset.
fn words_file(dir: &Path) {
    let words = json!([
        { "text": "Hello", "start": 0, "end": 0.5 },
        { "text": "there", "start": "1/2", "end": "1.25" },
        { "text": "again", "start": 4.25, "end": "5" },
    ]);
    std::fs::write(dir.join("w.json"), words.to_string()).unwrap();
}

#[test]
fn caption_import_words_and_srt_make_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    words_file(dir);
    let r = run_in(
        dir,
        &[
            "caption",
            "import",
            "--track",
            "t2",
            "--words",
            "w.json",
            "--style",
            "preset:bold",
            "--at",
            "10s",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_eq!(
        r.json["bindings"],
        json!({ "$cap1": "cap3", "$cap2": "cap4" })
    );
    let resolved = &r.json["resolved"];
    assert_eq!(resolved["items"], json!(["$cap1", "$cap2"]));
    assert_eq!(resolved["at"], "10");
    assert_eq!(resolved["words"], 3);
    assert_eq!(resolved["blocks"], 2);
    assert_eq!(resolved["mode"], "overwrite");
    assert_eq!(
        resolved["fonts"],
        json!({ "planned": [], "measured": false })
    );
    assert!(resolved.get("warnings").is_none());

    // SubRip: an overlap trimmed, a tag-only cue and a sub-frame cue dropped, all echoed.
    let srt = "1\n00:00:01,000 --> 00:00:03,000\n<i>First</i> cue\n\n\
               2\n00:00:02,500 --> 00:00:04,000\nSecond cue\n\n\
               3\n00:00:06,000 --> 00:00:07,000\n<b></b>\n\n\
               4\n00:00:08,000 --> 00:00:08,010\nblink\n";
    std::fs::write(dir.join("s.srt"), srt).unwrap();
    let r = run_in(
        dir,
        &[
            "caption", "import", "--track", "t2", "--srt", "s.srt", "--style", "st1", "--at", "10s",
        ],
    );
    assert_eq!(r.code, 0, "{}", r.stdout);
    let codes: Vec<&str> = r.json["resolved"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w["code"].as_str().unwrap())
        .collect();
    assert_eq!(
        codes,
        ["SRT_EMPTY_CUE", "SRT_OVERLAP_TRIMMED", "SRT_SHORT_CUE"]
    );
    assert_eq!(r.json["resolved"]["words"], 4);
    assert!(r.json["resolved"]["blocks"].as_u64().unwrap() >= 1);

    // Refusals.
    std::fs::write(dir.join("bad.srt"), "1\n00:00:01 --> soon\nx\n").unwrap();
    let e = refused_in(
        dir,
        &[
            "caption", "import", "--track", "t2", "--srt", "bad.srt", "--style", "st1",
        ],
        3,
        "SRT_INVALID",
    );
    assert_eq!(e["details"]["line"], 2);
    std::fs::write(
        dir.join("latin1.srt"),
        b"1\n00:00:01,000 --> 00:00:02,000\ncaf\xe9\n",
    )
    .unwrap();
    refused_in(
        dir,
        &[
            "caption",
            "import",
            "--track",
            "t2",
            "--srt",
            "latin1.srt",
            "--style",
            "st1",
        ],
        3,
        "SRT_INVALID",
    );
    std::fs::write(dir.join("none.json"), "[]").unwrap();
    refused_in(
        dir,
        &[
            "caption",
            "import",
            "--track",
            "t2",
            "--words",
            "none.json",
            "--style",
            "st1",
        ],
        2,
        "USAGE",
    );
    std::fs::write(
        dir.join("odd.json"),
        r#"[{"text":"x","start":"soon","end":1}]"#,
    )
    .unwrap();
    refused_in(
        dir,
        &[
            "caption", "import", "--track", "t2", "--words", "odd.json", "--style", "st1",
        ],
        2,
        "USAGE",
    );
    refused_in(
        dir,
        &[
            "caption", "import", "--track", "t1", "--words", "w.json", "--style", "st1",
        ],
        3,
        "TRACK_KIND_MISMATCH",
    );
    refused_in(
        dir,
        &[
            "caption", "import", "--track", "t2", "--words", "w.json", "--style", "st9",
        ],
        3,
        "NOT_FOUND",
    );
}

#[test]
fn caption_edits_dry_run() {
    let doc = dry(&["caption", "set-text", "cap1", "Hi there you"]);
    assert_eq!(
        doc["resolved"],
        json!({ "item": "cap1", "text": "Hi there you", "words": 3,
                "fonts": { "planned": [], "measured": false } })
    );
    let doc = dry(&["caption", "split", "cap1", "--word", "1"]);
    assert_eq!(doc["bindings"], json!({ "$cap": "cap3" }));
    let doc = dry(&["caption", "merge", "cap1", "cap2"]);
    assert_eq!(doc["resolved"]["words"], 4);
    let doc = dry(&["caption", "style", "cap2", "st1", "--set", "text.size=80px"]);
    assert_eq!(doc["resolved"]["overrides"], json!({ "text.size": 80.0 }));
    let doc = dry(&["caption", "style", "cap2", "preset:clean", "--clear"]);
    assert_eq!(doc["resolved"]["overrides"], json!({}));

    let tmp = tempfile::tempdir().unwrap();
    refused_in(
        tmp.path(),
        &["caption", "split", "cap1", "--word", "0"],
        3,
        "INVALID_RANGE",
    );
    refused_in(
        tmp.path(),
        &["caption", "style", "cap1", "st9"],
        3,
        "NOT_FOUND",
    );
    refused_in(
        tmp.path(),
        &[
            "caption",
            "style",
            "cap1",
            "st1",
            "--set",
            "text.size=2000px",
        ],
        3,
        "INVALID_RANGE",
    );
}

/// A test-hooks server whose catalog has `Test Sans` (TestSans bytes) as entry `sans`, a
/// project `film` with a caption track t3, and style st1 in Test Sans 400 (`extra` sets).
fn caption_project(
    test: &str,
    fonts: &FontServer,
) -> Option<(fontserver::Fonts, std::path::PathBuf)> {
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
    let f = fontserver::start(test, &catalog)?;
    let (sd, root) = (&f.server.sd, &f.server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    ok(fin(sd, &dir, &["track", "add", "--kind", "caption"]));
    ok(fin(
        sd,
        &dir,
        &[
            "style",
            "add",
            "--name",
            "S",
            "--set",
            "text.family=Test Sans",
            "--set",
            "text.weight=400",
        ],
    ));
    Some((f, dir))
}

#[test]
fn caption_import_pins_fonts_once() {
    let fonts = FontServer::start();
    let Some((f, dir)) = caption_project("caption_import_pins_fonts_once", &fonts) else {
        return;
    };
    let sd = &f.server.sd;
    words_file(&dir);
    let first = ok(fin(
        sd,
        &dir,
        &[
            "caption", "import", "--track", "t3", "--words", "w.json", "--style", "st1",
        ],
    ));
    assert_eq!(first["bindings"]["$cap1"], "cap1");
    assert_eq!(first["bindings"]["$cap2"], "cap2");
    let planned = first["resolved"]["fonts"]["planned"].as_array().unwrap();
    assert_eq!(planned.len(), 1, "{planned:?}");
    assert_eq!(planned[0]["op"], "font.pin");
    assert_eq!(planned[0]["key"], "Test Sans|400|normal");
    assert_eq!(planned[0]["source"]["entry"], "sans");
    let pin_changes = first["diff"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "font")
        .count();
    assert_eq!(pin_changes, 1, "{}", first["diff"]);
    // Pinned now: the next import plans nothing.
    let second = ok(fin(
        sd,
        &dir,
        &[
            "caption", "import", "--track", "t3", "--words", "w.json", "--style", "st1", "--at",
            "20s",
        ],
    ));
    assert_eq!(second["resolved"]["fonts"]["planned"], json!([]));
    let pins = ok(fin(sd, &dir, &["fonts", "ls", "--pinned"]));
    assert_eq!(pins["pins"][0]["key"], "Test Sans|400|normal");
    assert_eq!(
        pins["pins"][0]["captions"],
        json!(["cap1", "cap2", "cap3", "cap4"])
    );
}

#[test]
fn caption_import_overflow_refused_online() {
    let fonts = FontServer::start();
    let Some((f, dir)) = caption_project("caption_import_overflow_refused_online", &fonts) else {
        return;
    };
    let sd = &f.server.sd;
    // Cached first, so the commit measures.
    ok(fin(sd, &dir, &["fonts", "fetch", "sans"]));
    ok(fin(
        sd,
        &dir,
        &["style", "set", "st1", "text.size", "600px"],
    ));
    ok(fin(sd, &dir, &["style", "set", "st1", "maxLines", "1"]));
    let words = json!([{ "text": "Unbreakablewordthatcannotfit", "start": 0, "end": 2 }]);
    std::fs::write(dir.join("long.json"), words.to_string()).unwrap();
    let r = fin(
        sd,
        &dir,
        &[
            "caption",
            "import",
            "--track",
            "t3",
            "--words",
            "long.json",
            "--style",
            "st1",
        ],
    );
    support::assert_error(&r, 3, "CAPTION_OVERFLOW");
    // The same words fit at the default size.
    ok(fin(
        sd,
        &dir,
        &["style", "set", "st1", "text.size", "--reset"],
    ));
    ok(fin(sd, &dir, &["style", "set", "st1", "maxChars", "40"]));
    let words = json!([{ "text": "Short", "start": 0, "end": 2 }]);
    std::fs::write(dir.join("short.json"), words.to_string()).unwrap();
    ok(fin(
        sd,
        &dir,
        &[
            "caption",
            "import",
            "--track",
            "t3",
            "--words",
            "short.json",
            "--style",
            "st1",
        ],
    ));
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
