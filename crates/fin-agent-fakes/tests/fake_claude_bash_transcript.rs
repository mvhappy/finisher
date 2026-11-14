//! A `Bash` call the fake announces is in its transcript at once (as the CLI writes it), so a
//! process killed mid-call leaves a dangling call for the resume (7D Task 16: the e2e kills the
//! server while a shell call runs).

mod common;

use common::*;
use serde_json::json;

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
    assert_eq!(f.read()["type"], "result");
    assert!(f.finish().success());
    let all = observed_all(&dir);
    assert_eq!(all[1]["resume"]["dangling"], json!(["toolu_B"]));
    assert_eq!(all[1]["rerunDenied"], json!(["toolu_B"]));
}
