//! `client.rs`'s unit tests (kept apart so the module stays under the size limit).
use super::*;

fn fixture(name: &str) -> Vec<Frame> {
    let path = format!(
        "{}/tests/fixtures/rpc/{name}.response.ndjson",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| parse_frame(l).unwrap())
        .collect()
}

fn only_exit(name: &str) -> Result<Value, CoreError> {
    match fixture(name).as_slice() {
        [Frame::Exit { request_id, result }] => {
            assert_eq!(request_id, "1");
            result.clone()
        }
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn success_fixtures_decode_to_their_values() {
    let hello = only_exit("hello").unwrap();
    assert_eq!(hello["protocolVersion"], 2);
    assert!(
        hello["capabilities"]
            .as_array()
            .unwrap()
            .contains(&json!("tx.commit"))
    );
    assert_eq!(
        only_exit("project-create").unwrap()["projectId"],
        "01fixtureproject0000000000"
    );
    let committed: fin_core::CommittedTx =
        serde_json::from_value(only_exit("tx-commit").unwrap()).unwrap();
    assert_eq!(committed.rev, 1);
    assert_eq!(committed.bindings["$a"], "a1");
}

#[test]
fn failure_fixtures_decode_to_fin_errors() {
    let conflict = only_exit("conflict").unwrap_err();
    assert_eq!(conflict.code, "CONFLICT");
    assert_eq!(conflict.details["againstTx"], "tx-3");
    assert_eq!(conflict.details["txId"], "tx-late");
    assert!(conflict.fix.is_some());
    let missing = only_exit("subscribe-error").unwrap_err();
    assert_eq!(missing.code, "PROJECT_NOT_FOUND");
    assert_eq!(missing.details["projectId"], "nope");
}

#[test]
fn stream_fixture_has_chunks_then_an_interrupt() {
    let frames = fixture("ops-subscribe");
    let revs: Vec<u64> = frames
        .iter()
        .filter_map(|f| match f {
            Frame::Chunk { values, .. } => Some(
                values
                    .iter()
                    .map(|v| v["rev"].as_u64().unwrap())
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(revs, [2, 3, 4]);
    match frames.last().unwrap() {
        Frame::Exit { result: Err(e), .. } => assert_eq!(e.code, "CANCELLED"),
        other => panic!("{other:?}"),
    }
    assert_eq!(fixture("ping"), [Frame::Pong]);
}

#[test]
fn a_commit_that_may_have_landed_carries_its_tx_id() {
    let cancelled = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt"}]}}"#;
    let Frame::Exit { result: Err(e), .. } = parse_frame(cancelled).unwrap() else {
        panic!("not a failure");
    };
    for e in [e, CoreError::new("SERVER_TIMEOUT", "slow")] {
        let e = in_flight(e, "tx-9");
        assert_eq!(e.details["txId"], "tx-9", "{}", e.code);
        assert!(e.fix.unwrap().contains("--idem-key tx-9"));
    }
    let settled = in_flight(CoreError::new("CONFLICT", "no"), "tx-9");
    assert!(settled.details.get("txId").is_none());
}

#[test]
fn odd_frames_are_typed_errors_not_panics() {
    for line in [
        "",
        "{",
        "[]",
        r#"{"_tag":"Nope"}"#,
        r#"{"_tag":"Exit"}"#,
        r#"{"_tag":"Chunk","requestId":"1"}"#,
    ] {
        assert_eq!(
            parse_frame(line).unwrap_err().code,
            "PROTOCOL_ERROR",
            "{line}"
        );
    }
    let die = r#"{"_tag":"Exit","requestId":2,"exit":{"_tag":"Failure","cause":[{"_tag":"Interrupt"},{"_tag":"Die","defect":"x"}]}}"#;
    match parse_frame(die).unwrap() {
        Frame::Exit {
            request_id,
            result: Err(e),
        } => assert_eq!((request_id.as_str(), e.code.as_str()), ("2", "INTERNAL")),
        other => panic!("{other:?}"),
    }
    let bare = r#"{"_tag":"Exit","requestId":"1","exit":{"_tag":"Failure","cause":[{"_tag":"Fail","error":{"message":"m"}}]}}"#;
    assert!(
        matches!(parse_frame(bare).unwrap(), Frame::Exit { result: Err(e), .. } if e.code == "INTERNAL")
    );
}

/// Runs on Unix (socket files) and Windows (named pipes, which have no OS I/O timeouts).
#[test]
fn a_silent_server_is_a_typed_timeout_and_a_missing_one_is_unavailable() {
    use interprocess::local_socket::{Listener, ListenerOptions};
    use std::io::{BufRead, BufReader, Write};

    let dir = tempfile::tempdir().unwrap();
    let endpoint = |name: &str| {
        if cfg!(windows) {
            Endpoint::Pipe(format!(
                r"\\.\pipe\fin-client-test-{}-{name}",
                std::process::id()
            ))
        } else {
            Endpoint::Socket(dir.path().join(format!("{name}.sock")))
        }
    };
    let listen = |ep: &Endpoint| -> Listener {
        let name = match ep {
            Endpoint::Socket(p) => p.as_path().to_fs_name::<GenericFilePath>(),
            Endpoint::Pipe(p) => p.as_str().to_fs_name::<GenericFilePath>(),
        };
        ListenerOptions::new()
            .name(name.unwrap())
            .create_sync()
            .unwrap()
    };

    let silent = endpoint("silent");
    let listener = listen(&silent);
    let held = std::thread::spawn(move || listener.accept());
    let mut client = Client::open(&silent, Duration::from_millis(200), None).unwrap();
    let server_side = held.join().unwrap().unwrap();
    let hello = client.call("server.hello", json!({})).unwrap_err();
    assert_eq!(hello.code, "SERVER_TIMEOUT");
    assert!(hello.details.get("txId").is_none());
    let commit = client.commit(json!({}), "tx-9").unwrap_err();
    assert_eq!(
        (commit.code.as_str(), &commit.details["txId"]),
        ("SERVER_TIMEOUT", &json!("tx-9"))
    );
    assert!(commit.fix.unwrap().contains("retry with `--idem-key tx-9`"));
    drop(server_side);
    // The server reads the request, writes half a frame and goes away: unavailable, not a
    // protocol error, and the commit's fate is unknown.
    let half = endpoint("half");
    let listener = listen(&half);
    let server = std::thread::spawn(move || {
        let stream = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut String::new()).unwrap();
        reader
            .get_mut()
            .write_all(br#"{"_tag":"Exit","requestId":"1""#)
            .unwrap();
        reader.get_mut().flush().unwrap();
    });
    let mut client = Client::open(&half, Duration::from_secs(5), None).unwrap();
    let cut = client.commit(json!({}), "tx-10").unwrap_err();
    server.join().unwrap();
    assert_eq!(
        (cut.code.as_str(), &cut.details["txId"]),
        ("SERVER_UNAVAILABLE", &json!("tx-10"))
    );
    assert_eq!(
        cut.details["reason"],
        "the server closed the connection mid-frame"
    );
    let gone = endpoint("none");
    assert_eq!(
        Client::open(&gone, Duration::from_millis(200), None)
            .err()
            .unwrap()
            .code,
        "SERVER_UNAVAILABLE"
    );
}

/// The write of a fire-and-forget event is capped: against a server that never reads, a
/// frame the socket buffer cannot take gives up after the budget (`fin` then exits).
#[cfg(unix)]
#[test]
fn a_full_socket_buffer_costs_at_most_the_budget() {
    use interprocess::local_socket::ListenerOptions;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("never-reads.sock");
    let name = path.as_path().to_fs_name::<GenericFilePath>().unwrap();
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    let held = std::thread::spawn(move || {
        let conn = listener.accept().unwrap();
        std::thread::sleep(Duration::from_secs(3));
        drop(conn);
    });
    let mut client = Client::open(&Endpoint::Socket(path), Duration::from_secs(5), None).unwrap();
    // Far past any socket buffer (a few hundred KiB).
    let big = json!({ "filler": "a".repeat(4 << 20) });
    let started = std::time::Instant::now();
    let err = client
        .send_only("diagnostics.record", &big, Duration::from_millis(50))
        .unwrap_err();
    assert_eq!(err.code, "SERVER_TIMEOUT");
    assert!(
        started.elapsed() < Duration::from_millis(1000),
        "{:?}",
        started.elapsed()
    );
    drop(client);
    held.join().unwrap();
}
