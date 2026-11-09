//! The SDK over in-memory pipes against a fake host (`support`): the envelope's plugin rules
//! (plan 8B Decision 21), cancel, the paged host calls and the error mapping.

mod support;

use std::thread::JoinHandle;
use std::time::Duration;

use fin_core::model::new_project;
use fin_core::{Item, ItemBody, Rational};
use fin_jobs::worker::Channel;
use fin_jobs::{HostMsg, JobError, KindVersion, Retry, Role, WorkerMsg};
use fin_plugin_sdk::command::{error_from_job, error_to_job};
use fin_plugin_sdk::prelude::*;
use fin_plugin_sdk::runtime::{self, warm_from_args};
use support::*;

#[derive(Default)]
struct TestPlugin {
    served: u32,
}

impl Plugin for TestPlugin {
    fn command(&mut self, host: &Host, call: CommandCall) -> Result<CommandOut, PluginError> {
        self.served += 1;
        let value = match call.name.as_str() {
            "echo" => json!({ "args": call.args, "served": self.served, "rev": call.rev }),
            "context" => json!({
                "locale": host.context().locale,
                "plugin": host.context().plugin.id.to_string(),
            }),
            "snapshot" => {
                let p = host.snapshot()?;
                json!({ "name": p.name, "items": p.timelines[0].tracks[0].items.len() })
            }
            "view" => json!(host.view(Some(100))?),
            "fetch" => {
                let r = host.fetch(http_get("https://api.example.org/x"))?;
                json!({ "status": r.status, "len": r.body.len(), "type": r.header("content-type"), "text": r.text() })
            }
            "caught" => match host.view(None) {
                Ok(_) => json!("no error"),
                Err(e) => serde_json::to_value(e).unwrap(),
            },
            "wait-cancel" => {
                while !host.cancelled() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Err(cancelled());
            }
            "cancel-then-call" => {
                while !host.cancelled() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                host.view(None)?;
                json!("unreachable")
            }
            "fail" => {
                let mut e = PluginError::new("MY_CODE", "it broke").with("k", 1);
                e.fix = Some("try again".into());
                return Err(e);
            }
            "log" => {
                host.log(Level::Info, "hello");
                json!("logged")
            }
            "tx" => {
                let op: Op = serde_json::from_value(
                    json!({ "op": "item.delete", "item": "i1", "mode": "lift" }),
                )
                .unwrap();
                return Ok(CommandOut::Tx { ops: vec![op] });
            }
            "panic" => panic!("boom"),
            _ => return Err(call.unknown()),
        };
        Ok(CommandOut::Data { value })
    }
}

fn start(warm: bool) -> (FakeHost, JoinHandle<i32>) {
    let (in_r, in_w) = std::io::pipe().unwrap();
    let (out_r, out_w) = std::io::pipe().unwrap();
    let chan = Channel {
        input: Box::new(in_r),
        output: Box::new(out_w),
        hard_exit: false,
        self_sandbox: None,
        pid: 7,
    };
    let run = std::thread::spawn(move || runtime::run_on::<TestPlugin>(chan, warm));
    (FakeHost::new(Box::new(in_w), Box::new(out_r)), run)
}

fn result(m: WorkerMsg) -> Value {
    match m {
        WorkerMsg::Result { value, .. } => value,
        m => panic!("a result, got {m:?}"),
    }
}

fn error(m: WorkerMsg) -> JobError {
    match m {
        WorkerMsg::Error { error, .. } => error,
        m => panic!("an error, got {m:?}"),
    }
}

/// Runs one cold command answering calls with `answer`.
fn run_one(
    name: &str,
    rev: u64,
    answer: impl FnMut(&Call) -> Result<Value, JobError>,
) -> (WorkerMsg, Vec<Call>) {
    let (mut host, run) = start(false);
    host.hello();
    host.send(&command("j1", name, json!({}), rev));
    let out = host.serve(answer);
    assert_eq!(
        run.join().unwrap(),
        0,
        "a cold plugin exits 0 after its job"
    );
    out
}

