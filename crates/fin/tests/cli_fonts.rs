//! `fin fonts ls|fetch|status|pin|repin|unpin` (plan 4C Decision 44, Task 24): asset pins and
//! in-use refusals offline (the shared snapshot), and the catalog, cache and fetcher through
//! a finisherd — the built-in catalog for `ls`'s budget, and test catalogs over a loopback
//! font server (test-hooks build) for fetching, catalog pins and repins. No internet.

mod support;

use serde_json::{Value, json};
use support::fontserver::{self, CatalogFile, FontServer, sha256};
use support::settings::snapshot;
use support::{Run, Server, assert_error, fin, fin_cmd, finish, offline, ok};

fn run(args: &[&str]) -> Run {
    offline(&snapshot(), args)
}

#[test]
fn fonts_pin_asset() {
    let r = run(&[
        "fonts",
        "pin",
        "TestSans",
        "--weight",
        "700",
        "--asset",
        "a3",
        "--fallback",
        "0",
    ]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_eq!(
        r.json["resolved"],
        json!({ "key": "TestSans|700|normal", "sha256": "ab".repeat(32),
                "source": { "type": "asset", "asset": "a3" }, "fallback": 0 })
    );
    let font = r.json["diff"]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["kind"] == "font")
        .cloned()
        .unwrap();
    assert_eq!(font["id"], "font/TestSans|700|normal", "{font}");
    // No family: the asset's face at that weight.
    let r = run(&["fonts", "pin", "--asset", "a3", "--weight", "700"]);
    assert_eq!(
        r.json["resolved"]["key"], "TestSans|700|normal",
        "{}",
        r.stdout
    );
    // Pinning the held face again is a no-op.
    let r = run(&["fonts", "pin", "--asset", "a3"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_eq!(r.json["diff"]["changes"], json!([]));

    let e = run(&[
        "fonts", "pin", "TestSans", "--weight", "900", "--asset", "a3",
    ]);
    assert_eq!(e.code, 3, "{}", e.stdout);
    assert_eq!(e.json["error"]["code"], "NOT_FOUND");
    assert_eq!(
        e.json["error"]["details"]["alternatives"],
        json!(["TestSans|400|normal", "TestSans|700|normal"])
    );
    let e = run(&["fonts", "pin", "--asset", "a1"]);
    assert_eq!(e.json["error"]["code"], "INVALID_OP");
    // A catalog pin needs the server's catalog.
    let e = run(&["fonts", "pin", "Inter"]);
    assert_eq!(e.code, 5, "{}", e.stdout);
    assert_eq!(e.json["error"]["code"], "SERVER_UNAVAILABLE");
}

#[test]
fn fonts_unpin_in_use_refused() {
    let r = run(&["fonts", "unpin", "TestSans|400|normal"]);
    assert_eq!(r.code, 3, "{}", r.stdout);
    assert_eq!(r.json["error"]["code"], "FONT_IN_USE");
    assert!(
        r.json["error"]["fix"]
            .as_str()
            .is_some_and(|f| !f.is_empty())
    );
    let r = run(&["fonts", "unpin", "Nope|400|normal"]);
    assert_eq!(r.json["error"]["code"], "NOT_FOUND", "{}", r.stdout);
    // An asset pin is not repinned from the catalog.
    let r = run(&["fonts", "repin", "TestSans|400|normal"]);
    assert_eq!(r.json["error"]["code"], "INVALID_OP", "{}", r.stdout);
}

#[test]
fn fonts_ls_pinned_works_offline() {
    // cap1 by its style, cap2 (preset:bold, Inter unpinned) by the fallback order.
    let r = run(&["fonts", "ls", "--pinned"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert_eq!(
        r.json["pins"],
        json!([{ "key": "TestSans|400|normal", "source": { "type": "asset", "asset": "a3" },
                 "fallback": 0, "styles": ["st1"], "captions": ["cap1", "cap2"] }])
    );
    for args in [
        &["fonts", "ls"][..],
        &["fonts", "status"],
        &["fonts", "fetch", "inter"],
    ] {
        let r = run(args);
        assert_eq!(r.code, 5, "{args:?}: {}", r.stdout);
        assert_eq!(r.json["error"]["code"], "SERVER_UNAVAILABLE");
    }
}

#[test]
fn fonts_ls_fits_budget() {
    let Some(server) = Server::start("fonts_ls_fits_budget") else {
        return;
    };
    let r = fin(&server.sd, &server.root, &["fonts", "ls"]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    let entries = r.json["entries"].as_array().unwrap();
    assert!(entries.len() >= 5, "{}", r.stdout);
    assert!(entries.iter().any(|e| e["family"] == "Inter"));
    let style = &entries[0]["styles"][0];
    assert_eq!(style["cached"], false);
    assert_eq!(style["pinned"], false);
    assert!(style.get("urls").is_none());
    // Within the map's budget (ceil(chars / 4) tokens).
    assert!(
        r.stdout.len().div_ceil(4) < 1500,
        "{} tokens",
        r.stdout.len().div_ceil(4)
    );
    let detail = ok(fin(&server.sd, &server.root, &["fonts", "ls", "--detail"]));
    let urls = &detail["entries"][0]["styles"][0]["urls"];
    assert!(urls[0].as_str().unwrap().starts_with("https://"), "{urls}");
    assert_eq!(detail["entries"][0]["licence"], "OFL-1.1");
    let cached = ok(fin(&server.sd, &server.root, &["fonts", "ls", "--cached"]));
    assert_eq!(cached["entries"], json!([]));
    let status = ok(fin(&server.sd, &server.root, &["fonts", "status"]));
    assert!(status["files"].as_array().unwrap().len() >= entries.len());
    assert_eq!(status["catalogVersion"], r.json["catalogVersion"]);
}

fn sans_catalog(version: &str, bytes: &[u8], url: String) -> Value {
    fontserver::catalog_json(
        version,
        &[CatalogFile {
            id: "sans",
            family: "Test Sans",
            weight: 400,
            bytes,
            url,
        }],
        &["sans"],
    )
}

#[test]
fn fonts_fetch_failed_exits_5_with_fix() {
    let fonts = FontServer::start();
    let bytes = fin_testfonts::test_sans();
    let url = fonts.fail("sans.ttf", 500);
    let Some(f) = fontserver::start(
        "fonts_fetch_failed_exits_5_with_fix",
        &sans_catalog("2026.11.1", &bytes, url),
    ) else {
        return;
    };
    let args = ["fonts", "fetch", "sans"];
    let r = finish(
        &args,
        fin_cmd(&f.server.sd, &f.server.root, &args)
            .output()
            .unwrap(),
    );
    assert_error(&r, 5, "FONT_FETCH_FAILED");
    let fix = r.json["error"]["fix"].as_str().unwrap();
    assert!(
        fix.contains("proxy") && fix.contains("FIN_FONTS_OFFLINE"),
        "{fix}"
    );
    assert_eq!(r.json["error"]["details"]["sha256"], sha256(&bytes));
    assert!(fonts.hits("sans.ttf") >= 1);
    let r = fin(&f.server.sd, &f.server.root, &["fonts", "fetch", "nope"]);
    assert_error(&r, 3, "FONT_NOT_IN_CATALOG");
}

#[test]
fn fonts_fetch_pin_and_repin_to_catalog_version() {
    let fonts = FontServer::start();
    let old = fin_testfonts::test_sans();
    let url = fonts.serve("sans.ttf", &old);
    let Some(mut f) = fontserver::start(
        "fonts_fetch_pin_and_repin_to_catalog_version",
        &sans_catalog("2026.11.1", &old, url),
    ) else {
        return;
    };
    let (sd, root) = (f.server.sd.clone(), f.server.root.clone());
    ok(fin(
        &sd,
        &root,
        &["project", "create", "film", "--name", "Film"],
    ));
    let dir = root.join("film");

    // Fetch by key: cached, and listed as such.
    let status = ok(fin(&sd, &dir, &["fonts", "fetch", "Test Sans|400|normal"]));
    assert_eq!(status["files"][0]["state"]["state"], "cached", "{status}");
    assert_eq!(fonts.hits("sans.ttf"), 1);
    let cached = ok(fin(&sd, &dir, &["fonts", "ls", "--cached"]));
    assert_eq!(cached["entries"][0]["entry"], "sans");

    // A catalog pin, as fonts.plan would make it.
    let pin = ok(fin(&sd, &dir, &["fonts", "pin", "Test Sans"]));
    assert_eq!(pin["resolved"]["key"], "Test Sans|400|normal");
    assert_eq!(pin["resolved"]["sha256"], sha256(&old));
    assert_eq!(
        pin["resolved"]["source"],
        json!({ "type": "catalog", "entry": "sans", "style": "regular", "catalog": "2026.11.1" })
    );
    let e = fin(&sd, &dir, &["fonts", "pin", "Test Sans", "--weight", "700"]);
    assert_error(&e, 3, "FONT_NOT_IN_CATALOG");
    let ls = ok(fin(&sd, &dir, &["fonts", "ls"]));
    assert_eq!(ls["entries"][0]["styles"][0]["pinned"], true, "{ls}");
    ok(fin(&sd, &dir, &["fonts", "fetch", "--pinned"]));

    // A newer catalog serves other bytes for the same style: only repin moves the pin.
    let new = fin_testfonts::test_rtl();
    let url = fonts.serve("sans-2.ttf", &new);
    std::fs::write(&f.catalog, sans_catalog("2026.11.2", &new, url).to_string()).unwrap();
    f.server.restart();
    let pinned = ok(fin(&sd, &dir, &["fonts", "ls", "--pinned", "--detail"]));
    assert_eq!(pinned["pins"][0]["sha256"], sha256(&old));
    let e = fin(
        &sd,
        &dir,
        &[
            "fonts",
            "repin",
            "Test Sans|400|normal",
            "--catalog-version",
            "2026.11.1",
        ],
    );
    assert_error(&e, 3, "FONT_NOT_IN_CATALOG");
    let repin = ok(fin(
        &sd,
        &dir,
        &[
            "fonts",
            "repin",
            "Test Sans|400|normal",
            "--catalog-version",
            "2026.11.2",
        ],
    ));
    assert_eq!(
        repin["resolved"],
        json!({ "key": "Test Sans|400|normal",
                "from": { "sha256": sha256(&old), "catalog": "2026.11.1" },
                "to": { "sha256": sha256(&new), "catalog": "2026.11.2" } })
    );
    let pinned = ok(fin(&sd, &dir, &["fonts", "ls", "--pinned", "--detail"]));
    assert_eq!(pinned["pins"][0]["sha256"], sha256(&new));
    // Unused now: unpin removes it.
    ok(fin(&sd, &dir, &["fonts", "unpin", "Test Sans|400|normal"]));
    let pinned = ok(fin(&sd, &dir, &["fonts", "ls", "--pinned"]));
    assert_eq!(pinned["pins"], json!([]));
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
