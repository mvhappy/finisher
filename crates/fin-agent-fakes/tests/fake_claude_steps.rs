//! `fake-claude` script steps and turn boundaries: side effects, crashes, the unsigned turn,
//! steers, stdin EOF mid-turn, and the interrupt knobs.

mod common;

use std::process::Command;

use common::*;
use serde_json::json;

#[test]
fn fake_spawn_runs_and_records_exit_code() {
    let dir = Dir::new("spawn");
    let script = json!({"steps": [{"expect": "user",
        "spawn": {"argv": [BIN, "-p", "--bogus"]}, "emit": [{"type": "system", "subtype": "spawned"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(
        f.read()["subtype"],
        "spawned",
        "the spawn finished before the step went on"
    );
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let obs = observed_all(&dir);
    let parent = obs
        .iter()
        .find(|r| r["spawned"].is_array())
        .expect("the parent's record");
    assert_eq!(
        parent["spawned"],
        json!([{"argv": [BIN, "-p", "--bogus"], "code": 1}])
    );
    assert_eq!(
        obs.len(),
        2,
        "the child is a fake too and appended its own record"
    );
}

#[test]
fn fake_write_file_writes_before_emitting() {
    let dir = Dir::new("write");
    let target = dir.path("out.txt");
    let script = json!({"steps": [{"expect": "user",
        "writeFile": {"path": target, "contents": "hello"}, "emit": [{"type": "system", "subtype": "wrote"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    assert_eq!(f.read()["subtype"], "wrote");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    assert!(f.finish().success());
}

#[test]
fn fake_crash_exits_with_code_or_signal() {
    let dir = Dir::new("crash");
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "crash": {"code": 7}}]})),
        &[],
        &[],
    );
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.child.wait().unwrap().code(), Some(7));
    assert!(
        f.drain().iter().all(|l| l["type"] != "result"),
        "no result after a crash"
    );
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        let script = json!({"steps": [{"expect": "user",
            "emit": [{"type": "system", "subtype": "partial"}], "crash": {"signal": "KILL"}}]});
        let mut f = Fake::start(&dir, Some(script), &[], &[]);
        f.user("go");
        f.read();
        assert_eq!(f.read()["subtype"], "partial");
        assert_eq!(f.child.wait().unwrap().signal(), Some(9));
    }
}

#[test]
fn fake_not_logged_in_turn_matches_the_cli() {
    let dir = Dir::new("unsigned");
    let script = json!({"initialize": {"account": {"tokenSource": "none", "apiProvider": "firstParty"}},
        "steps": [{"expect": "user", "notLoggedIn": true}]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    assert_eq!(initialize(&mut f)["account"]["tokenSource"], "none");
    f.user("hi");
    assert_eq!(f.read_any()["subtype"], "init");
    let status = f.read_any();
    assert_eq!(
        (status["subtype"].as_str(), status["status"].as_str()),
        (Some("status"), Some("requesting"))
    );
    let a = f.read_any();
    assert_eq!(a["type"], "assistant");
    assert_eq!(a["message"]["model"], "<synthetic>");
    assert_eq!(a["error"], "authentication_failed");
    assert_eq!(a["is_api_error_message"], true);
    assert_eq!(
        a["message"]["content"][0]["text"],
        "Not logged in · Please run /login"
    );
    let r = f.read_any();
    assert_eq!(r["type"], "result");
    assert_eq!(r["subtype"], "success");
    assert_eq!(r["is_error"], true);
    assert_eq!(r["result"], "Not logged in · Please run /login");
    assert_eq!(r["modelUsage"], json!({}));
    assert_eq!(r["terminal_reason"], "api_error");
    assert!(r["api_error_status"].is_null() && r.get("api_error_status").is_some());
    assert_eq!(r["total_cost_usd"], 0.0);
    assert_eq!(r["queued_turn_count"], 0);
    assert_eq!(
        f.finish().code(),
        Some(1),
        "EOF after an is_error turn exits 1"
    );
}

#[test]
fn fake_unknown_line_has_a_type_no_cli_has() {
    let dir = Dir::new("unknown");
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "unknownLine": true}]})),
        &[],
        &[],
    );
    f.user("go");
    f.read();
    let l = f.read();
    assert_eq!(l["type"], "fake_future_type");
    assert!(l["session_id"].is_string());
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
}

