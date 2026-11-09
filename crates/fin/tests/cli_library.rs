//! `fin library …` and `fin search` against a real finisherd (plan 5D Task 14): the listing,
//! status, files, show and search output; a file asked for by id when two names meet (NFC and
//! NFD twins); a scan's job waited on with `fin wait`; libraries by id or name; the fields only a
//! person decides refused with `HUMAN_ONLY` (exit 7); the usage errors; and `SERVER_UNAVAILABLE`
//! with no server. Libraries are added through the console connection, as `finisherd library
//! add` does (the CLI never adds or removes one).

mod support;

use std::path::Path;

use serde_json::{Value, json};
use support::console::Console;
use support::{Server, assert_error, fin, ok};

fn write(root: &Path, rel: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"not really media").unwrap();
}

/// A server with one library `Raw` over `<root>/raw`, its first scan finished.
fn with_library(test: &str, files: &[&str]) -> Option<(Server, String)> {
    let server = Server::start(test)?;
    let root = server.root.join("raw");
    for f in files {
        write(&root, f);
    }
    let mut console = Console::open(&server);
    let added = console.confirmed_ok(
        "library.add",
        json!({ "root": root.to_str().unwrap(), "name": "Raw" }),
    );
    let job = added["job"]["id"].as_str().unwrap().to_string();
    let waited = ok(fin(
        &server.sd,
        &server.root,
        &["wait", &job, "--timeout", "120"],
    ));
    assert_eq!(waited["jobs"][0]["state"], "succeeded", "{waited}");
    Some((server, added["library"]["id"].as_str().unwrap().to_string()))
}

