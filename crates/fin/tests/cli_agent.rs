//! `fin agent spawn|wait|ls|send|stop|rollback|resume|interrupt` under an agent session's credential (plan 7B Task 6,
//! Decisions 16–21; spec §6.3, §11.2): a spawn answers `{session, branch}` at once, a wait
//! answers each child's status, branch and summary or exits 5 `WAIT_TIMEOUT` (repeatable), and a
//! session reaches its own children only. Against a real `finisherd` whose sessions run on the
//! scripted `fake` harness (`support::agent_host`, the test-hooks fin-node binary).

mod support;

use serde_json::{Value, json};
use support::agent_host::AgentHost;
use support::{assert_error, ok};

fn msg(text: &str) -> Value {
    json!({ "type": "message", "role": "assistant", "blocks": [{ "type": "text", "text": text }] })
}

/// A child that keeps working (its brief's run never ends by itself).
fn working() -> Value {
    json!({ "name": "working", "turns": [[{ "emit": [msg("working")] }, { "awaitAnswer": "never" }]] })
}

/// A child whose brief runs to its end with `summary`: it ends itself.
fn brief(summary: &str) -> Value {
    json!({ "name": "brief", "turns": [[{ "emit": [msg(summary), { "type": "turnCompleted", "stopReason": "endTurn" }] }]] })
}

fn id_of(v: &Value) -> String {
    v["session"].as_str().unwrap().to_string()
}

#[test]
fn spawn_returns_at_once_on_a_child_branch() {
    let Some(host) = AgentHost::start_fake(
        "spawn_returns_at_once_on_a_child_branch",
        &json!([working()]),
    ) else {
        return;
    };
    let (dir, project) = host.project();
    let p = host.open_fake(&project, "draft");
    let ws = host.workspace(&dir, &p);
    let spawned = ok(host.agent(
        &p,
        &ws,
        &[
            "agent",
            "spawn",
            "--role",
            "fixer",
            "--brief",
            "fix the intro",
        ],
    ));
    let child = id_of(&spawned);
    let branch = spawned["branch"].as_str().unwrap().to_string();
    assert_ne!(Some(branch.as_str()), p.root.as_deref());
    // At once: the child's brief is still running.
    let listed = ok(host.agent(&p, &ws, &["agent", "ls"]));
    assert_eq!(
        listed["children"],
        json!([{ "session": child, "role": "fixer", "mode": "draft", "status": "running", "branch": branch, "parent": p.id }]),
        "{listed}"
    );
    // A brief from a file in the workspace; one outside it is refused before it is read.
    std::fs::write(ws.join("brief.md"), "colour the night shots").unwrap();
    let second = ok(host.agent(
        &p,
        &ws,
        &[
            "agent",
            "spawn",
            "--role",
            "colourist",
            "--brief",
            "@brief.md",
            "--budget",
            "20000",
        ],
    ));
    assert_ne!(id_of(&second), child);
    let outside = host.agent(
        &p,
        &ws,
        &[
            "agent",
            "spawn",
            "--role",
            "fixer",
            "--brief",
            "@/etc/hostname",
        ],
    );
    assert_error(&outside, 7, "PATH_NOT_ALLOWED");
    // The bounds are the server's: a role the parent's may not spawn, a mode above the parent's.
    assert_error(
        &host.agent(
            &p,
            &ws,
            &["agent", "spawn", "--role", "editor", "--brief", "x"],
        ),
        3,
        "ROLE_UNKNOWN",
    );
    assert_error(
        &host.agent(
            &p,
            &ws,
            &[
                "agent", "spawn", "--role", "fixer", "--brief", "x", "--mode", "auto",
            ],
        ),
        3,
        "MODE_INVALID",
    );
    assert_error(
        &host.agent(
            &p,
            &ws,
            &[
                "agent", "spawn", "--role", "fixer", "--brief", "x", "--budget", "lots",
            ],
        ),
        2,
        "USAGE",
    );
}

