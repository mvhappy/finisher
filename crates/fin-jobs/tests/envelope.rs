//! The envelope (Decisions 3–5, 5a, 8): messages, framing, the envelope id, the read budget
//! and the test kinds' params.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fin_jobs::budget::BudgetClock;
use fin_jobs::codec::{self, CodecError, Decoded, MAX_CHECKPOINT, MAX_LINE};
use fin_jobs::*;
use serde_json::{Value, json};

fn hello() -> Hello {
    Hello {
        v: PROTOCOL,
        minor: MINOR,
        role: Role::Worker,
        family: "test".into(),
        kinds: vec![KindVersion::new("test.echo", 1)],
        warm: true,
        envelope: Some(envelope_id().to_owned()),
        self_sandbox: Some("worker".into()),
        pid: 7,
        kind: None,
        id: None,
    }
}

fn host_msgs() -> Vec<HostMsg> {
    let grant = FileGrant {
        name: "out/frame.bin".into(),
        path: PathBuf::from("/srv/jobs/j1/out/frame.bin"),
        mode: FileMode::ReadWrite,
        bytes: Some(4096),
    };
    let dirs = Dirs {
        job: PathBuf::from("/srv/jobs/j1"),
        tmp: PathBuf::from("/srv/jobs/j1/tmp"),
    };
    vec![
        HostMsg::Start(Start {
            job: StartJob {
                id: "j1".into(),
                kind: "test.echo".into(),
                kind_version: 1,
                params: json!({ "value": 1 }),
                checkpoint: Some(json!({ "step": 3 })),
                attempt: 2,
                extra: Some(json!({ "x": true })),
            },
            limits: Limits {
                alive_ms: 5000,
                memory_bytes: Some(1 << 30),
                read_bytes_per_sec: Some(1 << 20),
            },
            files: vec![grant.clone()],
            helpers: vec![HelperGrant {
                id: "h1".into(),
                kind: HelperKind::Codec,
                files: vec!["out/frame.bin".into()],
            }],
            dirs: dirs.clone(),
            diagnostics: true,
        }),
        HostMsg::Cancel {
            job: "j1".into(),
            reason: CancelReason::Pause,
        },
        HostMsg::Shutdown {},
        HostMsg::Attach(Attach {
            job: "j1".into(),
            files: vec![grant],
            dirs,
        }),
        HostMsg::Reply {
            id: 3,
            value: Some(json!([1, 2])),
            error: None,
        },
        HostMsg::Reply {
            id: 4,
            value: None,
            error: Some(JobError::new("INVALID_REQUEST", "no such method")),
        },
    ]
}

fn worker_msgs() -> Vec<WorkerMsg> {
    let err = JobError::new("IO_ERROR", "disk").with_retry(Retry::Backoff);
    let mut helper = hello();
    helper.role = Role::Helper;
    helper.family = String::new();
    helper.kinds = vec![];
    helper.warm = false;
    helper.kind = Some(HelperKind::Plugin);
    helper.id = Some("p1".into());
    vec![
        WorkerMsg::Hello(hello()),
        WorkerMsg::Hello(helper),
        WorkerMsg::Begin {
            job: "j1".into(),
            item: "a".into(),
        },
        WorkerMsg::Item {
            job: "j1".into(),
            item: "a".into(),
            value: Some(json!(1)),
            error: None,
        },
        WorkerMsg::Item {
            job: "j1".into(),
            item: "b".into(),
            value: None,
            error: Some(err.clone()),
        },
        WorkerMsg::Progress(Progress {
            job: "j1".into(),
            ..Progress::new(5.0, ProgressUnit::Frames)
                .of(10.0)
                .phase("decode")
        }),
        WorkerMsg::Checkpoint {
            job: "j1".into(),
            data: json!({ "step": 4 }),
        },
        WorkerMsg::Alive {},
        WorkerMsg::Result {
            job: "j1".into(),
            value: json!({ "ok": true }),
        },
        WorkerMsg::Error {
            job: "j1".into(),
            error: err.clone(),
            item_errors: Some(vec![ItemError {
                item: "b".into(),
                error: err,
            }]),
        },
        WorkerMsg::Ready {},
        WorkerMsg::Diag {
            job: Some("j1".into()),
            kind: "media.decode".into(),
            payload: json!({ "n": 1 }),
        },
        WorkerMsg::Call {
            id: 1,
            method: "project.snapshot".into(),
            params: json!({}),
        },
    ]
}

