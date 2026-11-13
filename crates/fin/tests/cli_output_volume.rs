//! Review batch L (agent output volume and inspection), offline against the
//! `FIN_OFFLINE_SNAPSHOT` hook: `fin view`'s default page and its `next` commands, `--full`, a
//! `--track` header that names only that track, effect ids in view lines, capped `NOT_FOUND`
//! alternatives, `fin status` as a summary, and the scoped reads `fin track|asset|bin|effect ls`.

mod support;

use serde_json::{Value, json};
use support::{assert_error, offline};

fn golden() -> Value {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/view_golden");
    serde_json::from_str(&std::fs::read_to_string(dir.join("project.json")).unwrap()).unwrap()
}

/// The golden project with `n` more one-second clips of a1 on t1 from 10 s, ids `x1`…
fn with_clips(n: usize) -> Value {
    let mut p = golden();
    let items = p["project"]["timelines"][0]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap();
    for i in 0..n {
        items.push(
            json!({ "id": format!("x{}", i + 1), "kind": "clip", "asset": "a1",
                           "start": (10 + i).to_string(), "duration": "1", "sourceIn": "0" }),
        );
    }
    p
}

/// The golden project with `n` more empty video tracks on tl1.
fn with_tracks(n: usize) -> Value {
    let mut p = golden();
    let tracks = p["project"]["timelines"][0]["tracks"]
        .as_array_mut()
        .unwrap();
    for i in 0..n {
        tracks.push(
            json!({ "id": format!("w{}", i + 1), "name": format!("W{}", i + 1),
                            "kind": "video", "items": [] }),
        );
    }
    p
}

fn run(p: &Value, args: &[&str]) -> Value {
    let r = offline(p, args);
    assert_eq!(r.code, 0, "{args:?}: {}", r.stdout);
    r.json
}

fn chars(v: &Value) -> usize {
    v.to_string().chars().count() + 1
}