#[test]
fn wait_any_send_stop_and_timeout() {
    let Some(host) = AgentHost::start_fake(
        "wait_any_send_stop_and_timeout",
        &json!([working(), brief("b done")]),
    ) else {
        return;
    };
    let (dir, project) = host.project();
    let p = host.open_fake(&project, "draft");
    let ws = host.workspace(&dir, &p);
    let a = id_of(&ok(host.agent(
        &p,
        &ws,
        &["agent", "spawn", "--role", "fixer", "--brief", "keep at it"],
    )));
    let b = id_of(&ok(host.agent(
        &p,
        &ws,
        &["agent", "spawn", "--role", "fixer", "--brief", "quick one"],
    )));
    // A still works: exit 5, and again.
    for _ in 0..2 {
        let waited = host.agent(&p, &ws, &["agent", "wait", &a, "--timeout", "1"]);
        assert_error(&waited, 5, "WAIT_TIMEOUT");
        assert_eq!(
            waited.json["error"]["details"]["children"][0]["status"],
            "running"
        );
    }
    // --any: the one that is done, with its summary.
    let any = ok(host.agent(
        &p,
        &ws,
        &["agent", "wait", &a, &b, "--any", "--timeout", "30"],
    ));
    assert_eq!(any["children"].as_array().unwrap().len(), 1, "{any}");
    assert_eq!(any["children"][0]["child"], b.as_str());
    assert_eq!(any["children"][0]["status"], "ended");
    assert_eq!(any["children"][0]["reason"], "briefDone");
    assert_eq!(any["children"][0]["summary"], "b done");
    // send steers the running one; stop ends it; then the wait for both answers.
    assert_eq!(
        ok(host.agent(&p, &ws, &["agent", "send", &a, "and the outro"])),
        json!({ "queued": false })
    );
    let stopped = ok(host.agent(&p, &ws, &["agent", "stop", &a]));
    assert_eq!(stopped["status"], "ended", "{stopped}");
    let all = ok(host.agent(&p, &ws, &["agent", "wait", &a, &b]));
    let statuses: Vec<&str> = all["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["status"].as_str().unwrap())
        .collect();
    assert_eq!(statuses, ["ended", "ended"], "{all}");
}

#[test]
fn a_session_reaches_its_own_children_only() {
    let Some(host) = AgentHost::start_fake(
        "a_session_reaches_its_own_children_only",
        &json!([working()]),
    ) else {
        return;
    };
    let (dir, project) = host.project();
    let (p, q) = (
        host.open_fake(&project, "draft"),
        host.open_fake(&project, "draft"),
    );
    let (wp, wq) = (host.workspace(&dir, &p), host.workspace(&dir, &q));
    let c = id_of(&ok(host.agent(
        &p,
        &wp,
        &["agent", "spawn", "--role", "fixer", "--brief", "x"],
    )));
    assert_error(
        &host.agent(&q, &wq, &["agent", "send", &c, "hello"]),
        3,
        "SESSION_NOT_FOUND",
    );
    assert_error(
        &host.agent(&q, &wq, &["agent", "stop", &c]),
        3,
        "SESSION_NOT_FOUND",
    );
    assert_error(
        &host.agent(&q, &wq, &["agent", "wait", &c, "--timeout", "1"]),
        3,
        "SESSION_NOT_FOUND",
    );
    assert_eq!(
        ok(host.agent(&q, &wq, &["agent", "ls"]))["children"],
        json!([])
    );
    // 7B Task 8: a rollback too, a direct child's only; a running child is busy.
    assert_error(
        &host.agent(&q, &wq, &["agent", "rollback", &c, "--to-run", "0"]),
        7,
        "SCOPE_DENIED",
    );
    assert_error(
        &host.agent(&p, &wp, &["agent", "rollback", &c, "--to-run", "0"]),
        4,
        "SESSION_BUSY",
    );
    // A plan session holds no agent.run: it spawns nothing.
    let plan = host.open_fake(&project, "plan");
    let wplan = host.workspace(&dir, &plan);
    assert_error(
        &host.agent(
            &plan,
            &wplan,
            &["agent", "spawn", "--role", "fixer", "--brief", "x"],
        ),
        7,
        "SCOPE_DENIED",
    );
}

/// A child whose every run hands two steers back (`notice {steerReturned}`) and keeps working.
fn returns() -> Value {
    let notice = json!({ "type": "notice", "name": "steerReturned",
        "details": { "harness": "fake", "steers": [{ "text": "first" }, { "text": "second" }] } });
    let turn = json!([{ "emit": [notice, msg("working")] }, { "awaitAnswer": "never" }]);
    json!({ "name": "returns", "turns": [turn, turn, turn, turn] })
}

/// 7B Decision 16 as amended (owner decision 2026-11-13): `fin agent resume` resumes a child the
/// parent interrupted itself, with text or the steers the last run handed back; any other cause
/// needs a person, whose `fin agent resume` resumes anything in reach.
#[test]
fn resume_a_child_the_parent_interrupted_only() {
    let Some(host) = AgentHost::start_fake(
        "resume_a_child_the_parent_interrupted_only",
        &json!([returns()]),
    ) else {
        return;
    };
    let (dir, project) = host.project();
    let (p, q) = (
        host.open_fake(&project, "draft"),
        host.open_fake(&project, "draft"),
    );
    let (wp, wq) = (host.workspace(&dir, &p), host.workspace(&dir, &q));
    let c = id_of(&ok(host.agent(
        &p,
        &wp,
        &["agent", "spawn", "--role", "fixer", "--brief", "x"],
    )));
    let interrupt = |by: Option<&str>| {
        let mut req = json!({ "op": "interrupt", "sessionId": c });
        if let Some(by) = by {
            req["by"] = json!(by);
        }
        host.request(req)["statusReason"].clone()
    };
    host.request(json!({ "op": "waitNode", "sessionId": c, "kind": "notice" }));
    // Another session's interrupt is refused (its own children only); the parent's is its own.
    assert_error(
        &host.agent(&q, &wq, &["agent", "interrupt", &c]),
        3,
        "SESSION_NOT_FOUND",
    );
    let stopped = ok(host.agent(&p, &wp, &["agent", "interrupt", &c]));
    assert_eq!(
        (&stopped["status"], &stopped["reason"]),
        (&json!("interrupted"), &json!("parentInterrupt")),
        "{stopped}"
    );
    // Text and --resend-returned are one or the other.
    assert_error(
        &host.agent(&p, &wp, &["agent", "resume", &c, "x", "--resend-returned"]),
        2,
        "USAGE",
    );
    // Another session may not; the parent may, with the steers handed back.
    assert_error(
        &host.agent(&q, &wq, &["agent", "resume", &c]),
        7,
        "SCOPE_DENIED",
    );
    let resumed = ok(host.agent(&p, &wp, &["agent", "resume", &c, "--resend-returned"]));
    assert_eq!(resumed["session"], c.as_str(), "{resumed}");
    assert!(resumed["run"].is_string(), "{resumed}");
    // Again, with text: each argument one steer, joined by a blank line.
    host.request(json!({ "op": "waitNode", "sessionId": c, "kind": "notice" }));
    assert_eq!(interrupt(Some(&p.id)), "parentInterrupt");
    let texted = ok(host.agent(&p, &wp, &["agent", "resume", &c, "one", "two"]));
    assert!(texted["run"].is_string(), "{texted}");
    // A person's interrupt: the parent is refused, the person resumes.
    assert_eq!(interrupt(None), "userInterrupt");
    let denied = host.agent(&p, &wp, &["agent", "resume", &c, "--resend-returned"]);
    assert_error(&denied, 7, "SCOPE_DENIED");
    assert_eq!(
        denied.json["error"]["details"]["reason"], "interruptedByOther",
        "{}",
        denied.stdout
    );
    let by_person = ok(host.fin(&dir, &["agent", "resume", &c]));
    assert_eq!(by_person["status"], "idle", "{by_person}");
    assert!(by_person.get("run").is_none(), "{by_person}");
}

#[test]
fn map_lists_agent_and_stays_under_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let run = support::fin(&tmp.path().join("sd"), tmp.path(), &[]);
    assert_eq!(run.code, 0, "{}", run.stdout);
    assert!(
        run.stdout.len().div_ceil(4) < 1500,
        "{} tokens",
        run.stdout.len().div_ceil(4)
    );
    let groups = run.json["groups"].as_array().unwrap();
    assert!(groups.iter().any(|g| g == "agent"), "{}", run.stdout);
    let line = run.json["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l.as_str().is_some_and(|l| l.starts_with("agent:")))
        .unwrap();
    assert_eq!(
        line,
        "agent: spawn wait ls send stop rollback resume interrupt"
    );
    let help = support::fin(&tmp.path().join("sd"), tmp.path(), &["help", "agent"]);
    assert_eq!(help.code, 0, "{}", help.stdout);
}