#[test]
fn envelope_round_trips_every_message() {
    for (i, m) in host_msgs().iter().enumerate() {
        let seq = i as u64 + 1;
        let line = codec::encode(seq, m).unwrap();
        assert_eq!(line.last(), Some(&b'\n'));
        assert_eq!(&codec::decode_host(&line, seq).unwrap(), m);
        let v: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(v["v"], 1);
        assert_eq!(v["seq"], seq);
        assert_eq!(v["t"], m.type_name());
    }
    for (i, m) in worker_msgs().iter().enumerate() {
        let seq = i as u64 + 1;
        let line = codec::encode(seq, m).unwrap();
        assert_eq!(
            codec::decode_worker(&line, seq, false).unwrap(),
            Decoded::Msg(m.clone())
        );
        let v: Value = serde_json::from_slice(&line).unwrap();
        assert_eq!(v["t"], m.type_name());
    }
    // Field names are camelCase on the wire (spec §6.2).
    let start = serde_json::to_value(&host_msgs()[0]).unwrap();
    assert_eq!(start["job"]["kindVersion"], 1);
    assert_eq!(start["limits"]["aliveMs"], 5000);
    assert_eq!(start["limits"]["readBytesPerSec"], 1 << 20);
    assert_eq!(start["files"][0]["mode"], "readWrite");
    let error = serde_json::to_value(&worker_msgs()[9]).unwrap();
    assert_eq!(error["itemErrors"][0]["error"]["retry"], "backoff");
    let hello = serde_json::to_value(&worker_msgs()[0]).unwrap();
    assert_eq!(hello["selfSandbox"], "worker");
    assert_eq!(
        hello["kinds"],
        json!([{ "kind": "test.echo", "version": 1 }])
    );
    let helper = serde_json::to_value(&worker_msgs()[1]).unwrap();
    assert_eq!(helper["role"], "helper");
    assert_eq!(helper["kind"], "plugin");
    assert!(helper.get("family").is_none() && helper.get("kinds").is_none());
    // A reply's error carries no retry; it reads as `never`.
    let reply = serde_json::to_value(&host_msgs()[5]).unwrap();
    assert!(reply["error"].get("retry").is_none());
}

/// The envelope id of this tree. Changing the envelope changes it: update the golden in the
/// same commit (a bundled process built from another tree is then refused, Decision 3).
const ENVELOPE_GOLDEN: &str = "21a2bc69d2620ce6";

#[test]
fn envelope_id_is_stable_and_changes_with_the_schema() {
    let id = envelope_id();
    assert_eq!(id.len(), 16);
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        id,
        envelope_id_of(&envelope_schema()),
        "computed once, deterministic"
    );
    assert_eq!(
        id, ENVELOPE_GOLDEN,
        "the envelope changed: update the golden"
    );
    // A test-only extra field changes it.
    let mut schema = envelope_schema();
    let defs = schema["$defs"].as_object_mut().unwrap();
    defs["StartJob"]["properties"]
        .as_object_mut()
        .unwrap()
        .insert("extraForTest".into(), json!({ "type": "string" }));
    assert_ne!(envelope_id_of(&schema), id);
    // A description does not.
    let mut schema = envelope_schema();
    schema["$defs"]["StartJob"]["description"] = json!("reworded");
    assert_eq!(envelope_id_of(&schema), id);
}

fn line(v: Value) -> Vec<u8> {
    serde_json::to_vec(&v).unwrap()
}

