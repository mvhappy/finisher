//! Effect RPC over NDJSON on `finisherd`'s local socket (plan decision 5), with blocking IO.
//!
//! Wire format (frames captured from the real server live in `tests/fixtures/rpc/`): one JSON
//! object per line. The client sends `{"_tag":"Request","id","tag","payload","headers":[]}`;
//! the server answers with `Exit {requestId, exit}` and, for streams, `Chunk {requestId,
//! values}` frames that must each be acknowledged with `{"_tag":"Ack","requestId"}` or the
//! server stalls. An `exit` is `{"_tag":"Success","value"}` or `{"_tag":"Failure","cause":[…]}`
//! whose reasons are `Fail {error: FinError}`, `Die {defect}` or `Interrupt {fiberId}`.

use std::io;
use std::time::Duration;

use fin_core::CoreError;
use interprocess::ConnectWaitMode;
use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{ConnectOptions, GenericFilePath};
use serde_json::{Value, json};

use crate::conn::TimedConn;
use crate::endpoint::Endpoint;
use crate::output::unavailable;

/// Connect, read and write timeout for one server conversation.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// One decoded server frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Frame {
    /// The final answer to a request: its success value or its error.
    Exit {
        request_id: String,
        result: Result<Value, CoreError>,
    },
    /// Stream items; must be acknowledged.
    Chunk {
        request_id: String,
        values: Vec<Value>,
    },
    Pong,
    /// A server-side defect not tied to one request.
    Defect(Value),
}

fn protocol(message: impl Into<String>, frame: &Value) -> CoreError {
    CoreError::new("PROTOCOL_ERROR", message)
        .with_details(json!({ "frame": frame }))
        .with_fix("use fin and finisherd from the same Finisher release")
}

/// Request ids are strings on the wire; accept numbers too.
fn request_id(frame: &Value) -> Result<String, CoreError> {
    match frame.get("requestId") {
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Number(n)) => Ok(n.to_string()),
        _ => Err(protocol("frame has no requestId", frame)),
    }
}

/// `FinError {code, message, details, fix?}` as a `CoreError`.
fn fin_error(e: &Value) -> CoreError {
    let Some(code) = e.get("code").and_then(Value::as_str) else {
        return CoreError::new("INTERNAL", "the server failed without an error code")
            .with_details(json!({ "error": e }))
            .with_fix("report this as a bug in finisherd");
    };
    CoreError {
        code: code.to_string(),
        message: e
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(code)
            .to_string(),
        details: e
            .get("details")
            .cloned()
            .filter(|d| !d.is_null())
            .unwrap_or_else(|| json!({})),
        fix: e.get("fix").and_then(Value::as_str).map(str::to_string),
    }
}

/// A `Failure` cause: the first `Fail` wins, then `Die`; any `Interrupt` means cancelled.
fn failure(cause: &Value) -> CoreError {
    let reasons: Vec<&Value> = match cause {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![cause],
        _ => Vec::new(),
    };
    let tagged = |tag: &str| {
        reasons
            .iter()
            .copied()
            .find(|r| r.get("_tag").and_then(Value::as_str) == Some(tag))
    };
    if let Some(fail) = tagged("Fail") {
        return fin_error(fail.get("error").unwrap_or(&Value::Null));
    }
    if let Some(die) = tagged("Die") {
        return CoreError::new("INTERNAL", "the server crashed while handling the request")
            .with_details(json!({ "defect": die.get("defect") }))
            .with_fix("report this as a bug in finisherd");
    }
    if tagged("Interrupt").is_some() {
        return CoreError::new("CANCELLED", "the server cancelled the request")
            .with_details(json!({}))
            .with_fix("send the request again (with the same --idem-key for writes)");
    }
    protocol("failure without a reason", cause)
}

/// Decodes an `exit` object.
pub fn decode_exit(exit: &Value) -> Result<Value, CoreError> {
    match exit.get("_tag").and_then(Value::as_str) {
        Some("Success") => Ok(exit.get("value").cloned().unwrap_or(Value::Null)),
        Some("Failure") => Err(failure(exit.get("cause").unwrap_or(&Value::Null))),
        _ => Err(protocol("exit is neither Success nor Failure", exit)),
    }
}