fn lines(v: &Value) -> Vec<String> {
    v["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

/// The arguments of a `next` command (`fin view --timeline tl1 …` → `["view", "--timeline", …]`).
fn argv(next: &Value) -> Vec<String> {
    let s = next.as_str().unwrap_or_else(|| panic!("no next: {next}"));
    let words: Vec<String> = s.split(' ').map(str::to_string).collect();
    assert_eq!(words[0], "fin", "{s}");
    words[1..].to_vec()
}

fn run_next(p: &Value, next: &Value) -> Value {
    let words = argv(next);
    let args: Vec<&str> = words.iter().map(String::as_str).collect();
    run(p, &args)
}

/// Item 1: without `--budget` a large view is a page: its first lines at full detail within
/// the default 4,000 tokens, a tail entry per cut timeline with the exact `next` command, and
/// paging through `next` reaches every line; `--full` prints them all.
#[test]
fn view_defaults_to_a_page_with_next_commands() {
    let p = with_clips(2_000);
    let full = run(&p, &["view", "--full"]);
    let all = lines(&full);
    assert!(chars(&full) > 16_000 * 4, "the project is large");
    assert_eq!(full["elided"], json!([]));
    assert_eq!(full["resolved"]["full"], true);

    let page = run(&p, &["view"]);
    assert!(
        page["responseTokens"].as_u64().unwrap() <= 4_000,
        "{}",
        page["responseTokens"]
    );
    let shown = lines(&page);
    // tl1's first lines, unchanged; tl2 (cut first, from the end) keeps only its header.
    let tl1_shown: Vec<&String> = shown
        .iter()
        .take_while(|l| !l.starts_with("== tl2"))
        .collect();
    let tl1_all: Vec<&String> = all.iter().take(tl1_shown.len()).collect();
    assert_eq!(tl1_shown, tl1_all, "a page is the first lines, unchanged");
    assert!(shown.last().unwrap().starts_with("== tl2"), "{shown:?}");
    let tail: Vec<&Value> = page["elided"].as_array().unwrap().iter().collect();
    assert!(
        tail.iter().all(|e| e["step"] == "tail"),
        "no detail/merge step: {tail:?}"
    );
    let first = tail.iter().find(|e| e["timeline"] == "tl1").unwrap();
    assert!(
        first["next"]
            .as_str()
            .unwrap()
            .starts_with("fin view --timeline tl1 --offset "),
        "{first}"
    );

    // Following `next` from page to page shows every item of tl1 once at least.
    let mut seen: std::collections::BTreeSet<String> = shown.iter().cloned().collect();
    let mut next = first["next"].clone();
    for _ in 0..50 {
        let v = run_next(&p, &next);
        assert!(v["responseTokens"].as_u64().unwrap() <= 4_000);
        seen.extend(lines(&v).into_iter().filter(|l| !l.starts_with("== ")));
        match v["elided"].as_array().unwrap().first() {
            Some(e) => next = e["next"].clone(),
            None => break,
        }
    }
    let tl1: Vec<&String> = all
        .iter()
        .take_while(|l| !l.starts_with("== tl2"))
        .collect();
    for l in tl1 {
        assert!(seen.contains(l), "never shown: {l}");
    }

    // A small project prints as before: no page, nothing in `resolved`.
    let small = run(&golden(), &["view"]);
    assert_eq!(small["elided"], json!([]));
    assert!(small.get("resolved").is_none(), "{small}");

    // `--full` and `--budget` exclude each other.
    assert_error(
        &offline(&p, &["view", "--full", "--budget", "100"]),
        2,
        "USAGE",
    );
}

/// An explicit `--budget` still degrades in its fixed order, and its tail entries name the next
/// command with the same budget.
#[test]
fn view_budget_tail_names_the_next_page() {
    // Clips and images alternate, so the merge step folds nothing and the tail cuts.
    let mut p = with_clips(400);
    for (i, it) in p["project"]["timelines"][0]["tracks"][0]["items"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .skip(3)
        .enumerate()
    {
        if i % 2 == 0 {
            *it = json!({ "id": it["id"], "kind": "image", "asset": "a3", "start": it["start"],
                          "duration": "1" });
        }
    }
    let v = run(
        &p,
        &["view", "--track", "t1", "--budget", "300", "--detail"],
    );
    let tail = v["elided"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["step"] == "tail")
        .cloned()
        .unwrap_or_else(|| panic!("{v}"));
    let next = tail["next"].as_str().unwrap();
    assert!(
        next.starts_with("fin view --timeline tl1 --track t1 --offset "),
        "{next}"
    );
    assert!(next.ends_with(" --budget 300 --detail"), "{next}");
}

/// Item 4: `--track` keeps the header to that track and counts the others.
#[test]
fn view_track_header_names_only_that_track() {
    let p = with_tracks(1_000);
    let v = run(&p, &["view", "--track", "t1"]);
    let header = &lines(&v)[0];
    assert!(header.ends_with("  V1=t1 (+1003 tracks)"), "{header}");
    assert!(chars(&v) < 1_000, "{}", chars(&v));
    // Without --track the header lists every track, as before.
    let all = run(&with_tracks(2), &["view", "--timeline", "tl1"]);
    assert!(
        lines(&all)[0].ends_with("C1=t4 W1=w1 W2=w2"),
        "{}",
        lines(&all)[0]
    );
}

/// Item 7: view lines name each effect `kind=id`; `--detail` adds its stored params.
#[test]
fn view_lines_name_effect_ids() {
    let v = run(&golden(), &["view"]);
    let l = lines(&v);
    assert!(
        l.iter()
            .any(|x| x.ends_with("c2 clip a1 [2.0s+5.25s] x1.5 opacity 0.8 fx: blur=fx1  ⟷ lk1")),
        "{l:?}"
    );
    assert!(
        l.iter()
            .any(|x| x.ends_with("adj1 adjustment blend screen fx: colorAdjust=fx2(off)")),
        "{l:?}"
    );
    let d = lines(&run(&golden(), &["view", "--detail"]));
    assert!(
        d.iter()
            .any(|x| x.contains("fx: blur=fx1{radius=4}  ⟷ lk1")),
        "{d:?}"
    );
}

/// Item 2: a missing item among thousands lists the 10 closest ids, the total and the lister.
#[test]
fn missing_address_lists_ten_closest_alternatives() {
    let p = with_clips(5_000);
    let r = offline(&p, &["item", "move", "x49999", "--to", "0"]);
    assert_error(&r, 3, "NOT_FOUND");
    let d = &r.json["error"]["details"];
    let alts = d["alternatives"].as_array().unwrap();
    assert_eq!(alts.len(), 10, "{d}");
    assert_eq!(alts[0], "x4999", "one edit away first");
    assert_eq!(d["alternativesTotal"], 5_000 + 13);
    assert!(r.stdout.chars().count() < 1_000, "{}", r.stdout);
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("`fin view`"),
        "{}",
        r.stdout
    );
}

/// Item 3: `fin status` cuts each timeline's tracks and the asset ids at 20, with totals and
/// the `ls` command that pages the rest; a small project is unchanged.
#[test]
fn status_is_a_bounded_summary() {
    let mut p = with_tracks(1_000);
    let assets = p["project"]["assets"].as_object_mut().unwrap();
    for i in 0..30 {
        let id = format!("z{i}");
        assets.insert(
            id.clone(),
            json!({ "id": id, "kind": "image", "path": format!("/m/{id}.png"),
                                          "fingerprint": id }),
        );
    }
    let s = run(&p, &["status"]);
    let tl1 = &s["timelines"][0];
    assert_eq!(tl1["tracks"].as_array().unwrap().len(), 20);
    assert_eq!(tl1["tracksTotal"], 1_004);
    assert_eq!(tl1["tracksNext"], "fin track ls --timeline tl1 --offset 20");
    assert_eq!(s["assets"].as_array().unwrap().len(), 20);
    assert_eq!(s["assetsNext"], "fin asset ls --offset 20");
    assert!(chars(&s) < 4_000, "{}", chars(&s));
    let small = run(&golden(), &["status"]);
    assert!(small["timelines"][0].get("tracksTotal").is_none());
    assert!(small.get("assetsTotal").is_none());
}

/// `fin track ls` pages through every track with the exact next command.
#[test]
fn track_ls_pages() {
    let p = with_tracks(120);
    let first = run(&p, &["track", "ls", "--timeline", "tl1"]);
    assert_eq!(first["total"], 124);
    assert_eq!(first["tracks"].as_array().unwrap().len(), 50);
    assert_eq!(first["tracks"][0]["id"], "t1");
    assert_eq!(first["tracks"][2]["mute"], true);
    assert_eq!(first["next"], "fin track ls --timeline tl1 --offset 50");
    let mut ids = Vec::new();
    let mut page = first;
    loop {
        ids.extend(
            page["tracks"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| t["id"].as_str().unwrap().to_string()),
        );
        if page["next"].is_null() {
            break;
        }
        page = run_next(&p, &page["next"]);
    }
    assert_eq!(ids.len(), 124);
    let small = run(&p, &["track", "ls", "--limit", "2", "--offset", "1"]);
    assert_eq!(small["tracks"][0]["id"], "t2");
    assert_eq!(small["next"], "fin track ls --offset 3 --limit 2");
    assert_error(&offline(&p, &["track", "ls", "--limit", "0"]), 2, "USAGE");
}

/// Items 8 and 9: stored effects (disabled ones too) with ids and values; assets with path and
/// bin; bins as a tree.
#[test]
fn effect_asset_and_bin_ls() {
    let mut p = golden();
    p["project"]["bins"] = json!({
        "b1": { "id": "b1", "name": "Originals" },
        "b2": { "id": "b2", "name": "Stills", "parent": "b1" },
    });
    p["project"]["assets"]["a3"]["bin"] = json!("b2");
    let fx = run(&p, &["effect", "ls"]);
    assert_eq!(fx["total"], 2);
    assert_eq!(
        fx["effects"][0],
        json!({ "item": "c2", "effect": "fx1", "kind": "blur", "enabled": true, "index": 0,
                "params": { "radius": 4.0 }, "animated": [] })
    );
    assert_eq!(
        fx["effects"][1]["enabled"], false,
        "disabled effects are listed"
    );
    let one = run(&p, &["effect", "ls", "adj1"]);
    assert_eq!(one["effects"][0]["effect"], "fx2");
    assert_error(&offline(&p, &["effect", "ls", "nope"]), 3, "NOT_FOUND");

    let assets = run(&p, &["asset", "ls"]);
    let a3 = assets["assets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "a3")
        .unwrap();
    assert_eq!(a3["bin"], "b2");
    assert!(a3["path"].as_str().unwrap().starts_with('/'), "{a3}");
    let in_b2 = run(&p, &["asset", "ls", "--bin", "b2"]);
    assert_eq!(in_b2["total"], 1);
    assert_error(
        &offline(&p, &["asset", "ls", "--bin", "b9"]),
        3,
        "NOT_FOUND",
    );

    let bins = run(&p, &["bin", "ls"]);
    assert_eq!(
        bins["bins"],
        json!([
            { "id": "b1", "name": "Originals", "parent": null, "path": "Originals", "depth": 0,
              "bins": 1, "assets": 0 },
            { "id": "b2", "name": "Stills", "parent": "b1", "path": "Originals/Stills",
              "depth": 1, "bins": 0, "assets": 1 },
        ])
    );
    assert_eq!(bins["topLevelAssets"], 3);
}

/// The views of `p`, page by page from `args` through each first `next`, until none; panics when
/// a page repeats a command (no progress). Returns every line shown (headers left out).
fn walk(p: &Value, args: &[&str]) -> Vec<String> {
    let mut seen = Vec::new();
    let mut cmds = std::collections::BTreeSet::new();
    let mut v = run(p, args);
    for _ in 0..500 {
        seen.extend(lines(&v).into_iter().filter(|l| !l.starts_with("== ")));
        let Some(next) = v["elided"]
            .as_array()
            .unwrap()
            .iter()
            .find_map(|e| e["next"].as_str().map(str::to_string))
        else {
            return seen;
        };
        assert!(cmds.insert(next.clone()), "paging repeats {next}");
        v = run_next(p, &json!(next));
    }
    panic!("paging did not end");
}

/// Review fix 1: lines that start together (400 tracks, each with a clip at 0) page through
/// by offset, never repeating a page, and every line shows.
#[test]
fn paging_progresses_through_lines_at_one_time() {
    let mut p = with_tracks(400);
    let tracks = p["project"]["timelines"][0]["tracks"]
        .as_array_mut()
        .unwrap();
    for (i, t) in tracks.iter_mut().skip(4).enumerate() {
        t["items"] = json!([{ "id": format!("z{i}"), "kind": "clip", "asset": "a1",
                              "start": "0", "duration": "5", "sourceIn": "0" }]);
    }
    let full = run(&p, &["view", "--track", "w1", "--full"]);
    assert_eq!(lines(&full).len(), 2, "{full}");
    let all = lines(&run(&p, &["view", "--timeline", "tl1", "--full"]));
    let first = run(&p, &["view", "--timeline", "tl1", "--budget", "2000"]);
    let tail = &first["elided"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["step"] == "tail")
        .cloned()
        .unwrap_or_else(|| panic!("{first}"));
    assert!(tail["offset"].as_u64().unwrap() > 0, "{tail}");
    // The default page fits the whole header, so walk with a budget that pages.
    let seen = walk(&p, &["view", "--timeline", "tl1", "--track", "t1"]);
    assert_eq!(seen.len(), 4, "c1, c2, tr1, c3: {seen:?}");
    // Under a budget (merging first) paging ends too.
    walk(&p, &["view", "--timeline", "tl1", "--budget", "2000"]);
    // The default page: every line shows, at full detail.
    let seen = walk(&p, &["view", "--timeline", "tl1"]);
    assert!(run(&p, &["view", "--timeline", "tl1"])["elided"][0]["next"].is_string());
    for l in all.iter().skip(1) {
        assert!(seen.contains(l), "never shown: {l}");
    }
}

/// Review fix 2: `next` keeps the requested range as it was, never widening it.
#[test]
fn next_keeps_the_requested_range() {
    let p = with_clips(400);
    let args = ["view", "--timeline", "tl1", "--range", "1/3..400"];
    let v = run(&p, &args);
    let next = v["elided"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|e| e["next"].as_str())
        .unwrap_or_else(|| panic!("{v}"))
        .to_string();
    assert!(next.contains(" --range 1/3..400 "), "{next}");
    walk(&p, &args);
}

/// Review fix 1: a header alone over the budget cannot page; it says so and names the track
/// listing instead of a `next` that would repeat. A multi-timeline view narrows instead.
#[test]
fn an_oversized_header_is_blocked_with_a_pointer() {
    let p = with_tracks(2_500);
    let v = run(&p, &["view", "--timeline", "tl1"]);
    let tail = v["elided"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["timeline"] == "tl1")
        .cloned()
        .unwrap_or_else(|| panic!("{v}"));
    assert_eq!(tail["header"], true, "{tail}");
    assert!(tail["next"].is_null(), "{tail}");
    assert!(
        tail["blocked"]
            .as_str()
            .unwrap()
            .contains("fin track ls --timeline tl1"),
        "{tail}"
    );
    let all = run(&with_clips(2_000), &["view", "--budget", "100"]);
    let tl2 = all["elided"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["timeline"] == "tl2")
        .cloned()
        .unwrap();
    assert_eq!(tl2["next"], "fin view --timeline tl2 --budget 100", "{tl2}");
}

/// Review fix 3: every follow-up carries the global `--project` and `--branch`.
#[test]
fn follow_ups_carry_project_and_branch() {
    let p = with_clips(2_000);
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().to_str().unwrap();
    let pre = format!("fin --project {d} --branch work");
    let v = run(&p, &["--project", d, "--branch", "work", "view"]);
    let next = v["elided"][0]["next"].as_str().unwrap();
    assert!(
        next.starts_with(&format!("{pre} view --timeline tl1 --offset ")),
        "{next}"
    );
    let s = run(
        &with_tracks(30),
        &["--project", d, "--branch", "work", "status"],
    );
    assert_eq!(
        s["timelines"][0]["tracksNext"],
        format!("{pre} track ls --timeline tl1 --offset 20")
    );
    let t = run(&with_tracks(60), &["--branch", "work", "track", "ls"]);
    assert_eq!(t["next"], "fin --branch work track ls --offset 50");
    let r = offline(
        &p,
        &["--branch", "work", "item", "move", "nope", "--to", "0"],
    );
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("`fin --branch work view`"),
        "{}",
        r.stdout
    );
}

/// Review fixes 5, 6: status caps its timelines too (a total, no listing to name).
#[test]
fn status_caps_timelines() {
    let mut p = golden();
    let tls = p["project"]["timelines"].as_array_mut().unwrap();
    let mut extra = tls[1].clone();
    extra["tracks"] = json!([]);
    for i in 0..30 {
        let mut t = extra.clone();
        t["id"] = json!(format!("tlx{i}"));
        tls.push(t);
    }
    let s = run(&p, &["status"]);
    assert_eq!(s["timelines"].as_array().unwrap().len(), 20);
    assert_eq!(s["timelinesTotal"], 32);
    assert!(s.get("timelinesNext").is_none());
}

/// Review fix 10: `fin effect ls` honours the global `--timeline`.
#[test]
fn effect_ls_honours_the_timeline() {
    let tl2 = run(&golden(), &["effect", "ls", "--timeline", "tl2"]);
    assert_eq!(tl2["total"], 1);
    assert_eq!(tl2["effects"][0]["item"], "adj1");
    let tl1 = run(&golden(), &["effect", "ls", "--timeline", "tl1"]);
    assert_eq!(tl1["total"], 1);
    assert_eq!(tl1["effects"][0]["item"], "c2");
}