#[test]
fn decode_refuses_long_lines_unknown_types_and_seq_gaps() {
    let long = vec![b'x'; MAX_LINE + 1];
    assert!(matches!(
        codec::decode_worker(&long, 1, false),
        Err(CodecError::TooLong { .. })
    ));
    assert!(matches!(
        codec::decode_host(&long, 1),
        Err(CodecError::TooLong { .. })
    ));
    assert!(matches!(
        codec::decode_worker(b"{nope", 1, false),
        Err(CodecError::Json(_))
    ));
    assert!(matches!(
        codec::decode_worker(b"[1]", 1, false),
        Err(CodecError::Json(_))
    ));
    let bogus = line(json!({ "v": 1, "t": "bogus", "seq": 1 }));
    assert_eq!(
        codec::decode_worker(&bogus, 1, false),
        Err(CodecError::UnknownType("bogus".into()))
    );
    assert_eq!(
        codec::decode_host(&bogus, 1),
        Err(CodecError::UnknownType("bogus".into()))
    );
    let alive = line(json!({ "v": 1, "t": "alive", "seq": 3 }));
    assert_eq!(
        codec::decode_worker(&alive, 2, false),
        Err(CodecError::Seq {
            expected: 2,
            got: 3
        })
    );
    let v2 = line(json!({ "v": 2, "t": "alive", "seq": 1 }));
    assert_eq!(
        codec::decode_worker(&v2, 1, false),
        Err(CodecError::Version { got: 2 })
    );
    // `hello` carries its own `v`; the host compares it (WORKER_PROTOCOL_MISMATCH).
    let mut h = hello();
    h.v = 2;
    let l = codec::encode(1, &WorkerMsg::Hello(h)).unwrap();
    let Ok(Decoded::Msg(WorkerMsg::Hello(back))) = codec::decode_worker(&l, 1, false) else {
        panic!("a v2 hello decodes");
    };
    assert_eq!(back.v, 2);
    // A known type with a bad shape is a JSON error; a line of exactly the cap passes.
    let bad = line(json!({ "v": 1, "t": "begin", "seq": 1, "job": 3 }));
    assert!(matches!(
        codec::decode_worker(&bad, 1, false),
        Err(CodecError::Json(_))
    ));
    let mut exact = line(json!({ "v": 1, "t": "alive", "seq": 1, "pad": "" }));
    let pad = MAX_LINE - exact.len();
    exact = line(json!({ "v": 1, "t": "alive", "seq": 1, "pad": "x".repeat(pad) }));
    assert_eq!(exact.len(), MAX_LINE);
    assert!(codec::decode_worker(&exact, 1, false).is_ok());
    // read_line drops an oversized line and goes on with the next.
    let mut input = vec![b'y'; MAX_LINE + 10];
    input.extend_from_slice(b"\n{\"a\":1}\n");
    let mut r = std::io::BufReader::new(&input[..]);
    let mut buf = Vec::new();
    assert_eq!(
        codec::read_line(&mut r, &mut buf, MAX_LINE).unwrap(),
        codec::Line::TooLong { len: MAX_LINE + 10 }
    );
    assert_eq!(
        codec::read_line(&mut r, &mut buf, MAX_LINE).unwrap(),
        codec::Line::Line
    );
    assert_eq!(buf, b"{\"a\":1}");
    assert_eq!(
        codec::read_line(&mut r, &mut buf, MAX_LINE).unwrap(),
        codec::Line::Eof
    );
}

#[test]
fn lenient_decode_returns_unknown_with_id() {
    let future = line(json!({ "v": 1, "t": "future", "seq": 1, "id": 9, "x": [] }));
    assert_eq!(
        codec::decode_worker(&future, 1, true),
        Ok(Decoded::Unknown {
            t: "future".into(),
            id: Some(9)
        })
    );
    let no_id = line(json!({ "v": 1, "t": "future", "seq": 2 }));
    assert_eq!(
        codec::decode_worker(&no_id, 2, true),
        Ok(Decoded::Unknown {
            t: "future".into(),
            id: None
        })
    );
    // Lenience is about types only: seq still counts, and known types with new optional
    // fields decode.
    assert!(matches!(
        codec::decode_worker(&no_id, 1, true),
        Err(CodecError::Seq { .. })
    ));
    let extra = line(json!({ "v": 1, "t": "ready", "seq": 1, "later": true }));
    assert_eq!(
        codec::decode_worker(&extra, 1, false),
        Ok(Decoded::Msg(WorkerMsg::Ready {}))
    );
}

#[test]
fn checkpoint_over_cap_is_refused() {
    let big = WorkerMsg::Checkpoint {
        job: "j".into(),
        data: json!("x".repeat(MAX_CHECKPOINT)),
    };
    assert!(matches!(
        codec::encode(1, &big),
        Err(CodecError::CheckpointTooLarge { .. })
    ));
    let raw = line(
        json!({ "v": 1, "t": "checkpoint", "seq": 1, "job": "j", "data": "x".repeat(MAX_CHECKPOINT) }),
    );
    assert!(matches!(
        codec::decode_worker(&raw, 1, false),
        Err(CodecError::CheckpointTooLarge { .. })
    ));
    let ok = WorkerMsg::Checkpoint {
        job: "j".into(),
        data: json!("x".repeat(MAX_CHECKPOINT - 10)),
    };
    let l = codec::encode(1, &ok).unwrap();
    assert!(codec::decode_worker(&l, 1, false).is_ok());
}

