//! `fake-claude`'s `result` shapes and refused calls as 2.1.321 writes them (7D Task 0, signed
//! in): `terminal_reason`, an absent `result` key on the interrupt's and the budget's ends,
//! `permission_denials`, a write outside the working directory refused with no prompt, and an
//! allowed Bash call run as a login shell of `$SHELL`.

mod common;

use common::*;
use serde_json::{Value, json};

const BLOCK: &str = r#"{"permissions": {"blockWritesOutsideWorkingDirectories": true}}"#;

fn denial(tool: &str, id: &Value, input: Value) -> Value {
    json!({"tool_name": tool, "tool_use_id": id, "tool_input": input})
}

#[test]
fn fake_success_and_budget_results_match_the_cli() {
    let dir = Dir::new("results");
    let script = json!({"steps": [
        {"expect": "user", "cumulativeUsage": {"inputTokens": 1, "outputTokens": 1, "costUsd": 0.0001}, "text": "one"},
        {"expect": "user", "cumulativeUsage": {"inputTokens": 1, "outputTokens": 1, "costUsd": 0.0002}},
    ]});
    let mut f = Fake::start(&dir, Some(script), &["--max-budget-usd", "0.0002"], &[]);
    f.user("one");
    f.read();
    let ok = f.read();
    assert_eq!(
        (&ok["subtype"], &ok["terminal_reason"], &ok["result"]),
        (&json!("success"), &json!("completed"), &json!("one"))
    );
    f.user("two");
    f.read();
    let r = f.read();
    assert_eq!(r["subtype"], "error_max_budget_usd");
    assert_eq!(r["is_error"], true);
    assert_eq!(r["terminal_reason"], "budget_exhausted");
    assert_eq!(r["stop_reason"], "end_turn");
    assert_eq!(r["errors"], json!(["Reached maximum budget ($0.0002)"]));
    assert!(r.get("result").is_none(), "absent, not null: {r}");
    f.finish();
}

#[test]
fn fake_interrupt_result_has_no_result_key_and_lists_the_withdrawn_prompt() {
    let dir = Dir::new("aborted");
    let ask = json!({"tool": "Bash", "input": {"command": "sleep 45"}});
    let script = json!({"steps": [{"expect": "user", "ask": ask}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    let req = f.read();
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    f.control("i-1", "interrupt");
    let r = loop {
        let v = f.read();
        if v["type"] == "result" {
            break v;
        }
    };
    assert_eq!(r["subtype"], "error_during_execution");
    assert_eq!(r["is_error"], true);
    assert!(r.get("result").is_none(), "absent, not null: {r}");
    // Verified on 2.1.321 (s2b): the prompt the interrupt withdrew is a denial of the turn.
    assert_eq!(
        r["permission_denials"],
        json!([denial(
            "Bash",
            &req["request"]["tool_use_id"],
            json!({"command": "sleep 45"})
        )])
    );
    f.finish();
}

#[test]
fn fake_outside_write_is_refused_without_a_prompt() {
    let dir = Dir::new("outside");
    let outside =
        json!({"tool": "Write", "input": {"file_path": "/elsewhere/out.txt", "content": "x"}});
    let up = json!({"tool": "Edit", "input": {"file_path": "sub/../../b.txt", "old_string": "a", "new_string": "b"}});
    let inside =
        json!({"tool": "Write", "input": {"file_path": "sub/../notes.md", "content": "x"}});
    let denied = json!({"tool": "Bash", "input": {"command": "ls"}});
    let script = json!({"steps": [{"expect": "user", "ask": outside}, {"ask": up}, {"ask": inside}, {"ask": denied}]});
    let mut f = Fake::start(&dir, Some(script), &["--settings", BLOCK], &[]);
    f.user("go");
    f.read();
    let mut refused = vec![];
    for _ in 0..2 {
        let u = f.read();
        assert_eq!(u["type"], "user", "no prompt first: {u}");
        let block = &u["message"]["content"][0];
        assert_eq!(block["is_error"], true);
        assert!(
            block["content"]
                .as_str()
                .unwrap()
                .contains("blockWritesOutsideWorkingDirectories")
        );
        assert_eq!(
            u["tool_result_meta"][0]["permission_decision"]["source"],
            "config"
        );
        refused.push(block["tool_use_id"].clone());
    }
    let req = f.read();
    assert_eq!(req["request"]["tool_name"], "Write", "a write inside asks");
    assert!(
        !refused.contains(&req["request"]["tool_use_id"]),
        "a refused call's id is its own"
    );
    f.answer(
        &req,
        json!({"behavior": "allow", "updatedInput": req["request"]["input"]}),
    );
    let req = f.read();
    assert_eq!(req["request"]["tool_name"], "Bash");
    f.answer(&req, json!({"behavior": "deny", "message": "no"}));
    let r = f.read();
    assert_eq!(r["type"], "result");
    assert_eq!(
        r["permission_denials"],
        json!([
            denial(
                "Write",
                &refused[0],
                json!({"file_path": "/elsewhere/out.txt", "content": "x"})
            ),
            denial(
                "Edit",
                &refused[1],
                json!({"file_path": "sub/../../b.txt", "old_string": "a", "new_string": "b"})
            ),
            denial(
                "Bash",
                &req["request"]["tool_use_id"],
                json!({"command": "ls"})
            ),
        ])
    );
    f.finish();
    assert_eq!(
        observed(&dir)["cliRefused"].as_array().map(Vec::len),
        Some(2)
    );

    // Without the setting the CLI asks about every write.
    let script = json!({"steps": [{"expect": "user", "ask": {"tool": "Write", "input": {"file_path": "/elsewhere/out.txt"}}}]});
    let mut f = Fake::start(&dir, Some(script), &[], &[]);
    f.user("go");
    f.read();
    assert_eq!(f.read()["request"]["subtype"], "can_use_tool");
    f.finish();
}

#[cfg(unix)]
#[test]
fn fake_bash_runs_a_login_shell_of_shell() {
    let dir = Dir::new("bash");
    let home = dir.path("home");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join(".profile"), "export FAKE_RC_PROBE=sourced\n").unwrap();
    let script = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "input": {"command": "echo \"$FAKE_RC_PROBE\""}, "run": true}}]});
    let home_s = home.to_str().unwrap();
    for (shell, want) in [(None, "sourced"), (Some("/bin/sh"), "sourced")] {
        let mut envs = vec![("HOME", home_s)];
        if let Some(s) = shell {
            envs.push(("SHELL", s));
        }
        let mut f = Fake::start(&dir, Some(script.clone()), &[], &envs);
        f.user("go");
        f.read();
        let req = f.read();
        f.answer(
            &req,
            json!({"behavior": "allow", "updatedInput": req["request"]["input"]}),
        );
        let u = f.read();
        let block = &u["message"]["content"][0];
        assert_eq!(block["tool_use_id"], req["request"]["tool_use_id"]);
        assert_eq!(
            block["content"].as_str().map(str::trim),
            Some(want),
            "the login shell sourced $HOME/.profile"
        );
        assert_eq!(block["is_error"], false);
        f.finish();
    }
}
