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
    view_of(&project(), args)
}

/// `fin view` of `p`; checks that `responseTokens` is `ceil(chars / 4)` of what was printed
/// (the document and its newline) and `lineTokens` that of the lines (review #43).
fn view_of(p: &Value, args: &[&str]) -> Value {
    let mut all = vec!["view"];
    all.extend_from_slice(args);
    let r = offline(p, &all);
    assert_eq!(r.code, 0, "{}", r.stdout);
    let printed = (r.stdout.chars().count() as u64).div_ceil(4);
    assert_eq!(r.json["responseTokens"], printed, "{}", r.stdout);
    assert_eq!(r.json["lineTokens"], estimate(&r.json), "{}", r.stdout);
    assert!(r.json.get("tokens").is_none());
    r.json
}

fn response(v: &Value) -> u64 {
    v["responseTokens"].as_u64().unwrap()
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
    let l = lines(&v);
    assert!(l[0].starts_with("== tl1 \"Main\" 1080x1920 30fps 00:10.0"));
    assert!(
        l.iter()
            .any(|x| x.starts_with("== tl2 \"Alt\" 1920x1080 29.97fps"))
    );
    for want in [
        "00:04.0–00:07.5  V1  c2 clip a1 [2.0s+5.25s] x1.5 opacity 0.8 fx: blur=fx1  ⟷ lk1",
        "00:06.0  ──  marker m1 \"drop\"",
        "00:07.0–00:08.0  V1  tr1 dissolve c2→c3 1.0s",
        "00:00.0–00:02.0  C1  cap1 \"Hello world\" (st1)",
    ] {
        assert!(l.iter().any(|x| x == want), "{want}\n{}", text(&v));
    }
}

