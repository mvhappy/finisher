//! Sessions (Decision 25), `record_value_sync`, the status report and paged export (plan D,
//! Task D4's journal side).

mod support;

use std::time::Duration;

use fin_core::events::Source;
use fin_core::events::Version;
use fin_core::events::payloads::{Arch, BuildProfile, OsFamily, SessionStart};
use fin_journal::{EnvOverrides, Filter, Journal, MAX_PAGE};
use serde_json::{Value, json};
use support::*;

fn start() -> SessionStart {
    SessionStart {
        app: Version::parse("0.0.0").unwrap(),
        protocol: 1,
        os: OsFamily::Linux,
        os_major: 6,
        arch: Arch::X86_64,
        cores: 8,
        mem_gi_b: 16,
        profile: BuildProfile::Release,
    }
}

fn open(dir: &tempfile::TempDir, privacy: Value) -> Journal {
    let c = clock();
    Journal::open_with_settings(
        &events(dir),
        Some(&privacy),
        EnvOverrides::default(),
        opts(&c),
    )
    .0
}

fn state_json(dir: &tempfile::TempDir) -> Value {
    serde_json::from_slice(&std::fs::read(events(dir).join("state.json")).unwrap()).unwrap()
}

fn kinds(all: &[fin_core::events::Envelope]) -> Vec<&str> {
    all.iter().map(|e| e.kind.as_str()).collect()
}

#[test]
fn session_start_end_and_unclean() {
    let dir = tempfile::tempdir().unwrap();
    // A clean run: start, end, `openSession` cleared.
    let j = open(&dir, json!({}));
    j.start_session(start());
    j.flush().unwrap();
    assert_eq!(state_json(&dir)["openSession"], j.session().unwrap());
    assert!(j.end_session(3, Duration::from_secs(1)));
    assert!(state_json(&dir).get("openSession").is_none());
    let first = read_all(&j);
    assert_eq!(kinds(&first), ["session.start", "session.end"]);
    assert_eq!(first[0].source, Source::Finisherd);
    assert_eq!(first[1].payload["uptimeS"], 3);
    j.close();

    // A run that never ended: the next start reports it.
    let j = open(&dir, json!({}));
    j.start_session(start());
    j.flush().unwrap();
    let crashed = j.session().unwrap().to_owned();
    drop(j); // no end_session: `openSession` stays
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(state_json(&dir)["openSession"], crashed.as_str());
    let j = open(&dir, json!({}));
    j.start_session(start());
    let all = read_all(&j);
    let unclean = of_kind(&all, "session.unclean");
    assert_eq!(unclean.len(), 1);
    assert_eq!(unclean[0].payload["previous"], crashed.as_str());
    assert_eq!(
        kinds(&all[all.len() - 2..]),
        ["session.unclean", "session.start"]
    );
    // Other state keys survive.
    assert!(state_json(&dir).get("consent").is_some());
    j.close();
}

#[test]
fn session_start_waits_for_a_purpose_and_off_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(&dir, json!({ "diagnostics": "off" }));
    j.start_session(start());
    assert!(!j.end_session(1, Duration::from_secs(1)));
    j.flush().unwrap();
    assert!(!events(&dir).exists(), "off creates nothing");
    j.start_session(start());
    j.apply_settings(Some(&json!({ "diagnostics": "on" })));
    let all = read_all(&j);
    assert_eq!(kinds(&all), ["consent.changed", "session.start"]);
    j.close();
}

#[test]
fn record_value_sync_is_on_disk_when_it_returns() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(&dir, json!({}));
    let home = std::env::temp_dir().join("someone-secret");
    let payload = json!({
        "where": "finisherd", "location": "apps/finisherd/src/main.ts:12", "thirdParty": false,
        "thread": "main", "message": format!("boom at {}", home.display()),
    });
    let session = j.session().unwrap().to_owned();
    let ok = j
        .record_value_sync(
            Source::Finisherd,
            &session,
            "error.panic",
            1,
            None,
            payload,
            Duration::from_millis(250),
        )
        .unwrap();
    assert!(ok);
    let all = raw_lines(&events(&dir));
    let p = &of_kind(&all, "error.panic")[0].payload;
    assert!(p["messageHash"].as_str().unwrap().starts_with("k1:"));
    assert!(
        p.get("message").is_none(),
        "diagnostics alone keeps no message"
    );
    let bad = j.record_value_sync(
        Source::Fin,
        &session,
        "log.commit",
        1,
        None,
        json!({}),
        Duration::from_millis(250),
    );
    assert_eq!(bad.unwrap_err().code, "EVENT_INVALID");
    j.close();
}

#[test]
fn report_counts_kinds_and_projects() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(&dir, json!({}));
    j.set_project_policy(P1, Some(fin_core::events::CollectPolicy::Default));
    for _ in 0..3 {
        j.record(Source::FinNode, in_project(P1), commit(5));
    }
    let r = j.report();
    assert_eq!(r.kinds.get("log.commit"), Some(&3));
    assert_eq!(r.projects.len(), 1);
    let v = serde_json::to_value(&r).unwrap();
    assert_eq!(v["state"], "on");
    assert_eq!(v["consent"]["diagnostics"], "on");
    assert_eq!(v["projects"][P1], "default");
    j.close();
}

#[test]
fn export_pages_with_cursor_and_limit() {
    let dir = tempfile::tempdir().unwrap();
    let j = open(&dir, json!({}));
    for i in 0..7 {
        j.record(Source::FinNode, None, commit(i));
    }
    let f = Filter::default();
    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = j.export_page(&f, None, cursor.as_deref(), 3).unwrap();
        pages += 1;
        ids.extend(page.lines.iter().map(|e| e.id.as_str().to_owned()));
        match page.next {
            Some(n) => cursor = Some(n),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(ids.len(), 7);
    let mut sorted = ids.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted, ids, "oldest first, no repeats");
    assert_eq!(j.export_page(&f, None, None, 0).unwrap().lines.len(), 1);
    assert!(j.export_page(&f, None, None, 5000).unwrap().lines.len() <= MAX_PAGE);
    assert_eq!(
        j.export_page(&f, None, Some("nope"), 3).unwrap_err().code,
        "INVALID_ARGS"
    );
    j.close();
}
