//! `fake-claude`'s `result` lines against the real CLI's, key for key (recursively): the scrubbed
//! 2.1.321 lines of 7D Task 0 (`fixtures/claude-results.jsonl`: s1a's turn, s1e's clean resume,
//! s1b's interrupt mid-tool, s1f's budget stop).

mod common;

use common::*;
use serde_json::{Value, json};

fn real(case: &str) -> Value {
    include_str!("fixtures/claude-results.jsonl")
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).expect("a fixture line"))
        .find(|v| v["case"] == case)
        .map(|v| v["line"].clone())
        .unwrap_or_else(|| panic!("no fixture {case}"))
}

/// Every key path where `fake` and `real` differ: objects by their keys (per-model maps by each
/// model's keys, the model ids being the run's own), arrays by their first elements.
fn diff(fake: &Value, real: &Value, path: &str, out: &mut Vec<String>) {
    match (fake, real) {
        (Value::Object(f), Value::Object(r)) => {
            if path.ends_with("modelUsage") || path.ends_with("model_usage_since_last_result") {
                for (name, fv) in f {
                    let rv = r.values().next().unwrap_or(&Value::Null);
                    diff(fv, rv, &format!("{path}.{name}"), out);
                }
                if f.is_empty() != r.is_empty() {
                    out.push(format!("{path}: empty in one only"));
                }
                return;
            }
            for k in f.keys().filter(|k| !r.contains_key(*k)) {
                out.push(format!("{path}.{k}: only in the fake"));
            }
            for k in r.keys().filter(|k| !f.contains_key(*k)) {
                out.push(format!("{path}.{k}: missing in the fake"));
            }
            for (k, fv) in f {
                if let Some(rv) = r.get(k) {
                    diff(fv, rv, &format!("{path}.{k}"), out);
                }
            }
        }
        (Value::Array(f), Value::Array(r)) => {
            if let (Some(fv), Some(rv)) = (f.first(), r.first()) {
                diff(fv, rv, &format!("{path}[0]"), out);
            }
        }
        (Value::Null, Value::Null) => {}
        (f, r) if std::mem::discriminant(f) != std::mem::discriminant(r) => {
            out.push(format!("{path}: {f} vs {r}"));
        }
        _ => {}
    }
}

fn same_keys(fake: &Value, case: &str) {
    let real = real(case);
    let mut out = vec![];
    diff(fake, &real, "result", &mut out);
    assert!(out.is_empty(), "{case}: {out:#?}");
    for k in ["subtype", "is_error", "stop_reason", "terminal_reason"] {
        assert_eq!(fake[k], real[k], "{case}: {k}");
    }
}

fn result_of(f: &Fake) -> Value {
    loop {
        let v = f.read();
        if v["type"] == "result" {
            return v;
        }
    }
}

const TURN: f64 = 0.0003;

fn turn() -> Value {
    json!({"steps": [{"expect": "user", "text": "alpha",
        "cumulativeUsage": {"inputTokens": 2, "outputTokens": 5, "cacheCreationInputTokens": 3764, "costUsd": TURN}}]})
}

#[test]
fn result_lines_match_the_real_cli_key_for_key() {
    let dir = Dir::new("shapes");
    // s1a: a turn; s1e: the turn after a clean exit's resume.
    let mut f = Fake::start(&dir, Some(turn()), &["--session-id", UUID], &[]);
    f.user("work");
    let first = result_of(&f);
    assert!(f.finish().success());
    same_keys(&first, "s1a turn");
    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(&dir, Some(turn()), &[&resume], &[]);
    f.user("again");
    let resumed = result_of(&f);
    assert!(f.finish().success());
    same_keys(&resumed, "s1e clean resume");
    // s1f: the budget stop.
    let mut f = Fake::start(
        &dir,
        Some(turn()),
        &[&resume, "--max-budget-usd", "0.0002"],
        &[],
    );
    f.user("over");
    let stop = result_of(&f);
    same_keys(&stop, "s1f budget stop");
    assert_eq!(stop["errors"], json!(["Reached maximum budget ($0.0002)"]));
    let _ = f.finish();
}

#[test]
fn the_interrupt_mid_tool_matches_the_real_cli_key_for_key() {
    let dir = Dir::new("shapes-interrupt");
    let tool = json!({"type": "assistant", "session_id": "$session_id", "parent_tool_use_id": null,
        "message": {"id": "msg_1", "model": "claude-haiku-5-5", "role": "assistant",
            "content": [{"type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {"command": "sleep 45"}}]}});
    let script = json!({"steps": [{"expect": "user", "emit": [tool],
        "hiddenUsage": {"inputTokens": 2, "outputTokens": 168, "costUsd": TURN}}, {"sleep": 60000}]});
    let mut f = Fake::start(&dir, Some(script), &["--session-id", UUID], &[]);
    f.user("count");
    loop {
        if f.read()["type"] == "assistant" {
            break;
        }
    }
    f.control("i-1", "interrupt");
    let r = result_of(&f);
    same_keys(&r, "s1b interrupt mid-tool");
    let _ = f.finish();
}
