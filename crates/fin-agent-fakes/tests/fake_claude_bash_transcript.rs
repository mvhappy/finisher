//! A `Bash` call the fake announces is in its transcript at once (as the CLI writes it), so a
//! process killed mid-call leaves a dangling call for the resume (7D Task 16: the e2e kills the
//! server while a shell call runs).

mod common;

use common::*;
use serde_json::{Value, json};

#[test]
fn fake_bash_call_killed_mid_run_is_dangling_on_resume() {
    let dir = Dir::new("bash-dangling");
    let call = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "bash": true,
        "toolUseId": "toolu_B", "input": {"command": "true"}}}]});
    let mut f = Fake::start(&dir, Some(call), &["--session-id", UUID], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "assistant");
    let req = f.read();
    assert_eq!(req["request"]["tool_use_id"], "toolu_B");
    // The host dies with the call open: no answer, the process is killed.
    f.child.kill().unwrap();
    let _ = f.child.wait();

    let plain = json!({"onResumeDangling": "reask", "steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 1}}]});
    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(&dir, Some(plain), &[&resume], &[]);
    f.user("again");
    assert_eq!(f.read()["subtype"], "init");
    let again = f.read();
    assert_eq!(again["request"]["tool_use_id"], "toolu_B");
    f.answer(&again, json!({"behavior": "deny", "message": "no"}));
    let denied = f.read();
    assert_eq!(denied["message"]["content"][0]["tool_use_id"], "toolu_B");
    assert_eq!(denied["message"]["content"][0]["is_error"], true);
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let all = observed_all(&dir);
    assert_eq!(all[1]["resume"]["dangling"], json!(["toolu_B"]));
    assert_eq!(all[1]["rerunDenied"], json!(["toolu_B"]));
}

/// Runs one answered `Bash` call (`answer`), ends the process cleanly, and resumes under `reask`:
/// the resumed process's record.
fn answered_then_resumed(tag: &str, answer: Value) -> (Value, Dir) {
    let dir = Dir::new(tag);
    let file = dir.path("side-effect.txt");
    let command = format!("echo ran >> '{}'", file.display());
    let call = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "bash": true,
        "toolUseId": "toolu_A", "input": {"command": command}}, "cumulativeUsage": {"inputTokens": 1}}]});
    let mut f = Fake::start(&dir, Some(call), &["--session-id", UUID], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "assistant");
    let req = f.read();
    f.answer(&req, answer);
    assert_eq!(f.read()["message"]["content"][0]["type"], "tool_result");
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());

    let plain = json!({"onResumeDangling": "reask", "steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 1}}]});
    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(&dir, Some(plain), &[&resume], &[]);
    f.user("again");
    assert_eq!(f.read()["subtype"], "init");
    // Straight to the turn's end: nothing is asked again.
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    (observed_all(&dir)[1].clone(), dir)
}

#[test]
fn fake_bash_allowed_call_is_not_asked_again_on_resume() {
    let (resumed, dir) = answered_then_resumed(
        "bash-allowed-resume",
        json!({"behavior": "allow", "updatedInput": {}}),
    );
    assert_eq!(resumed["resume"]["dangling"], json!([]));
    assert_eq!(resumed.get("reran"), None);
    assert_eq!(resumed.get("rerunDenied"), None);
    let ran = std::fs::read_to_string(dir.path("side-effect.txt")).unwrap();
    assert_eq!(ran, "ran\n", "the command ran once");
}

#[test]
fn fake_bash_denied_call_is_not_asked_again_on_resume() {
    let (resumed, dir) = answered_then_resumed(
        "bash-denied-resume",
        json!({"behavior": "deny", "message": "no"}),
    );
    assert_eq!(resumed["resume"]["dangling"], json!([]));
    assert_eq!(resumed.get("rerunDenied"), None);
    assert!(
        !dir.path("side-effect.txt").exists(),
        "a denied command never runs"
    );
}

/// A dangling `Bash` call asked again and allowed runs again (a duplicate side effect: what the
/// runtime must refuse) and gets its result, so a later resume finds nothing open.
#[test]
fn fake_bash_dangling_call_allowed_on_resume_runs_again() {
    let dir = Dir::new("bash-rerun");
    let file = dir.path("side-effect.txt");
    let command = format!("echo ran >> '{}'", file.display());
    let call = json!({"steps": [{"expect": "user", "ask": {"tool": "Bash", "bash": true,
        "toolUseId": "toolu_R", "input": {"command": command}}}]});
    let mut f = Fake::start(&dir, Some(call), &["--session-id", UUID], &[]);
    f.user("go");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "assistant");
    assert_eq!(f.read()["request"]["tool_use_id"], "toolu_R");
    f.child.kill().unwrap();
    let _ = f.child.wait();
    assert!(!file.exists(), "killed before its answer: never ran");

    let plain = json!({"onResumeDangling": "reask", "steps": [{"expect": "user", "cumulativeUsage": {"inputTokens": 1}}]});
    let resume = format!("--resume={UUID}");
    let mut f = Fake::start(&dir, Some(plain.clone()), &[&resume], &[]);
    f.user("again");
    assert_eq!(f.read()["subtype"], "init");
    let again = f.read();
    assert_eq!(again["request"]["subtype"], "can_use_tool");
    assert_eq!(again["request"]["tool_name"], "Bash");
    assert_eq!(again["request"]["tool_use_id"], "toolu_R");
    f.answer(&again, json!({"behavior": "allow", "updatedInput": {}}));
    let result = f.read();
    assert_eq!(result["message"]["content"][0]["tool_use_id"], "toolu_R");
    assert_eq!(result["message"]["content"][0]["is_error"], false);
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "ran\n");
    assert_eq!(observed_all(&dir)[1]["reran"], json!(["toolu_R"]));

    let mut f = Fake::start(&dir, Some(plain), &[&resume], &[]);
    f.user("once more");
    assert_eq!(f.read()["subtype"], "init");
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    assert_eq!(observed_all(&dir)[2]["resume"]["dangling"], json!([]));
}