#[test]
fn sdk_speaks_the_envelope() {
    // Cold: hello, one job, its result, exit 0.
    let (mut host, run) = start(false);
    let h = host.hello();
    assert_eq!(
        (h.v, h.minor, h.role, h.pid),
        (1, fin_plugin_sdk::MINOR, Role::Worker, 7)
    );
    assert_eq!(h.family, "plugin");
    assert_eq!(h.kinds, [KindVersion::new("plugin.command", 1)]);
    assert!(!h.warm);
    assert_eq!(h.envelope, None, "a plugin sends no envelope id");
    host.send(&command("j1", "echo", json!({ "x": 1 }), 5));
    let v = result(host.recv());
    assert_eq!(
        v,
        json!({ "kind": "data", "value": { "args": { "x": 1 }, "served": 1, "rev": 5 } })
    );
    assert_eq!(run.join().unwrap(), 0);

    // Warm: `ready` after each job, one plugin value across jobs, `shutdown` exits 0.
    let (mut host, run) = start(true);
    assert!(host.hello().warm);
    for id in ["j1", "j2"] {
        host.send(&command(id, "echo", json!({}), 1));
        assert!(matches!(host.recv(), WorkerMsg::Result { job, .. } if job == id));
        assert_eq!(host.recv(), WorkerMsg::Ready {});
    }
    host.send(&command("j3", "echo", json!({}), 1));
    assert_eq!(result(host.recv())["value"]["served"], 3);
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    host.send(&HostMsg::Shutdown {});
    assert_eq!(run.join().unwrap(), 0);
}

#[test]
fn warm_is_a_trailing_argument() {
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(warm_from_args(args(&["--warm"])));
    assert!(warm_from_args(args(&["x", "--warm"])));
    assert!(!warm_from_args(args(&["--warm", "x"])));
    assert!(!warm_from_args(args(&[])));
}

#[test]
fn unknown_host_message_type_is_not_fatal() {
    let (mut host, run) = start(true);
    host.hello();
    host.send_type("future.notice", json!({}));
    host.send_type("future.request", json!({ "id": 3, "payload": [1, 2] }));
    host.send(&command("j1", "echo", json!({}), 1));
    assert!(matches!(host.recv(), WorkerMsg::Result { .. }));
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    host.send_type("future.notice", json!({}));
    host.send(&command("j2", "echo", json!({}), 1));
    assert!(matches!(host.recv(), WorkerMsg::Result { job, .. } if job == "j2"));
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    host.close();
    assert_eq!(run.join().unwrap(), fin_jobs::EXIT_HOST_GONE);
}

#[test]
fn cancel_reaches_command() {
    let (mut host, run) = start(true);
    host.hello();
    host.send(&command("j1", "wait-cancel", json!({}), 1));
    std::thread::sleep(Duration::from_millis(20));
    host.send(&HostMsg::Cancel {
        job: "j1".into(),
        reason: fin_jobs::CancelReason::Cancel,
    });
    assert_eq!(error(host.recv()).code, "JOB_CANCELLED");
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    // A host call after the cancel never reaches the host.
    host.send(&command("j2", "cancel-then-call", json!({}), 1));
    host.send(&HostMsg::Cancel {
        job: "j2".into(),
        reason: fin_jobs::CancelReason::Cancel,
    });
    let (m, calls) = host.serve(unanswered);
    assert_eq!(error(m).code, "JOB_CANCELLED");
    assert!(calls.is_empty());
    host.close();
    run.join().unwrap();
}

fn project_bytes(items: usize) -> Vec<u8> {
    let mut p = new_project("p1".into(), "Talk".into(), Rational::from_int(25)).unwrap();
    for i in 0..items {
        let at = Rational::from_int(i as i64);
        p.timelines[0].tracks[0].items.push(Item::new(
            format!("i{i}"),
            at,
            Rational::from_int(1),
            ItemBody::Adjustment {},
        ));
    }
    serde_json::to_vec(&p).unwrap()
}