/// Parses one NDJSON line from the server.
pub fn parse_frame(line: &str) -> Result<Frame, CoreError> {
    let frame: Value = serde_json::from_str(line)
        .map_err(|e| protocol(format!("server sent invalid JSON: {e}"), &json!(line)))?;
    match frame.get("_tag").and_then(Value::as_str) {
        Some("Exit") => Ok(Frame::Exit {
            request_id: request_id(&frame)?,
            result: decode_exit(frame.get("exit").unwrap_or(&Value::Null)),
        }),
        Some("Chunk") => {
            let values = match frame.get("values") {
                Some(Value::Array(v)) => v.clone(),
                _ => return Err(protocol("chunk without values", &frame)),
            };
            Ok(Frame::Chunk {
                request_id: request_id(&frame)?,
                values,
            })
        }
        Some("Pong") => Ok(Frame::Pong),
        Some("Defect") => Ok(Frame::Defect(
            frame.get("defect").cloned().unwrap_or(Value::Null),
        )),
        Some("ClientProtocolError") => {
            Err(protocol("the server rejected a frame from fin", &frame))
        }
        _ => Err(protocol("unknown frame", &frame)),
    }
}

/// `headers`: `[name, value]` pairs (the actor of Decision 23a); empty when there is none.
pub fn request_frame(id: &str, tag: &str, payload: &Value, headers: &[[String; 2]]) -> Value {
    json!({ "_tag": "Request", "id": id, "tag": tag, "payload": payload, "headers": headers })
}

fn timed_out(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    )
}

fn timeout_error(endpoint: &Endpoint, what: &str, timeout: Duration) -> CoreError {
    CoreError::new(
        "SERVER_TIMEOUT",
        format!(
            "finisherd did not {what} within {} s",
            timeout.as_secs_f64()
        ),
    )
    .with_details(
        json!({ "endpoint": endpoint.display(), "timeoutMs": timeout.as_millis() as u64 }),
    )
    .with_fix("check that finisherd is responsive; retry writes with the same --idem-key")
}

/// Codes for which a sent commit may or may not have landed. `CANCELLED`: the server interrupted
/// the request (e.g. while stopping), possibly after the commit was written.
pub const IN_FLIGHT_CODES: &[&str] = &[
    "SERVER_TIMEOUT",
    "SERVER_UNAVAILABLE",
    "PROTOCOL_ERROR",
    "CANCELLED",
];

fn in_flight(mut e: CoreError, tx_id: &str) -> CoreError {
    if !IN_FLIGHT_CODES.contains(&e.code.as_str()) {
        return e;
    }
    match e.details.as_object_mut() {
        Some(map) => {
            map.insert("txId".to_string(), json!(tx_id));
        }
        None => e.details = json!({ "txId": tx_id, "detail": e.details }),
    }
    e.fix = Some(format!(
        "the commit may have landed; retry with `--idem-key {tx_id}` (after starting finisherd if it is down)"
    ));
    e
}

/// A connection to `finisherd`. One request at a time.
pub struct Client {
    endpoint: Endpoint,
    timeout: Duration,
    conn: TimedConn,
    next_id: u64,
    /// The `fin-actor-*` headers of every request (computed once per run).
    headers: Vec<[String; 2]>,
}

impl Client {
    pub fn connect(endpoint: &Endpoint, timeout: Duration) -> Result<Client, CoreError> {
        let down = |reason: String| {
            unavailable(json!({ "endpoint": endpoint.display(), "reason": reason }))
        };
        let name = match endpoint {
            Endpoint::Socket(p) => p.as_path().to_fs_name::<GenericFilePath>(),
            Endpoint::Pipe(p) if cfg!(windows) => p.as_str().to_fs_name::<GenericFilePath>(),
            Endpoint::Pipe(_) => {
                return Err(down(
                    "named pipes exist only on Windows; set FIN_SERVER to a socket path".into(),
                ));
            }
        }
        .map_err(|e| down(e.to_string()))?;
        let stream = ConnectOptions::new()
            .name(name)
            .wait_mode(ConnectWaitMode::Timeout(timeout))
            .connect_sync()
            .map_err(|e| {
                if timed_out(&e) {
                    timeout_error(endpoint, "accept the connection", timeout)
                } else {
                    down(format!("{:?}", e.kind()))
                }
            })?;
        // Read/write timeouts come from `TimedConn`: Windows named pipes have no OS-level ones.
        let conn = TimedConn::new(stream, timeout).map_err(|e| down(e.to_string()))?;
        Ok(Client {
            endpoint: endpoint.clone(),
            timeout,
            conn,
            next_id: 1,
            headers: crate::diag::actor_headers(),
        })
    }