/// `--detail` lists each animated path's keys as `<key>@<at>` (item-relative) and each effect's
/// stored params (review batch L); every other line, and the lines without it, are as before
/// (spec §5.12.3).
#[test]
fn view_detail_lists_keyframe_ids_and_times() {
    let plain = view(&[]);
    let v = view(&["--detail"]);
    assert_eq!(v["resolved"], json!({ "detail": true }));
    let c3 = "00:07.5–00:10.0  V1  c3 clip a1 [10.0s+2.5s] rot 90";
    let c2 = "00:04.0–00:07.5  V1  c2 clip a1 [2.0s+5.25s] x1.5 opacity 0.8 fx: blur=fx1";
    for (a, b) in lines(&plain).iter().zip(lines(&v).iter()) {
        if a.starts_with(c3) {
            assert_eq!(a, &format!("{c3} kf: opacity"));
            assert_eq!(b, &format!("{c3} kf: opacity[kf1@0.0s,kf2@1.0s]"));
        } else if a.starts_with(c2) {
            assert_eq!(a, &format!("{c2}  ⟷ lk1"));
            assert_eq!(b, &format!("{c2}{{radius=4}}  ⟷ lk1"));
        } else {
            assert_eq!(a, b);
        }
    }
    assert_eq!(lines(&plain).len(), lines(&v).len());

    // Two paths: sorted by path, separated by a space.
    let mut p = project();
    let tracks = p["project"]["timelines"][0]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap();
    let c3_item = tracks.iter_mut().find(|i| i["id"] == "c3").unwrap();
    c3_item["keyframes"]["transform.rotation"] = json!([
        { "at": "1/2", "interp": { "type": "hold" }, "value": 45.0 }
    ]);
    let r = offline(&p, &["view", "--detail", "--track", "t1"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    let l = lines(&r.json);
    let line = l.iter().find(|x| x.contains(" c3 ")).unwrap();
    assert!(
        line.ends_with("kf: opacity[kf1@0.0s,kf2@1.0s] transform.rotation[kf3@0.5s]"),
        "{line}"
    );
}

#[test]
fn view_budget_degrades_in_order_and_says_so() {
    let full = view(&[]);
    let at = |b: u64| view(&["--budget", &b.to_string()]);
    // A budget the full view fits in changes nothing.
    let roomy = at(response(&full) + 10);
    assert_eq!(roomy["lines"], full["lines"]);
    assert_eq!(roomy["elided"], json!([]));
    let b200 = at(200);
    golden("budget_200.txt", &text(&b200));
    // `items` counts the lines shown: details of the clips the merge folded are not counted.
    assert_eq!(
        b200["elided"],
        json!([{ "step": "detail", "items": 6 }, { "step": "merge", "items": 9 }])
    );
    assert_eq!(
        at(240)["elided"],
        json!([{ "step": "detail", "items": 11 }])
    );
    let order = ["detail", "text", "merge", "tail"];
    let mut shown = usize::MAX;
    // The smallest response: no line left, the envelope and one tail entry per timeline (each
    // with its `next`, which repeats the budget: a few more digits for a larger one).
    let floor = response(&at(0));
    for b in [240, 200, 150, 120, 115, 100, 90, 80, 60, 30] {
        let v = at(b);
        let s = steps(&v);
        // The steps run in the fixed order (tail may repeat, one per timeline).
        let idx: Vec<usize> = s
            .iter()
            .map(|x| order.iter().position(|o| o == x).unwrap())
            .collect();
        assert!(idx.windows(2).all(|w| w[0] <= w[1]), "{b}: {s:?}");
        let n = lines(&v).len();
        assert!(n <= shown, "fewer lines as the budget shrinks: {b}");
        shown = n;
        // Headers go last: only once no item line is left.
        let headers = lines(&v).iter().filter(|l| l.starts_with("== ")).count();
        assert!(headers == 2 || headers == n, "{b}: {}", text(&v));
        if b >= floor + 2 {
            assert!(response(&v) <= b, "{b}: {}", v["responseTokens"]);
        } else {
            assert!(lines(&v).is_empty(), "{b}: {}", text(&v));
            assert!(response(&v) <= floor + 2, "{b}: {}", v["responseTokens"]);
        }
        assert_eq!(v["resolved"]["budget"], b);
    }
    // Below the headers, they go too, last timeline first, each marked on its tail entry.
    let v = at(115);
    assert_eq!(lines(&v).len(), 1, "{v}");
    assert_eq!(v["elided"][1]["header"], true, "{v}");
    assert_eq!(v["elided"][0].get("header"), None, "{v}");
    let v = at(0);
    assert!(lines(&v).is_empty());
    assert_eq!(
        v["elided"][1],
        json!({ "step": "tail", "timeline": "tl2", "from": "0", "to": "2001/1000", "items": 2, "header": true,
                "next": "fin view --timeline tl2 --range 0.. --budget 0" })
    );
    let v = at(200);
    // Merging folds runs of a kind on a track; the tail cut names its range per timeline.
    assert!(steps(&v).contains(&"merge".to_string()), "{v}");
    let v = at(160);
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
        // Review batch L: the exact command for the cut lines, same budget.
        let next = t["next"].as_str().unwrap();
        assert!(next.starts_with("fin view --timeline tl"), "{next}");
        assert!(next.ends_with(" --budget 160"), "{next}");
    }
    // Just past the detail step, the text step shortens long caption text, and stops there
    // (a caption long enough for that to pay for its own `elided` entry).
    let mut p = project();
    let words: Vec<Value> = (0..60)
        .map(|n| json!({ "start": "0", "end": "0", "text": format!("word{n}") }))
        .collect();
    for tl in p["project"]["timelines"].as_array_mut().unwrap() {
        for t in tl["tracks"].as_array_mut().unwrap() {
            for i in t["items"].as_array_mut().unwrap() {
                if i["id"] == "cap3" {
                    i["words"] = json!(words);
                }
            }
        }
    }
    // Without `--budget` the response lacks `resolved.budget`, so this budget is just short.
    let full = response(&view_of(&p, &[]));
    let detail_only = view_of(&p, &["--budget", &full.to_string()]);
    assert_eq!(steps(&detail_only), ["detail"], "{detail_only}");
    let after_detail = response(&detail_only) - 5;
    let v = view_of(&p, &["--budget", &after_detail.to_string()]);
    assert_eq!(steps(&v), ["detail", "text"], "{v}");
    assert_eq!(v["elided"][1], json!({ "step": "text", "items": 1 }));
    assert!(
        lines(&v)
            .iter()
            .any(|l| l.ends_with("cap3 \"word0 word1 word2…\" (st1)")),
        "{}",
        text(&v)
    );
}

/// Review #43 (owner decision 2026-11-09): the budget caps the whole printed response, not
/// only the lines: budget 90 on the golden project, and a label of 400 newlines (escaped
/// twice: `\n` in the line, `\\n` in JSON) under 240.
#[test]
fn view_budget_covers_the_whole_response() {
    // 120: the golden's two tail entries with their `next` commands (review batch L) fit.
    let v = view(&["--budget", "120"]);
    assert!(response(&v) <= 120, "{v}");
    assert!(response(&v) > v["lineTokens"].as_u64().unwrap());
    let mut p = project();
    p["project"]["timelines"][0]["markers"][0]["label"] = json!("\n".repeat(400));
    let full = view_of(&p, &[]);
    assert!(response(&full) > 300, "{full}");
    for b in [240, 400, 1000] {
        let v = view_of(&p, &["--budget", &b.to_string()]);
        assert!(response(&v) <= b, "{b}: {v}");
    }
    // A roomier budget keeps more.
    let small = lines(&view_of(&p, &["--budget", "240"])).len();
    let big = lines(&view_of(&p, &["--budget", "1000"])).len();
    assert!(small <= big);
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

/// Review #47: frames in `--range` count at the shown timeline's rate (tl2 runs at 30000/1001
/// in a 30 fps project), and a view over timelines of different rates refuses frames.
#[test]
fn view_range_frames_count_at_the_shown_timeline_rate() {
    let ids = |v: &Value| -> Vec<String> {
        lines(v)
            .iter()
            .filter(|x| !x.starts_with("== "))
            .map(|x| {
                x.split("  ")
                    .nth(2)
                    .unwrap()
                    .split(' ')
                    .next()
                    .unwrap()
                    .to_string()
            })
            .collect()
    };
    for narrow in [&["--timeline", "tl2"][..], &["--track", "t5"][..]] {
        let mut args = narrow.to_vec();
        args.extend(["--range", "f30..f31"]);
        let v = view(&args);
        assert_eq!(
            v["resolved"]["range"],
            json!({ "from": "1001/1000", "to": "31031/30000" }),
            "{args:?}"
        );
        // c6 ends at 1001/1000, outside the half-open range; adj1 starts there.
        assert_eq!(ids(&v), ["adj1"], "{args:?}: {}", text(&v));
    }
    // Seconds and rationals need no rate: still fine across timelines.
    let v = view(&["--range", "1001/1000..2s"]);
    assert_eq!(
        v["resolved"]["range"],
        json!({ "from": "1001/1000", "to": "2" })
    );
    // tl1 (30) and tl2 (30000/1001) differ: a frame is ambiguous.
    for range in ["f30..f31", "f30..", "1s..f60"] {
        let r = offline(&project(), &["view", "--range", range]);
        assert_error(&r, 2, "USAGE");
        assert_eq!(r.json["error"]["details"]["field"], "range", "{}", r.stdout);
        assert_eq!(
            r.json["error"]["details"]["reason"], "MIXED_RATES",
            "{}",
            r.stdout
        );
        let fix = r.json["error"]["fix"].as_str().unwrap();
        assert!(fix.contains("--timeline"), "{fix}");
    }
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