#[test]
fn snapshot_pages_join() {
    let bytes = project_bytes(20);
    let (m, calls) = run_one("snapshot", 9, |c| {
        assert_eq!(c.method, "project.snapshot");
        Ok(snapshot_page(&bytes, 100, asked_page(c), 9))
    });
    assert_eq!(result(m)["value"], json!({ "name": "Talk", "items": 20 }));
    let pages = bytes.len().div_ceil(100);
    assert!(pages > 3);
    assert_eq!(calls.len(), pages);
    assert_eq!(calls[0].params, json!({}), "page 0 is asked without `page`");
    for (i, c) in calls.iter().enumerate().skip(1) {
        assert_eq!(c.params, json!({ "page": i }));
    }

    // A page of another rev, or a first page not of the job's rev, is an error.
    for bad_page in [0, 2] {
        let (m, calls) = run_one("snapshot", 9, |c| {
            let page = asked_page(c);
            let rev = if page == bad_page { 10 } else { 9 };
            Ok(snapshot_page(&bytes, 100, page, rev))
        });
        let e = error(m);
        assert_eq!(e.code, "PLUGIN_PROTOCOL");
        assert_eq!(e.details["reason"], "revMismatch");
        assert_eq!(e.details["page"], bad_page);
        assert_eq!(
            calls.len(),
            bad_page as usize + 1,
            "no page is asked after it"
        );
    }

    // A page count that changes is an error.
    let (m, _) = run_one("snapshot", 9, |c| {
        let mut v = snapshot_page(&bytes, 100, asked_page(c), 9);
        if asked_page(c) == 1 {
            v["pages"] = json!(99);
        }
        Ok(v)
    });
    assert_eq!(error(m).details["reason"], "pages");
}

#[test]
fn fetch_reads_body_pages() {
    let body: Vec<u8> = (0..2500u32).map(|i| b'a' + (i % 26) as u8).collect();
    let small_pages = |s: &mut fin_jobs::Start| {
        s.job.extra.as_mut().unwrap()["limits"]["pageBytes"] = json!(1000);
    };
    let (mut host, run) = start(false);
    host.hello();
    host.send(&command_with("j1", "fetch", json!({}), 1, small_pages));
    let (m, calls) = host.serve(|c| match c.method.as_str() {
        "net.fetch" => Ok(fetch_head(200, body.len())),
        "net.read" => Ok(read_page(&body, &c.params)),
        m => panic!("{m}"),
    });
    run.join().unwrap();
    let v = result(m)["value"].clone();
    assert_eq!(
        (v["status"].clone(), v["len"].clone()),
        (json!(200), json!(2500))
    );
    assert_eq!(v["type"], "text/html");
    assert_eq!(v["text"], String::from_utf8(body.clone()).unwrap());
    assert_eq!(
        calls[0].params,
        json!({ "request": { "method": "GET", "url": "https://api.example.org/x" } })
    );
    let reads: Vec<_> = calls[1..]
        .iter()
        .map(|c| (c.params["offset"].clone(), c.params["length"].clone()))
        .collect();
    assert_eq!(
        reads,
        [
            (json!(0), json!(1000)),
            (json!(1000), json!(1000)),
            (json!(2000), json!(500))
        ]
    );
    assert!(calls[1..].iter().all(|c| c.params["handle"] == "h1"));

    // An empty body is not read.
    let (m, calls) = run_one("fetch", 1, |c| match c.method.as_str() {
        "net.fetch" => Ok(fetch_head(204, 0)),
        m => panic!("{m}"),
    });
    assert_eq!(result(m)["value"]["len"], 0);
    assert_eq!(calls.len(), 1);

    // A body that ends before its length is an error.
    let (m, _) = run_one("fetch", 1, |c| match c.method.as_str() {
        "net.fetch" => Ok(fetch_head(200, 10)),
        _ => Ok(read_page(b"short", &c.params)),
    });
    let e = error(m);
    assert_eq!(
        (e.code.as_str(), e.details["reason"].clone()),
        ("PLUGIN_PROTOCOL", json!("body"))
    );
}