    fn io_error(&self, what: &str, e: &io::Error) -> CoreError {
        if timed_out(e) {
            return timeout_error(&self.endpoint, what, self.timeout);
        }
        unavailable(
            json!({ "endpoint": self.endpoint.display(), "reason": format!("{what}: {:?}", e.kind()) }),
        )
    }

    fn send(&mut self, frame: &Value) -> Result<(), CoreError> {
        let mut line = frame.to_string();
        line.push('\n');
        self.conn
            .write_all(line.as_bytes())
            .map_err(|e| self.io_error("send", &e))
    }

    fn read_frame(&mut self) -> Result<Frame, CoreError> {
        loop {
            let line = self
                .conn
                .read_line()
                .map_err(|e| self.io_error("answer", &e))?;
            let n = line.len();
            if n == 0 || !line.ends_with('\n') {
                let reason = if n == 0 {
                    "the server closed the connection"
                } else {
                    "the server closed the connection mid-frame"
                };
                return Err(unavailable(
                    json!({ "endpoint": self.endpoint.display(), "reason": reason }),
                ));
            }
            if !line.trim().is_empty() {
                return parse_frame(line.trim_end());
            }
        }
    }

    /// Sends one unary request and waits for its `Exit`. Stream chunks for it are acknowledged
    /// and dropped; frames for other requests are ignored.
    pub fn call(&mut self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        self.request(tag, payload)
    }

    /// `tx.commit`. Once the request may have reached the server, a transport failure
    /// (timeout, lost connection, garbled answer) leaves the commit's fate unknown: the error
    /// then carries `details.txId` and tells the agent to retry with that idempotency key.
    pub fn commit(&mut self, payload: Value, tx_id: &str) -> Result<Value, CoreError> {
        self.request("tx.commit", payload)
            .map_err(|e| in_flight(e, tx_id))
    }

    /// Fire and forget (`cli.command`, `error.panic`): writes the request frame within `budget`
    /// and never reads its reply. The server enqueues the event before it yields, so closing
    /// the connection right after loses nothing.
    pub fn send_only(
        &mut self,
        tag: &str,
        payload: &Value,
        budget: Duration,
    ) -> Result<(), CoreError> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        let mut line = request_frame(&id, tag, payload, &self.headers).to_string();
        line.push('\n');
        self.conn
            .write_all_within(line.as_bytes(), budget)
            .map_err(|e| self.io_error("send", &e))
    }

    fn request(&mut self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.send(&request_frame(&id, tag, &payload, &self.headers))?;
        loop {
            match self.read_frame()? {
                Frame::Exit { request_id, result } if request_id == id => return result,
                Frame::Chunk { request_id, .. } if request_id == id => {
                    self.send(&json!({ "_tag": "Ack", "requestId": id }))?;
                }
                Frame::Defect(defect) => {
                    return Err(CoreError::new(
                        "INTERNAL",
                        "the server reported a protocol defect",
                    )
                    .with_details(json!({ "defect": defect, "tag": tag }))
                    .with_fix("report this as a bug in finisherd"));
                }
                Frame::Exit { .. } | Frame::Chunk { .. } | Frame::Pong => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(hello["protocolVersion"], 1);
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
        let mut client = Client::connect(&silent, Duration::from_millis(200)).unwrap();
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
        let mut client = Client::connect(&half, Duration::from_secs(5)).unwrap();
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
            Client::connect(&gone, Duration::from_millis(200))
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
        let mut client = Client::connect(&Endpoint::Socket(path), Duration::from_secs(5)).unwrap();
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
}
