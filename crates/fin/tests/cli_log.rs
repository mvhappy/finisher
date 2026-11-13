//! Review batch L against a real `finisherd`: `fin log` (pages of a branch's records from
//! `history.log`, newest first, and one record from `tx.get`), and a refused merge's conflicts
//! listed 20 at a time with the exact dry-run that shows the next page. Tests print
//! `SKIPPED <test>: <reason>` and pass when Node or fin-node is missing.

mod support;

use std::path::{Path, PathBuf};

use serde_json::Value;
use support::{Run, Server, assert_error, fin, ok};

fn project(test: &str) -> Option<(Server, PathBuf)> {
    let server = Server::start(test)?;
    let dir = server.root.join("film");
    ok(fin(
        &server.sd,
        &server.root,
        &["project", "create", "film", "--name", "Film"],
    ));
    Some((server, dir))
}

fn run(s: &Server, dir: &Path, args: &[&str]) -> Value {
    ok(fin(&s.sd, dir, args))
}

fn raw(s: &Server, dir: &Path, args: &[&str]) -> Run {
    fin(&s.sd, dir, args)
}

/// The words of a `next` command after `fin`.
fn argv(next: &Value) -> Vec<String> {
    let s = next.as_str().unwrap_or_else(|| panic!("no next: {next}"));
    s.split(' ').skip(1).map(str::to_string).collect()
}

#[test]
fn log_pages_newest_first_and_shows_one_record() {
    let Some((s, dir)) = project("log_pages_newest_first_and_shows_one_record") else {
        return;
    };
    run(&s, &dir, &["checkpoint", "zero"]);
    for i in 0..25 {
        let at = i.to_string();
        let label = format!("M{i}");
        let key = format!("m-{i}");
        run(
            &s,
            &dir,
            &[
                "--idem-key",
                &key,
                "marker",
                "add",
                "--at",
                &at,
                "--label",
                &label,
            ],
        );
    }
    run(&s, &dir, &["checkpoint", "base"]);
    let head = run(&s, &dir, &["log", "--limit", "10"]);
    let records = head["records"].as_array().unwrap();
    assert_eq!(records.len(), 10);
    assert_eq!(head["branch"], "main");
    let top = head["head"].as_u64().unwrap();
    assert_eq!(records[0]["rev"], top, "newest first");
    assert_eq!(records[0]["kind"], "checkpoint");
    assert_eq!(records[0]["name"], "base");
    assert_eq!(records[1]["txId"], "m-24");
    assert_eq!(records[1]["kind"], "edit");
    assert_eq!(records[1]["ops"], 1);
    assert_eq!(records[1]["author"], "fin");
    let lowest = records[9]["rev"].as_u64().unwrap();
    assert_eq!(
        head["next"],
        format!("fin log --before {lowest} --limit 10")
    );

    // Paging through `next` lists every record once, down to rev 1.
    let mut seen: Vec<u64> = records.iter().map(|r| r["rev"].as_u64().unwrap()).collect();
    let mut next = head["next"].clone();
    while !next.is_null() {
        let words = argv(&next);
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let page = run(&s, &dir, &args);
        seen.extend(
            page["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["rev"].as_u64().unwrap()),
        );
        next = page["next"].clone();
    }
    let want: Vec<u64> = (1..=top).rev().collect();
    assert_eq!(seen, want);

    // The default page is 20, and stays small.
    let page = raw(&s, &dir, &["log"]);
    assert_eq!(page.json["records"].as_array().unwrap().len(), 20);
    assert!(page.stdout.chars().count() < 6_000, "{}", page.stdout);

    // One record: its ops and changes; unknown → NOT_FOUND.
    let one = run(&s, &dir, &["log", "m-3"]);
    assert_eq!(one["tx"]["txId"], "m-3");
    assert_eq!(one["tx"]["ops"][0]["op"], "marker.add");
    assert_eq!(one["tx"]["changes"][0]["kind"], "marker");
    assert_error(&raw(&s, &dir, &["log", "nope"]), 3, "NOT_FOUND");
    assert_error(&raw(&s, &dir, &["log", "--limit", "101"]), 2, "USAGE");

    // An undo of 25 records names them all in the log, cut to 20 with the total.
    let undo = run(&s, &dir, &["undo", "--to", "zero"]);
    let rec = run(&s, &dir, &["log", "--limit", "1"]);
    let u = &rec["records"][0];
    assert_eq!(u["kind"], "undo", "{rec}");
    assert_eq!(u["txId"], undo["txId"]);
    assert_eq!(u["undoes"].as_array().unwrap().len(), 20, "{u}");
    assert_eq!(u["undoesTotal"], 25);
}

#[test]
fn refused_merge_lists_conflicts_a_page_at_a_time() {
    let Some((s, dir)) = project("refused_merge_lists_conflicts_a_page_at_a_time") else {
        return;
    };
    run(&s, &dir, &["marker", "add", "--at", "0", "--label", "Here"]);
    run(&s, &dir, &["branch", "new", "work"]);
    run(&s, &dir, &["marker", "set", "m1", "label", "Main"]);
    for i in 0..45 {
        let label = format!("Other {i}");
        run(
            &s,
            &dir,
            &["--branch", "work", "marker", "set", "m1", "label", &label],
        );
    }
    let r = raw(&s, &dir, &["--dry-run", "merge", "work"]);
    assert_error(&r, 4, "CONFLICT");
    let d = &r.json["error"]["details"];
    assert_eq!(d["conflicts"].as_array().unwrap().len(), 20);
    assert_eq!(d["conflictsTotal"], 45);
    assert_eq!(d["conflictsFrom"], 0);
    assert_eq!(
        d["conflictsNext"],
        "fin merge work --into main --dry-run --conflicts-from 20"
    );
    assert!(
        r.stdout.chars().count() < 6_000,
        "{}",
        r.stdout.chars().count()
    );
    let first: Vec<Value> = d["conflicts"].as_array().unwrap().clone();

    let mut all = first;
    let mut next = d["conflictsNext"].clone();
    while !next.is_null() {
        let words = argv(&next);
        let args: Vec<&str> = words.iter().map(String::as_str).collect();
        let page = raw(&s, &dir, &args);
        assert_error(&page, 4, "CONFLICT");
        let pd = &page.json["error"]["details"];
        all.extend(pd["conflicts"].as_array().unwrap().iter().cloned());
        next = pd["conflictsNext"].clone();
    }
    assert_eq!(all.len(), 45);
    let mut ids: Vec<&str> = all.iter().map(|c| c["txId"].as_str().unwrap()).collect();
    ids.dedup();
    assert_eq!(ids.len(), 45, "each conflict once");
    // Nothing was merged by paging.
    let log = run(&s, &dir, &["log", "--limit", "1"]);
    assert_eq!(log["records"][0]["kind"], "edit");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
