//! `fin view` with a token budget and the fuller `fin status` (plan 4C Task 25). The offline
//! goldens read `view_golden/project.json` (two timelines; clips with speed, links, effects,
//! keyframes; an image, a graphic, a nested item, an adjustment; captions, one with no pin
//! for its face; markers; a transition); `FIN_VIEW_BLESS=1` rewrites `full.txt` and
//! `budget_200.txt` (refused under `CI`). The online tests run a test-hooks finisherd with
//! `FIN_FONTS_OFFLINE=1`, so planned pins stay missing.

mod support;

use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::{Value, json};
use support::fontserver::{self, CatalogFile, FontServer};
use support::{Server, assert_error, fin, offline, ok};

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/view_golden")
}

fn project() -> Value {
    let text = std::fs::read_to_string(golden_dir().join("project.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn view(args: &[&str]) -> Value {
    let mut all = vec!["view"];
    all.extend_from_slice(args);
    let r = offline(&project(), &all);
    assert_eq!(r.code, 0, "{}", r.stdout);
    r.json
}

fn lines(v: &Value) -> Vec<String> {
    v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

fn text(v: &Value) -> String {
    lines(v).join("\n") + "\n"
}

fn golden(name: &str, actual: &str) {
    let path = golden_dir().join(name);
    if std::env::var_os("FIN_VIEW_BLESS").is_some() {
        assert!(
            std::env::var_os("CI").is_none(),
            "FIN_VIEW_BLESS is refused under CI"
        );
        std::fs::write(&path, actual).unwrap();
    }
    let expected = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, expected, "{name} (FIN_VIEW_BLESS=1 rewrites it)");
}

fn steps(v: &Value) -> Vec<String> {
    v["elided"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["step"].as_str().unwrap().to_string())
        .collect()
}

/// `ceil(chars / 4)`, each line with its newline.
fn estimate(v: &Value) -> u64 {
    let chars: usize = lines(v).iter().map(|l| l.chars().count() + 1).sum();
    (chars as u64).div_ceil(4)
}

#[test]
fn view_full_matches_golden() {
    let v = view(&[]);
    golden("full.txt", &text(&v));
    assert_eq!(v["rev"], 12);
    assert_eq!(v["timeline"], Value::Null, "every timeline");
    assert_eq!(v["elided"], json!([]));
    assert_eq!(v["tokens"], estimate(&v));
    let l = lines(&v);
    assert!(l[0].starts_with("== tl1 \"Main\" 1080x1920 30fps 00:10.0"));
    assert!(
        l.iter()
            .any(|x| x.starts_with("== tl2 \"Alt\" 1920x1080 29.97fps"))
    );
    for want in [
        "00:04.0–00:07.5  V1  c2 clip a1 [2.0s+5.25s] x1.5 opacity 0.8 fx: blur  ⟷ lk1",
        "00:06.0  ──  marker m1 \"drop\"",
        "00:07.0–00:08.0  V1  tr1 dissolve c2→c3 1.0s",
        "00:00.0–00:02.0  C1  cap1 \"Hello world\" (st1)",
    ] {
        assert!(l.iter().any(|x| x == want), "{want}\n{}", text(&v));
    }
}

#[test]
fn view_budget_degrades_in_order_and_says_so() {
    let full = view(&[]);
    let at = |b: u64| view(&["--budget", &b.to_string()]);
    // A budget the full view fits in changes nothing.
    let roomy = at(full["tokens"].as_u64().unwrap());
    assert_eq!(roomy["lines"], full["lines"]);
    assert_eq!(roomy["elided"], json!([]));
    let b200 = at(200);
    golden("budget_200.txt", &text(&b200));
    assert_eq!(b200["elided"], json!([{ "step": "detail", "items": 11 }]));
    let order = ["detail", "text", "merge", "tail"];
    let mut last = 0;
    for b in [200, 150, 120, 90, 60, 30] {
        let v = at(b);
        let s = steps(&v);
        // The steps run in the fixed order (tail may repeat, one per timeline).
        let idx: Vec<usize> = s
            .iter()
            .map(|x| order.iter().position(|o| o == x).unwrap())
            .collect();
        assert!(idx.windows(2).all(|w| w[0] <= w[1]), "{b}: {s:?}");
        assert!(s.len() >= last, "more steps as the budget shrinks: {b}");
        last = s.len();
        assert_eq!(v["tokens"], estimate(&v), "{b}");
        let headers = lines(&v).iter().filter(|l| l.starts_with("== ")).count();
        assert_eq!(headers, 2, "headers stay: {b}");
        if b >= 60 {
            assert!(v["tokens"].as_u64().unwrap() <= b, "{b}: {}", v["tokens"]);
        }
        assert_eq!(v["resolved"]["budget"], b);
    }
    // Merging folds runs of a kind on a track; the tail cut names its range per timeline.
    let v = at(60);
    assert!(
        lines(&v).iter().any(|l| l.contains("c1…c3 3 clips")),
        "{}",
        text(&v)
    );
    let tails: Vec<&Value> = v["elided"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["step"] == "tail")
        .collect();
    assert!(!tails.is_empty());
    for t in tails {
        assert!(t["timeline"].is_string() && t["from"].is_string() && t["to"].is_string());
        assert!(t["items"].as_u64().unwrap() > 0);
    }
    // Just past the detail step, the text step shortens long caption text, and stops there.
    let v = at(188);
    assert_eq!(steps(&v), ["detail", "text"]);
    assert_eq!(v["elided"][1], json!({ "step": "text", "items": 1 }));
    assert!(
        lines(&v)
            .iter()
            .any(|l| l.ends_with("cap3 \"This caption has…\" (st1)")),
        "{}",
        text(&v)
    );
}

#[test]
fn view_range_and_timeline() {
    let v = view(&["--range", "6s..7.5s"]);
    let l = lines(&v);
    assert_eq!(v["resolved"]["range"], json!({ "from": "6", "to": "15/2" }));
    let ids: Vec<&str> = l
        .iter()
        .filter(|x| !x.starts_with("== "))
        .map(|x| x.split("  ").nth(2).unwrap().split(' ').next().unwrap())
        .collect();
    // tl1: c2, c5 and g1 overlap, cap2 ends at 6.2, m1 at 6, cap3 and tr1 start at 7; c3 starts
    // at 7.5 (out). tl2 ends before 6.
    assert_eq!(
        ids,
        ["c2", "c5", "g1", "cap2", "marker", "cap3", "tr1"],
        "{}",
        text(&v)
    );
    let v = view(&["--timeline", "tl2"]);
    assert_eq!(v["timeline"], "tl2");
    let l = lines(&v);
    assert!(l[0].starts_with("== tl2 ") && l.len() == 3, "{l:?}");
    let v = view(&["--track", "t4"]);
    let l = lines(&v);
    assert_eq!(l.len(), 4, "the header and three captions: {l:?}");
    assert!(l[1..].iter().all(|x| x.contains("  C1  cap")));
    assert_error(
        &offline(&project(), &["view", "--timeline", "tl9"]),
        3,
        "NOT_FOUND",
    );
    assert_error(
        &offline(&project(), &["view", "--track", "t9"]),
        3,
        "NOT_FOUND",
    );
    assert_error(
        &offline(&project(), &["view", "--timeline", "tl2", "--track", "t1"]),
        3,
        "NOT_FOUND",
    );
}

#[test]
fn view_is_deterministic() {
    for args in [&[][..], &["--budget", "90"][..]] {
        let a = view(args);
        let b = view(args);
        assert_eq!(a, b);
    }
}

#[test]
fn view_marks_unmeasured_captions() {
    let l = lines(&view(&[]));
    let cap = |id: &str| {
        l.iter()
            .find(|x| x.contains(&format!("  C1  {id} ")))
            .unwrap()
            .clone()
    };
    assert!(
        cap("cap2").ends_with("(preset:bold) (unmeasured: font unpinned)"),
        "{}",
        cap("cap2")
    );
    // Offline there are no file states: a pinned face is not marked.
    assert!(!cap("cap1").contains("unmeasured"));
    assert!(!cap("cap3").contains("unmeasured"));
}

#[test]
fn status_offline() {
    let r = offline(&project(), &["status"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    let s = r.json;
    assert_eq!(s["project"], json!({ "id": "p1", "name": "Reel" }));
    assert_eq!(s["branch"], "main");
    assert_eq!(s["head"], 12);
    assert_eq!(s["server"], false);
    let tls = s["timelines"].as_array().unwrap();
    assert_eq!(tls.len(), 2, "every timeline");
    assert_eq!(tls[1]["id"], "tl2");
    assert_eq!(tls[1]["rate"], "30000/1001");
    assert_eq!(
        tls[1]["tracks"][0],
        json!({ "id": "t5", "name": "V1", "kind": "video", "items": 2, "end": "2001/1000" })
    );
    assert_eq!(tls[0]["end"], "10");
    assert_eq!(tls[0]["markers"], 2);
    assert_eq!(
        s["fonts"]["unmeasured"],
        json!([{ "item": "cap2", "reason": "font unpinned" }])
    );
}

/// A test-hooks finisherd on a one-entry catalog (Test Sans), with fetching switched off.
fn fonts_offline_server(test: &str) -> Option<(Server, tempfile::TempDir)> {
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
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.json");
    std::fs::write(&path, catalog.to_string()).unwrap();
    let mut env = fontserver::env(&path);
    env.push(("FIN_FONTS_OFFLINE".into(), OsString::from("1")));
    let server = Server::start_with(test, env, fontserver::why_no_test_hooks())?;
    Some((server, dir))
}

#[test]
fn status_online_fields() {
    let Some((server, _catalog)) = fonts_offline_server("status_online_fields") else {
        return;
    };
    let (sd, root) = (&server.sd, &server.root);
    ok(fin(
        sd,
        root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");
    let run = |args: &[&str]| ok(fin(sd, &dir, args));
    run(&["track", "add", "--kind", "caption"]);
    run(&[
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
    std::fs::write(
        dir.join("w.json"),
        json!([{ "text": "Hello", "start": 0, "end": 1 }]).to_string(),
    )
    .unwrap();
    let import = run(&[
        "caption", "import", "--track", "t3", "--words", "w.json", "--style", "st1",
    ]);
    assert_eq!(
        import["resolved"]["fonts"]["planned"][0]["key"],
        "Test Sans|400|normal"
    );
    // preset:bold's face (Inter) is not in the test catalog: nothing pins it.
    run(&[
        "caption",
        "import",
        "--track",
        "t3",
        "--words",
        "w.json",
        "--style",
        "preset:bold",
        "--at",
        "5s",
    ]);
    run(&["branch", "new", "side"]);

    // The font summary follows the commit; wait for it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let status = loop {
        let s = run(&["status"]);
        let codes: Vec<&str> = s["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|w| w["code"].as_str())
            .collect();
        if codes.contains(&"FONT_MISSING") && codes.contains(&"FONT_UNPINNED") {
            break s;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no font warnings: {s}"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    assert_eq!(status["project"]["name"], "Film");
    assert_eq!(status["branch"], "main");
    assert_eq!(status["head"], run(&["view"])["rev"], "{status}");
    assert_eq!(status["lease"]["state"], "held");
    assert_eq!(status["readOnly"], Value::Null);
    assert!(status.get("lastExport").is_some());
    let unmerged = status["unmergedBranches"].as_array().unwrap();
    assert_eq!(unmerged.len(), 1, "{status}");
    assert_eq!(unmerged[0]["name"], "side");
    assert!(unmerged[0]["id"].is_string());
    assert_eq!(status["timelines"][0]["tracks"][2]["items"], 2);
    let missing = status["fonts"]["missing"].as_array().unwrap();
    assert!(
        missing.iter().any(|m| m["key"] == "Test Sans|400|normal"),
        "{status}"
    );
    assert_eq!(
        status["fonts"]["unmeasured"],
        json!([{ "item": "cap1", "reason": "font missing" },
               { "item": "cap2", "reason": "font unpinned" }])
    );
    for w in status["warnings"].as_array().unwrap() {
        assert!(w["fix"].as_str().is_some_and(|f| !f.is_empty()), "{w}");
    }

    // The same marks in the view.
    let v = run(&["view", "--track", "t3"]);
    let l = lines(&v);
    assert!(l[1].ends_with("(st1) (unmeasured: font missing)"), "{l:?}");
    assert!(
        l[2].ends_with("(preset:bold) (unmeasured: font unpinned)"),
        "{l:?}"
    );
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
