//! Host calls (Decision 5a) and the helper runtime (Decision 8) over in-memory pipes.

mod support;

use std::time::Duration;

use fin_jobs::helper::{Helper, HelperCtx};
use fin_jobs::*;
use serde_json::{Value, json};
use support::*;

/// Answers the next `call` with `answer(method, params)`; returns its id.
fn serve(rig: &mut Rig, answer: impl FnOnce(&str, &Value) -> Result<Value, JobError>) -> u64 {
    let WorkerMsg::Call { id, method, params } = rig.recv() else {
        panic!("a call")
    };
    let (value, error) = match answer(&method, &params) {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    rig.send(&HostMsg::Reply { id, value, error });
    id
}

#[test]
fn call_reply_round_trip() {
    let mut rig = Rig::worker(toy(|_, ctx| ctx.call("test.echo", json!({ "x": 1 }))));
    rig.recv();
    rig.send(&start("j", "toy.run", json!({})));
    let id = serve(&mut rig, |m, p| {
        assert_eq!(m, "test.echo");
        Ok(json!({ "echo": p }))
    });
    assert_eq!(id, 1, "ids start at 1");
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "j".into(),
            value: json!({ "echo": { "x": 1 } })
        }
    );
    rig.join();
}

#[test]
fn calls_from_two_threads_get_their_own_replies() {
    let mut rig = Rig::worker(toy(|_, ctx| {
        let (a, b) = (ctx.reporter(), ctx.reporter());
        let ta = std::thread::spawn(move || a.call("m", json!("a")));
        let tb = std::thread::spawn(move || b.call("m", json!("b")));
        Ok(json!([ta.join().unwrap()?, tb.join().unwrap()?]))
    }));
    rig.recv();
    rig.send(&start("j", "toy.run", json!({})));
    let mut calls = Vec::new();
    for _ in 0..2 {
        let WorkerMsg::Call { id, params, .. } = rig.recv() else {
            panic!()
        };
        calls.push((id, params));
    }
    // Replies in the reverse order.
    for (id, params) in calls.into_iter().rev() {
        rig.send(&HostMsg::Reply {
            id,
            value: Some(json!(format!("re:{}", params.as_str().unwrap()))),
            error: None,
        });
    }
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "j".into(),
            value: json!(["re:a", "re:b"])
        }
    );
    rig.join();
}

#[test]
fn call_error_reply_is_a_job_error() {
    let mut rig = Rig::worker(toy(|_, ctx| ctx.call("nope.method", json!({}))));
    rig.recv();
    rig.send(&start("j", "toy.run", json!({})));
    serve(&mut rig, |m, _| {
        Err(JobError::new("INVALID_REQUEST", "unknown method").with_details(json!({ "method": m })))
    });
    let WorkerMsg::Error { error, .. } = rig.recv() else {
        panic!()
    };
    assert_eq!(error.code, "INVALID_REQUEST");
    assert_eq!(error.details, json!({ "method": "nope.method" }));
    assert_eq!(error.retry, Retry::Never);
    rig.join();
}

#[test]
fn call_fails_when_the_host_is_gone() {
    let mut rig = Rig::worker(toy(|_, ctx| {
        let e = ctx.call("m", json!({})).unwrap_err();
        let again = ctx.call("m", json!({})).unwrap_err();
        Ok(json!([e.code, again.code]))
    }));
    rig.recv();
    rig.send(&start("j", "toy.run", json!({})));
    assert!(matches!(rig.recv(), WorkerMsg::Call { .. }));
    rig.close_stdin();
    assert_eq!(
        rig.recv(),
        WorkerMsg::Result {
            job: "j".into(),
            value: json!(["HOST_GONE", "HOST_GONE"])
        }
    );
    rig.join();
}

/// A helper that records its attach and, with `block`, serves until `shutdown`.
struct TestHelper {
    block: bool,
    call: bool,
}

impl Helper for TestHelper {
    fn kind(&self) -> HelperKind {
        HelperKind::Codec
    }

    fn id(&self) -> &str {
        "t1"
    }

    fn run(&mut self, attach: &Attach, ctx: &mut HelperCtx<'_>) -> Result<(), JobError> {
        assert_eq!(attach.job, "j1");
        assert_eq!(attach.files.len(), 1);
        if self.call {
            let v = ctx.call("helper.ping", json!({ "from": "t1" }))?;
            assert_eq!(v, json!("pong"));
        }
        let token = ctx.cancel_token();
        while self.block && !token.is_cancelled() {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }
}

fn attach() -> HostMsg {
    HostMsg::Attach(Attach {
        job: "j1".into(),
        files: vec![FileGrant {
            name: "ring.bin".into(),
            path: "/job/ring.bin".into(),
            mode: FileMode::ReadWrite,
            bytes: Some(4096),
        }],
        dirs: dirs(),
    })
}

#[test]
fn helper_waits_for_attach_and_exits_on_shutdown() {
    for block in [false, true] {
        let mut rig = Rig::helper(TestHelper { block, call: false });
        let WorkerMsg::Hello(h) = rig.recv_any() else {
            panic!()
        };
        assert_eq!(h.role, Role::Helper);
        assert_eq!(
            (h.kind, h.id.as_deref()),
            (Some(HelperKind::Codec), Some("t1"))
        );
        assert_eq!(h.envelope.as_deref(), Some(envelope_id()));
        assert!(h.family.is_empty() && h.kinds.is_empty());
        assert!(
            rig.quiet_for(Duration::from_millis(100)),
            "waits for attach"
        );
        rig.send(&attach());
        assert!(
            rig.quiet_for(Duration::from_millis(100)),
            "serves until shutdown"
        );
        rig.send(&HostMsg::Shutdown {});
        assert_eq!(rig.join_open(), 0, "block {block}");
    }
}

#[test]
fn helper_can_call() {
    let mut rig = Rig::helper(TestHelper {
        block: false,
        call: true,
    });
    rig.recv();
    rig.send(&attach());
    serve(&mut rig, |m, p| {
        assert_eq!((m, p), ("helper.ping", &json!({ "from": "t1" })));
        Ok(json!("pong"))
    });
    rig.send(&HostMsg::Shutdown {});
    assert_eq!(rig.join_open(), 0);
}

#[test]
fn helper_error_is_reported_and_exits_nonzero() {
    let mut rig = Rig::helper(TestHelper {
        block: false,
        call: true,
    });
    rig.recv();
    rig.send(&attach());
    serve(&mut rig, |_, _| Err(JobError::new("NOPE", "no")));
    let WorkerMsg::Error { job, error, .. } = rig.recv() else {
        panic!()
    };
    assert_eq!((job.as_str(), error.code.as_str()), ("j1", "NOPE"));
    assert_eq!(rig.join_open(), 1);
}

#[test]
fn helper_exits_70_when_the_host_is_gone() {
    let mut rig = Rig::helper(TestHelper {
        block: true,
        call: false,
    });
    rig.recv();
    rig.send(&attach());
    assert_eq!(rig.join(), EXIT_HOST_GONE);
}
