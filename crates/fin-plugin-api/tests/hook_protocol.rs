//! The hook protocol's types (7F plan Task 1, Decisions 1 and 2): a hook answers exactly one of
//! `allow`, `block {reason}` and `context {text}` (no `modify`: spec §9.1), the events are the
//! thirteen of spec §9.1 without `PreOp`/`PostOp`, each in the class Decision 1 gives it, and
//! a request is protocol version 1.

use fin_core::events::payloads::{AgentMode, Harness};
use fin_plugin_api::protocol::hook::{
    HOOK_PROTOCOL_VERSION, HookClass, HookEvent, HookProject, HookReply, HookRequest, HookRun,
    HookSession,
};
use serde_json::{Value, json};

fn reply(v: Value) -> Result<HookReply, serde_json::Error> {
    serde_json::from_value(v)
}

#[test]
fn reply_shapes_round_trip() {
    let cases = [
        (json!({"reply": "allow"}), HookReply::Allow {}),
        (
            json!({"reply": "block", "reason": "no trims on Fridays"}),
            HookReply::Block {
                reason: "no trims on Fridays".into(),
            },
        ),
        (
            json!({"reply": "context", "text": "the style guide says…"}),
            HookReply::Context {
                text: "the style guide says…".into(),
            },
        ),
    ];
    for (v, want) in cases {
        let got = reply(v.clone()).unwrap_or_else(|e| panic!("{v}: {e}"));
        assert_eq!(got, want);
        assert_eq!(serde_json::to_value(&got).unwrap(), v);
    }
}

#[test]
fn modify_reply_does_not_decode() {
    for v in [
        json!({"reply": "modify"}),
        json!({"reply": "modify", "payload": {"a": 1}}),
    ] {
        assert!(reply(v.clone()).is_err(), "{v}");
    }
}

#[test]
fn unknown_reply_field_refused() {
    for v in [
        json!({"reply": "allow", "reason": "x"}),
        json!({"reply": "block", "reason": "x", "text": "y"}),
        json!({"reply": "context", "text": "y", "payload": {}}),
        // A missing field, a wrong case and no tag are refused too.
        json!({"reply": "block"}),
        json!({"reply": "Allow"}),
        json!({"reason": "x"}),
    ] {
        assert!(reply(v.clone()).is_err(), "{v}");
    }
}

#[test]
fn event_names_are_pascal_case_and_complete() {
    let names: Vec<String> = HookEvent::ALL
        .iter()
        .map(|e| {
            serde_json::to_value(e)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    // `name()` and `from_name()` agree with serde (manifests and permissions use them).
    for (e, n) in HookEvent::ALL.iter().zip(&names) {
        assert_eq!(e.name(), n);
        assert_eq!(HookEvent::from_name(n), Some(*e));
    }
    assert_eq!(HookEvent::from_name("PreOp"), None);
    assert_eq!(
        names,
        [
            "SessionStart",
            "SessionEnd",
            "UserPrompt",
            "PreCommand",
            "PostCommand",
            "BranchReady",
            "PreMerge",
            "PostMerge",
            "PreRender",
            "PostRender",
            "JobDone",
            "Approval",
            "Notification",
        ]
    );
    for (e, n) in HookEvent::ALL.iter().zip(&names) {
        assert_eq!(serde_json::from_value::<HookEvent>(json!(n)).unwrap(), *e);
    }
    for cut in ["PreOp", "PostOp", "preCommand"] {
        assert!(
            serde_json::from_value::<HookEvent>(json!(cut)).is_err(),
            "{cut}"
        );
    }
}

#[test]
fn classes_are_decision_1() {
    use HookClass::*;
    use HookEvent::*;
    for e in HookEvent::ALL {
        let want = match e {
            UserPrompt | PreCommand | PreMerge | BranchReady | PreRender => BlockingBefore,
            PostCommand => BlockingAfter,
            SessionStart => Awaited,
            SessionEnd | PostMerge | PostRender | JobDone | Approval | Notification => Observe,
        };
        assert_eq!(e.class(), want, "{e:?}");
    }
}

#[test]
fn pre_merge_defaults_to_block() {
    for e in HookEvent::ALL {
        assert_eq!(
            e.default_on_timeout_blocks(),
            e == HookEvent::PreMerge,
            "{e:?}"
        );
    }
}

#[test]
fn request_v_is_1() {
    assert_eq!(HOOK_PROTOCOL_VERSION, 1);
    let req = HookRequest::new(
        HookEvent::PreCommand,
        Some(HookSession {
            id: "01J00000000000000000000000".into(),
            role: "editor".into(),
            mode: AgentMode::Draft,
            harness: Harness::Fake,
            root_branch: Some("main".into()),
        }),
        Some(HookProject {
            id: "01J00000000000000000000001".into(),
            dir: "/projects/demo".into(),
        }),
        json!({"rpc": "item.trim"}),
    );
    assert_eq!(req.v, 1);
    let v = serde_json::to_value(&req).unwrap();
    assert_eq!(
        v,
        json!({
            "event": "PreCommand",
            "v": 1,
            "session": {
                "id": "01J00000000000000000000000",
                "role": "editor",
                "mode": "draft",
                "harness": "fake",
                "rootBranch": "main"
            },
            "project": {"id": "01J00000000000000000000001", "dir": "/projects/demo"},
            "payload": {"rpc": "item.trim"}
        })
    );
    assert_eq!(serde_json::from_value::<HookRequest>(v).unwrap(), req);

    // Without a session (`UserPrompt` before `agent.session.start` opens one), no key.
    let bare = HookRequest::new(
        HookEvent::JobDone,
        None,
        Some(HookProject {
            id: "p".into(),
            dir: "/p".into(),
        }),
        Value::Null,
    );
    let v = serde_json::to_value(&bare).unwrap();
    assert!(v.get("session").is_none(), "{v}");

    // A server-scoped event (a call or a job that names no project): no `project` key, and a
    // request without one decodes.
    let server = HookRequest::new(
        HookEvent::PreCommand,
        None,
        None,
        json!({"rpc": "auth.whoami"}),
    );
    let v = serde_json::to_value(&server).unwrap();
    assert!(v.get("project").is_none(), "{v}");
    assert_eq!(
        serde_json::from_value::<HookRequest>(json!({"event": "JobDone", "v": 1, "payload": null}))
            .unwrap()
            .project,
        None
    );
    assert_eq!(serde_json::from_value::<HookRequest>(v).unwrap(), server);

    // A plugin handler's job params carry the same request.
    let run = HookRun {
        handler: "onPreCommand".into(),
        request: req.clone(),
    };
    let v = serde_json::to_value(&run).unwrap();
    assert_eq!(v["handler"], "onPreCommand");
    assert_eq!(v["request"]["v"], 1);
    assert_eq!(serde_json::from_value::<HookRun>(v).unwrap(), run);
}