#[test]
fn fake_steer_is_queued_and_counted() {
    let dir = Dir::new("steer");
    let script = json!({"steps": [{"expect": "user", "sleep": 400, "cumulativeUsage": {"outputTokens": 1}, "text": "first"}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("one");
    assert_eq!(f.read()["subtype"], "init");
    f.user("steer");
    let first = f.read();
    assert_eq!(
        (
            first["result"].as_str(),
            first["queued_turn_count"].as_u64()
        ),
        (Some("first"), Some(1))
    );
    let second = f.read();
    assert_eq!(
        second["type"], "result",
        "the queued line runs as the next turn"
    );
    assert_eq!(second["queued_turn_count"], 0);
    assert!(f.finish().success());
}

#[test]
fn fake_eof_mid_turn_plays_the_turn_to_its_result() {
    let dir = Dir::new("eof");
    let script = json!({"steps": [
        {"expect": "user", "sleep": 300, "emit": [{"type": "system", "subtype": "after_sleep"}]},
        {"notLoggedIn": true},
        {"expect": "user", "emit": [{"type": "system", "subtype": "never"}]},
    ]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.stdin.take();
    let st = f.child.wait().unwrap();
    assert_eq!(st.code(), Some(1), "an is_error turn then EOF exits 1");
    let lines = f.drain();
    let kinds: Vec<String> = lines
        .iter()
        .map(|l| {
            format!(
                "{}/{}",
                l["type"].as_str().unwrap_or(""),
                l["subtype"].as_str().unwrap_or("")
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "system/ui_invalidate",
            "system/init",
            "system/status",
            "system/after_sleep",
            "assistant/",
            "result/success"
        ],
        "{lines:?}"
    );
    // A turn that ends without an error exits 0.
    let mut f = Fake::start(
        &dir,
        Some(json!({"steps": [{"expect": "user", "sleep": 200}]})),
        &[],
        &[],
    );
    f.user("go");
    f.stdin.take();
    assert!(f.child.wait().unwrap().success());
    assert!(f.drain().iter().any(|l| l["type"] == "result"));
}

#[test]
fn fake_interrupt_knobs() {
    // An interrupt with no turn open: answered, no result.
    let dir = Dir::new("knobs");
    let mut f = Fake::start(&dir, None, &[], &[]);
    f.control("i-1", "interrupt");
    assert_eq!(f.read()["response"]["request_id"], "i-1");
    assert!(f.silent_for(300), "no result without a turn");
    assert!(f.finish().success());

    // ignore: answered, and the turn plays on.
    let script = json!({"onInterrupt": "ignore", "steps": [{"expect": "user", "sleep": 300,
        "emit": [{"type": "system", "subtype": "kept_going"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    f.control("i-2", "interrupt");
    assert_eq!(f.read()["response"]["request_id"], "i-2");
    assert_eq!(f.read()["subtype"], "kept_going");
    let r = f.read();
    assert_eq!(
        (r["type"].as_str(), r["is_error"].as_bool()),
        (Some("result"), Some(false))
    );
    assert!(f.finish().success());

    // result (the default): the turn ends, the process stays.
    let script = json!({"steps": [{"expect": "user", "sleep": 60000}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    f.control("i-3", "interrupt");
    f.read();
    assert_eq!(f.read()["subtype"], "error_during_execution");
    f.user("again");
    assert_eq!(f.read()["is_error"], false);
    assert!(f.finish().success());
}

#[cfg(unix)]
#[test]
fn fake_sigint_knobs() {
    let dir = Dir::new("sigknobs");
    let kill = |f: &Fake| {
        assert!(
            Command::new("kill")
                .args(["-INT", &f.child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
    };
    let script = json!({"onSigint": "exit", "steps": [{"expect": "user", "sleep": 60000}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    kill(&f);
    assert_eq!(f.read()["subtype"], "error_during_execution");
    assert_eq!(f.child.wait().unwrap().code(), Some(130));

    let script = json!({"onSigint": "ignore", "steps": [{"expect": "user", "sleep": 400,
        "emit": [{"type": "system", "subtype": "kept_going"}]}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    kill(&f);
    assert_eq!(f.read()["subtype"], "kept_going");
    assert!(f.finish().success());
}

#[test]
fn fake_spawn_in_on_allow_runs_only_when_allowed() {
    let dir = Dir::new("allowspawn");
    let step = json!({"expect": "user", "ask": {"tool": "Bash", "input": {"command": "fin view"}},
        "onAllow": {"spawn": {"argv": [BIN, "--version"]}, "emit": [{"type": "system", "subtype": "ran"}]},
        "onDeny": [{"type": "system", "subtype": "refused"}]});
    let mut f = Fake::start(&dir, Some(json!({"steps": [step.clone(), step]})), &[], &[]);
    f.user("one");
    f.read();
    let req = f.read();
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    assert_eq!(f.read()["subtype"], "refused");
    assert_eq!(f.read()["type"], "result");
    f.user("two");
    let req = f.read();
    f.answer(
        &req,
        json!({"behavior": "allow", "updatedInput": {"command": "fin view"}}),
    );
    assert_eq!(f.read()["subtype"], "ran");
    assert!(f.finish().success());
    let obs = observed_all(&dir);
    let parent = obs.iter().find(|r| r["answers"].is_array()).unwrap();
    assert_eq!(
        parent["spawned"].as_array().map(Vec::len),
        Some(1),
        "{parent}"
    );
    assert_eq!(parent["spawned"][0]["code"], 0);
    assert_eq!(parent["answers"][1]["answer"]["behavior"], "allow");
}