#[test]
fn generated_ts_matches() {
    // The drift gate (`scripts/check-generated-types.mjs`) compares the generated files; this
    // checks the schema names exist.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/core-types/schema/fin-core.schema.json");
    let schema: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let defs = schema["$defs"].as_object().unwrap();
    for name in [
        "HostMsg",
        "WorkerMsg",
        "Hello",
        "HelloRole",
        "Start",
        "StartJob",
        "Attach",
        "Progress",
        "JobError",
        "ItemError",
        "SelfSandbox",
        "JobDone",
        "WorkerExit",
        "TestSleepParams",
        "TestCrashParams",
        "TestWarmResult",
        "TestPluginMinorParams",
    ] {
        assert!(
            defs.contains_key(name),
            "{name} is not in the generated schema"
        );
    }
    assert!(
        defs.keys()
            .all(|k| !k.ends_with(|c: char| c.is_ascii_digit())),
        "a schema name collided (schemars added a number)"
    );
}

#[test]
fn test_kinds_are_in_the_name_table() {
    use fin_core::events::names_jobs::JOB_KINDS;
    for p in test_kinds::PARAMS {
        assert!(
            JOB_KINDS.get(p.kind).is_some(),
            "{} is not in JOB_KINDS",
            p.kind
        );
        assert_eq!(p.version, 1);
        let _ = (p.schema)();
    }
    let n = test_kinds::PARAMS.len();
    let unique: std::collections::BTreeSet<&str> =
        test_kinds::PARAMS.iter().map(|p| p.kind).collect();
    assert_eq!(unique.len(), n);
}

#[test]
fn params_validation_names_the_field() {
    let p = kinds::find(&[test_kinds::PARAMS], "test.sleep", 1).unwrap();
    assert_eq!((p.validate)(&json!({ "ms": 5 })), Ok(json!({ "ms": 5 })));
    let bad = (p.validate)(&json!({ "progressEveryMs": 1, "ms": "five" })).unwrap_err();
    assert!(bad[0].starts_with("params.ms: "), "{bad:?}");
    let missing = (p.validate)(&json!({})).unwrap_err();
    assert!(missing[0].contains("`ms`"), "{missing:?}");
    let unknown = (p.validate)(&json!({ "ms": 1, "msx": 2 })).unwrap_err();
    assert!(unknown[0].contains("msx"), "{unknown:?}");
    let crash = kinds::find(&[test_kinds::PARAMS], "test.crash", 1).unwrap();
    let defaults = (crash.validate)(&json!({ "items": ["a"], "how": "exit" })).unwrap();
    assert_eq!(defaults["poison"], json!([]));
    let how = (crash.validate)(&json!({ "items": [], "how": "explode" })).unwrap_err();
    assert!(how[0].starts_with("params.how: "), "{how:?}");
    assert!(kinds::find(&[test_kinds::PARAMS], "test.sleep", 2).is_none());
}

/// A clock whose `sleep` advances `now`.
#[derive(Default)]
struct FakeClock(Mutex<Duration>);

impl BudgetClock for FakeClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }

    fn sleep(&self, d: Duration) {
        *self.0.lock().unwrap() += d;
    }
}

#[test]
fn read_budget_limits_rate() {
    let clock = Arc::new(FakeClock::default());
    let budget = ReadBudget::with_clock(5 << 20, clock.clone());
    for _ in 0..160 {
        budget.clone().take(64 << 10); // 10 MiB in 64 KiB reads
    }
    let took = clock.now().as_secs_f64();
    assert!(
        (1.8..=2.2).contains(&took),
        "10 MiB at 5 MiB/s took {took} s"
    );
    // One large read goes into debt and sleeps it off.
    let clock = Arc::new(FakeClock::default());
    let budget = ReadBudget::with_clock(1 << 20, clock.clone());
    budget.take(3 << 20);
    let took = clock.now().as_secs_f64();
    assert!(
        (2.7..=3.0).contains(&took),
        "3 MiB at 1 MiB/s took {took} s"
    );
}