fn names(files: &Value) -> Vec<String> {
    files["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["display"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn ls_status_files_show_search_output_and_json() {
    let Some((server, id)) = with_library(
        "library_output",
        &["a.mov", "clips/interview_take3.final.mov", "clips/b.mov"],
    ) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    let run = |args: &[&str]| fin(sd, cwd, args);

    let ls = ok(run(&["library", "ls"]));
    let libs = ls["libraries"].as_array().unwrap();
    assert_eq!(libs.len(), 1, "{ls}");
    assert_eq!(libs[0]["name"], "Raw");
    assert_eq!(libs[0]["id"], id.as_str());
    assert_eq!(libs[0]["storage"], "local");

    let status = ok(run(&["library", "status", "Raw"]));
    assert_eq!(status["library"]["id"], id.as_str(), "{status}");
    assert!(
        status["jobs"].is_array() && status["stages"].is_array(),
        "{status}"
    );

    let root = ok(run(&["library", "files", "Raw"]));
    assert_eq!(names(&root), ["a.mov"], "{root}");
    assert_eq!(root["dirs"][0]["name"], "clips");
    let page = ok(run(&["library", "files", "Raw", "clips", "--limit", "1"]));
    assert_eq!(names(&page), ["clips/b.mov"], "{page}");
    let cursor = page["next"].as_str().unwrap().to_string();
    let next = ok(run(&[
        "library", "files", "Raw", "clips", "--limit", "1", "--cursor", &cursor,
    ]));
    assert_eq!(names(&next), ["clips/interview_take3.final.mov"], "{next}");
    // An absolute folder under the root is turned into a relative one.
    let abs = server.root.join("raw/clips");
    let by_abs = ok(run(&["library", "files", "Raw", abs.to_str().unwrap()]));
    assert_eq!(by_abs["dir"]["display"], "clips");

    let file = ok(run(&["library", "show", "Raw", "clips/b.mov"]));
    assert_eq!(file["file"]["display"], "clips/b.mov", "{file}");
    let abs_file = server.root.join("raw/clips/b.mov");
    let by_abs = ok(run(&["library", "show", "Raw", abs_file.to_str().unwrap()]));
    assert_eq!(by_abs["file"]["id"], file["file"]["id"]);
    let by_id = ok(run(&[
        "library",
        "show",
        "Raw",
        "--id",
        &file["file"]["id"].to_string(),
    ]));
    assert_eq!(by_id["file"]["display"], "clips/b.mov");
    assert_error(
        &run(&["library", "show", "Raw", "nope.mov"]),
        3,
        "FILE_NOT_FOUND",
    );
    let outside = server.root.join("elsewhere/b.mov");
    assert_error(
        &run(&["library", "show", "Raw", outside.to_str().unwrap()]),
        3,
        "INVALID_PATH",
    );
    assert_error(
        &run(&["library", "show", "Raw", "../x.mov"]),
        3,
        "INVALID_PATH",
    );

    // `fin search` is `fin library search`.
    let found = ok(run(&["search", "interview"]));
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{found}");
    assert_eq!(hits[0]["display"], "clips/interview_take3.final.mov");
    assert_eq!(
        ok(run(&["library", "search", "interv", "--lib", "raw"])),
        found
    );
    assert_eq!(found["truncated"], false);
    let none = ok(run(&[
        "search",
        "interview",
        "--lib",
        &id,
        "--min-duration",
        "1:00:00",
    ]));
    assert_eq!(none["hits"], json!([]), "{none}");
    let all = ok(run(&["search", "--limit", "2"]));
    assert_eq!(all["hits"].as_array().unwrap().len(), 2, "{all}");
    assert!(all["next"].is_string(), "{all}");
    assert_error(
        &run(&["search", "x", "--lib", "nope"]),
        3,
        "LIBRARY_NOT_FOUND",
    );
}

#[test]
fn libraries_by_id_or_name() {
    let Some((server, id)) = with_library("library_names", &["a.mov"]) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    for name in [id.as_str(), "Raw", "raw", "RAW"] {
        let s = ok(fin(sd, cwd, &["library", "status", name]));
        assert_eq!(s["library"]["id"], id.as_str(), "{name}");
    }
    assert_error(
        &fin(sd, cwd, &["library", "status", "nowhere"]),
        3,
        "LIBRARY_NOT_FOUND",
    );
    assert_error(
        &fin(sd, cwd, &["library", "files", "nowhere"]),
        3,
        "LIBRARY_NOT_FOUND",
    );
}

#[test]
fn show_by_id_resolves_ambiguity() {
    let Some((server, _)) = with_library("library_twins", &["a.mov"]) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    // Two names that are one after NFC: precomposed and decomposed é.
    let root = server.root.join("raw");
    write(&root, "caf\u{e9}.mov");
    write(&root, "cafe\u{301}.mov");
    if std::fs::read_dir(&root).unwrap().count() < 3 {
        eprintln!(
            "SKIPPED show_by_id_resolves_ambiguity: the filesystem unifies NFC and NFD names"
        );
        return;
    }
    let job = ok(fin(sd, cwd, &["library", "scan", "Raw", "--full"]))["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    ok(fin(sd, cwd, &["wait", &job, "--timeout", "120"]));
    let listing = ok(fin(sd, cwd, &["library", "files", "Raw"]));
    assert_eq!(names(&listing).len(), 3, "both twins are listed: {listing}");
    let r = fin(sd, cwd, &["library", "show", "Raw", "caf\u{e9}.mov"]);
    assert_error(&r, 3, "PATH_AMBIGUOUS");
    let ids = r.json["error"]["details"]["fileIds"].as_array().unwrap();
    assert_eq!(ids.len(), 2, "{}", r.stdout);
    for id in ids {
        let one = ok(fin(
            sd,
            cwd,
            &["library", "show", "Raw", "--id", &id.to_string()],
        ));
        assert_eq!(one["file"]["id"], *id);
    }
}

#[test]
fn scan_prints_job_and_returns() {
    let Some((server, id)) = with_library("library_scan", &["a.mov", "clips/b.mov"]) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    let run = |args: &[&str]| fin(sd, cwd, args);
    for args in [
        vec!["library", "scan", "Raw"],
        vec!["library", "scan", "Raw", "--full"],
        vec!["library", "scan", "Raw", "--dir", "clips"],
    ] {
        let scan = ok(run(&args));
        assert_eq!(scan["job"]["kind"], "library.scan", "{args:?}: {scan}");
        assert!(scan["job"]["id"].is_string());
    }
    assert_error(
        &run(&["library", "scan", "Raw", "--dir", "nope"]),
        3,
        "FILE_NOT_FOUND",
    );
    // Paused: the scan says why; resuming brings it back.
    ok(run(&["library", "set", "Raw", "--pause"]));
    let paused = run(&["library", "scan", "Raw"]);
    assert_error(&paused, 5, "LIBRARY_PAUSED");
    assert_eq!(
        paused.json["error"]["details"]["id"],
        id.as_str(),
        "{}",
        paused.stdout
    );
    let set = ok(run(&[
        "library", "set", "Raw", "--resume", "--poll", "300", "--full", "weekly",
    ]));
    assert_eq!(
        set["library"]["scan"],
        json!({ "poll": 300, "full": "weekly" }),
        "{set}"
    );
    assert_ne!(set["library"]["state"], "paused");
}

#[test]
fn fin_wait_on_scan_job() {
    let Some((server, _)) = with_library("library_wait", &["a.mov"]) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    write(&server.root.join("raw"), "later.mov");
    let job = ok(fin(sd, cwd, &["library", "scan", "Raw", "--full"]))["job"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let waited = ok(fin(sd, cwd, &["wait", &job, "--timeout", "120"]));
    assert_eq!(waited["jobs"][0]["id"], job.as_str());
    assert_eq!(waited["jobs"][0]["state"], "succeeded", "{waited}");
    let listing = ok(fin(sd, cwd, &["library", "files", "Raw"]));
    assert_eq!(names(&listing), ["a.mov", "later.mov"], "{listing}");
    assert_eq!(
        ok(fin(sd, cwd, &["job", "show", &job]))["kind"],
        "library.scan"
    );
}

#[test]
fn set_human_fields_from_fin_is_human_only_exit_7() {
    let Some((server, _)) = with_library("library_human", &["a.mov"]) else {
        return;
    };
    let (sd, cwd) = (&server.sd, &server.root);
    for args in [
        vec!["--hydrate", "on"],
        vec!["--bandwidth", "20"],
        vec!["--bandwidth", "off"],
        vec!["--alias", "/mnt/raw"],
        vec!["--no-aliases"],
        vec!["--accept-volume"],
        vec!["--accept-unmounted", "clips"],
        vec!["--accept-empty-root"],
        // Mixed with an ordinary field, nothing is changed.
        vec!["--name", "Other", "--hydrate", "off"],
    ] {
        let mut full = vec!["library", "set", "Raw"];
        full.extend(&args);
        let r = fin(sd, cwd, &full);
        assert_error(&r, 7, "HUMAN_ONLY");
        let fix = r.json["error"]["fix"].as_str().unwrap();
        assert!(!fix.contains('`') && !fix.contains("fin "), "{fix}");
    }
    let ls = ok(fin(sd, cwd, &["library", "ls"]));
    assert_eq!(ls["libraries"][0]["name"], "Raw", "{ls}");
    assert_eq!(ls["libraries"][0]["hydrate"], false);
    assert_eq!(ls["libraries"][0]["aliases"], json!([]));
    // The owner's console does them.
    let mut console = Console::open(&server);
    let set = console.confirmed_ok("library.set", json!({ "id": "Raw", "hydrate": true }));
    assert_eq!(set["library"]["hydrate"], true, "{set}");
}

#[test]
fn usage_errors_never_reach_the_server() {
    // No server at all: a bad flag is still a usage error (exit 2), a good command is
    // SERVER_UNAVAILABLE (exit 5).
    let dir = tempfile::tempdir().unwrap();
    let (sd, cwd) = (dir.path().join("sd"), dir.path());
    for args in [
        vec!["library", "show", "raw"],
        vec!["library", "set", "raw"],
        vec!["library", "set", "raw", "--poll", "soon"],
        vec!["library", "set", "raw", "--pause", "--resume"],
        vec!["library", "files", "raw", "--state", "gone"],
        vec!["library", "files", "raw", "--limit", "501"],
        vec!["library", "search", "x", "--limit", "201"],
        vec!["search", "x", "--min-duration", "soon"],
        vec!["search", "x", "--since", "yesterday"],
    ] {
        let r = fin(&sd, cwd, &args);
        assert_eq!(r.code, 2, "{args:?}: {}", r.stdout);
        assert_eq!(r.json["error"]["code"], "USAGE", "{args:?}");
    }
    for args in [
        vec!["library", "ls"],
        vec!["library", "status", "raw"],
        vec!["library", "scan", "raw"],
        vec!["library", "files", "raw"],
        vec!["library", "show", "raw", "a.mov"],
        vec!["library", "set", "raw", "--pause"],
        vec!["search", "x"],
    ] {
        assert_error(&fin(&sd, cwd, &args), 5, "SERVER_UNAVAILABLE");
    }
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