#[test]
fn host_call_errors_reach_the_command() {
    let (m, _) = run_one("caught", 1, |_| {
        Err(JobError::new("PLUGIN_PERMISSION_DENIED", "no project:read")
            .with_details(json!({ "perm": "project:read", "fix": "grant it" })))
    });
    assert_eq!(
        result(m)["value"],
        json!({
            "code": "PLUGIN_PERMISSION_DENIED",
            "message": "no project:read",
            "details": { "perm": "project:read" },
            "fix": "grant it"
        })
    );
    // Propagated with `?`, it is the job's error as the host sent it.
    let (m, _) = run_one("view", 1, |_| {
        Err(JobError::new("EGRESS_DENIED", "no").with_details(json!({ "origin": "x" })))
    });
    let e = error(m);
    assert_eq!(
        (e.code.as_str(), e.details.clone()),
        ("EGRESS_DENIED", json!({ "origin": "x" }))
    );
}

#[test]
fn command_errors_map_to_job_errors() {
    let (m, _) = run_one("fail", 1, unanswered);
    let e = error(m);
    assert_eq!(e.code, "MY_CODE");
    assert_eq!(e.message, "it broke");
    assert_eq!(e.details, json!({ "k": 1, "fix": "try again" }));
    assert_eq!(e.retry, Retry::Never, "a command is never retried");

    let (m, _) = run_one("no-such", 1, unanswered);
    let e = error(m);
    assert_eq!(e.code, "PLUGIN_COMMAND_ARGS");
    assert_eq!(e.details["command"], "no-such");
    assert_eq!(e.details["problems"][0]["reason"], "unknownCommand");

    // Both directions round-trip.
    let mut p = PluginError::new("X", "m").with("a", json!([1]));
    p.fix = Some("f".into());
    assert_eq!(error_from_job(error_to_job(p.clone())), p);
    let odd = error_from_job(JobError::new("X", "m").with_details(json!("text")));
    assert_eq!(odd.details["value"], "text");
}

type Edit = dyn FnOnce(&mut fin_jobs::Start);

#[test]
fn bad_jobs_are_protocol_errors() {
    let (mut host, run) = start(true);
    host.hello();
    let cases: [(&str, Box<Edit>); 3] = [
        ("kind", Box::new(|s| s.job.kind = "plugin.hook".into())),
        (
            "params",
            Box::new(|s| s.job.params = json!({ "command": 1 })),
        ),
        ("context", Box::new(|s| s.job.extra = None)),
    ];
    for (i, (reason, f)) in cases.into_iter().enumerate() {
        host.send(&command_with(&format!("j{i}"), "echo", json!({}), 1, f));
        let e = error(host.recv());
        assert_eq!(
            (e.code.as_str(), e.details["reason"].clone()),
            ("PLUGIN_PROTOCOL", json!(reason))
        );
        assert_eq!(host.recv(), WorkerMsg::Ready {});
    }
    host.close();
    run.join().unwrap();
}

#[test]
fn results_context_and_log() {
    let (m, _) = run_one("tx", 1, unanswered);
    assert_eq!(
        result(m),
        json!({ "kind": "tx", "ops": [{ "op": "item.delete", "item": "i1", "mode": "lift" }] })
    );
    let (m, _) = run_one("context", 1, unanswered);
    assert_eq!(
        result(m)["value"],
        json!({ "locale": "en", "plugin": "@example/rust-tools" })
    );
    // `log` is sent, and a refused one is not the command's failure.
    let (m, calls) = run_one("log", 1, |_| {
        Err(JobError::new("RATE_LIMITED", "slow down"))
    });
    assert_eq!(result(m)["value"], "logged");
    assert_eq!(calls[0].method, "log");
    assert_eq!(calls[0].params, json!({ "level": "info", "msg": "hello" }));
}

#[test]
fn a_panic_is_a_crash() {
    let (mut host, run) = start(true);
    host.hello();
    host.send(&command("j1", "panic", json!({}), 1));
    assert_eq!(error(host.recv()).code, "WORKER_CRASHED");
    assert_eq!(run.join().unwrap(), fin_jobs::worker::EXIT_PANIC);
}
