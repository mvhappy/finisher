//! `fin approval wait|ls` under an agent session's credential (plan 7B Task 5, Decision 11;
//! spec §7.3): a command a rule asks about exits 8 `APPROVAL_PENDING`; `fin approval wait`
//! exits 8 again on timeout (repeatable), 0 once a person allowed it, 7 `APPROVAL_DENIED` on a
//! deny or expiry; the allowed command then goes through once. Against a real `finisherd`
//! (`support::agent_host`).

mod support;

use serde_json::json;
use support::agent_host::AgentHost;
use support::{assert_error, ok};

#[test]
fn wait_timeout_is_pending_exit_8() {
    let Some(host) = AgentHost::start("wait_timeout_is_pending_exit_8") else {
        return;
    };
    let (dir, project) = host.project();
    let s = host.open(&project, "draft");
    let ws = host.workspace(&dir, &s);
    host.request(json!({ "op": "ask", "sessionId": s.id, "command": "checkpoint" }));

    // The rule asks: pending, not denied.
    let asked = host.agent(&s, &ws, &["checkpoint", "c1"]);
    assert_error(&asked, 8, "APPROVAL_PENDING");
    let id = asked.json["error"]["details"]["approval"]
        .as_str()
        .unwrap()
        .to_string();

    // No answer yet: pending again, and again.
    for _ in 0..2 {
        let waited = host.agent(&s, &ws, &["approval", "wait", &id, "--timeout", "1"]);
        assert_error(&waited, 8, "APPROVAL_PENDING");
        assert_eq!(waited.json["error"]["details"]["approval"], id.as_str());
    }
    let listed = ok(host.agent(&s, &ws, &["approval", "ls", "--status", "open"]));
    assert_eq!(listed["approvals"][0]["id"], id.as_str(), "{listed}");

    // Allowed: exit 0 (a longer --timeout is capped at 90 s, not refused).
    host.request(json!({ "op": "answer", "id": id, "decision": "allow" }));
    let allowed = ok(host.agent(&s, &ws, &["approval", "wait", &id, "--timeout", "600"]));
    assert_eq!(allowed["approval"]["decision"], "allow", "{allowed}");

    // The grant is one-time and exact: the same command goes through once, then asks again.
    ok(host.agent(&s, &ws, &["checkpoint", "c1"]));
    let again = host.agent(&s, &ws, &["checkpoint", "c1"]);
    assert_error(&again, 8, "APPROVAL_PENDING");
    let second = again.json["error"]["details"]["approval"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(second, id);

    // Denied: exit 7.
    host.request(json!({ "op": "answer", "id": second, "decision": "deny" }));
    let denied = host.agent(&s, &ws, &["approval", "wait", &second]);
    assert_error(&denied, 7, "APPROVAL_DENIED");
    assert_eq!(denied.json["error"]["details"]["decision"], "deny");

    // Expired: exit 7 too.
    let third = host.agent(&s, &ws, &["checkpoint", "c1"]);
    assert_error(&third, 8, "APPROVAL_PENDING");
    let third = third.json["error"]["details"]["approval"]
        .as_str()
        .unwrap()
        .to_string();
    host.request(json!({ "op": "expireAll" }));
    let expired = host.agent(&s, &ws, &["approval", "wait", &third, "--timeout", "5"]);
    assert_error(&expired, 7, "APPROVAL_DENIED");
    assert_eq!(expired.json["error"]["details"]["decision"], "expired");
}

#[test]
fn approval_is_the_sessions_own() {
    let Some(host) = AgentHost::start("approval_is_the_sessions_own") else {
        return;
    };
    let (dir, project) = host.project();
    let (a, b) = (host.open(&project, "draft"), host.open(&project, "draft"));
    let (wa, wb) = (host.workspace(&dir, &a), host.workspace(&dir, &b));
    host.request(json!({ "op": "ask", "sessionId": a.id, "command": "checkpoint" }));
    let asked = host.agent(&a, &wa, &["checkpoint", "c1"]);
    assert_error(&asked, 8, "APPROVAL_PENDING");
    let id = asked.json["error"]["details"]["approval"]
        .as_str()
        .unwrap()
        .to_string();
    // Another session neither waits on nor lists it.
    let other = host.agent(&b, &wb, &["approval", "wait", &id, "--timeout", "1"]);
    assert_error(&other, 3, "APPROVAL_NOT_FOUND");
    let listed = ok(host.agent(&b, &wb, &["approval", "ls", "--session", &a.id]));
    assert_eq!(listed["approvals"], json!([]), "{listed}");
}
