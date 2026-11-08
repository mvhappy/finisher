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

use crate::conn::{MAX_LINE, TimedConn, is_frame_too_large};
use crate::deadline::{self, Deadline};
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
    /// The run's overall deadline ([`deadline`]); `None` for excluded long operations.
    deadline: Option<Deadline>,
    /// The current operation's timeout is the deadline's remainder, not `timeout`.
    limited: bool,
    /// The `fin-actor-*` headers of every request (computed once per run).
    headers: Vec<[String; 2]>,
    /// What the handshake bound (`None` only on [`Client::open`]'s unproved test connections).
    principal: Option<Value>,
    /// Why the handshake was retried once, when it was (`keyStale` after a server restart).
    retried: Option<&'static str>,
}

impl Client {
    pub fn connect(endpoint: &Endpoint, timeout: Duration) -> Result<Client, CoreError> {
        Client::connect_with(endpoint, timeout, deadline::current())
    }

    /// [`Client::connect`] outside the overall deadline (D6's recording has its own budget).
    pub fn connect_unbounded(endpoint: &Endpoint, timeout: Duration) -> Result<Client, CoreError> {
        Client::connect_with(endpoint, timeout, None)
    }

    /// Connects and proves fin's credential (`auth::connect`: one retry, else `SERVER_UNTRUSTED`).
    fn connect_with(
        endpoint: &Endpoint,
        timeout: Duration,
        deadline: Option<Deadline>,
    ) -> Result<Client, CoreError> {
        crate::auth::connect(endpoint, |ep| Client::open(ep, timeout, deadline))
    }

    /// A connection without the handshake (fin's unit tests talk to fake servers through it).
    pub(crate) fn open(
        endpoint: &Endpoint,
        timeout: Duration,
        deadline: Option<Deadline>,
    ) -> Result<Client, CoreError> {
        let down = |reason: String| {
            unavailable(json!({ "endpoint": endpoint.display(), "reason": reason }))
        };
        let wait = match deadline {
            Some(d) => timeout.min(d.at.saturating_duration_since(std::time::Instant::now())),
            None => timeout,
        };
        if wait.is_zero() {
            return Err(deadline::exceeded(&deadline.unwrap(), &endpoint.display()));
        }
        let failed = |e: io::Error| {
            if timed_out(&e) && wait < timeout {
                deadline::exceeded(&deadline.unwrap(), &endpoint.display())
            } else if timed_out(&e) {
                timeout_error(endpoint, "accept the connection", timeout)
            } else {
                down(format!("{:?}", e.kind()))
            }
        };
        // Read/write timeouts come from `TimedConn`: Windows named pipes have no OS-level ones.
        let conn = match endpoint {
            Endpoint::Socket(p) => {
                let name = p
                    .as_path()
                    .to_fs_name::<GenericFilePath>()
                    .map_err(|e| down(e.to_string()))?;
                let stream = ConnectOptions::new()
                    .name(name)
                    .wait_mode(ConnectWaitMode::Timeout(wait))
                    .connect_sync()
                    .map_err(failed)?;
                TimedConn::new(stream, timeout)
            }
            // Identification-level, least-access client end (5A Task 11, `pipe_client`).
            #[cfg(windows)]
            Endpoint::Pipe(p) => TimedConn::new(
                crate::pipe_client::connect(p, wait).map_err(failed)?,
                timeout,
            ),
            #[cfg(not(windows))]
            Endpoint::Pipe(_) => {
                return Err(down(
                    "named pipes exist only on Windows; set FIN_SERVER to a socket path".into(),
                ));
            }
        }
        .map_err(|e| down(e.to_string()))?;
        Ok(Client {
            endpoint: endpoint.clone(),
            timeout,
            conn,
            next_id: 1,
            deadline,
            limited: false,
            headers: crate::diag::actor_headers(),
            principal: None,
            retried: None,
        })
    }

    pub(crate) fn set_principal(&mut self, principal: Value, retried: Option<&'static str>) {
        self.principal = Some(principal);
        self.retried = retried;
    }

    /// The principal the server bound to this connection; `None` when fin had no credential.
    pub fn principal(&self) -> Option<&Value> {
        self.principal.as_ref()
    }

    /// Why the handshake needed its one retry (`keyStale`), if it did.
    pub fn retried(&self) -> Option<&'static str> {
        self.retried
    }

    /// This client for a long operation the overall deadline does not cover (module `deadline`).
    pub fn without_deadline(mut self) -> Client {
        self.deadline = None;
        self
    }

    /// Bounds the next operation by what is left of the deadline.
    fn arm(&mut self) -> Result<(), CoreError> {
        let Some(d) = self.deadline else {
            return Ok(());
        };
        let left = d.at.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return Err(deadline::exceeded(&d, &self.endpoint.display()));
        }
        self.limited = left < self.timeout;
        self.conn.set_timeout(left.min(self.timeout));
        Ok(())
    }

    fn io_error(&self, what: &str, e: &io::Error) -> CoreError {
        if is_frame_too_large(e) {
            return CoreError::new(
                "PROTOCOL_ERROR",
                format!("finisherd sent a frame over {MAX_LINE} bytes"),
            )
            .with_details(json!({ "endpoint": self.endpoint.display(), "max": MAX_LINE }))
            .with_fix("use fin and finisherd from the same Finisher release");
        }
        if timed_out(e) && self.limited {
            return deadline::exceeded(&self.deadline.unwrap(), &self.endpoint.display());
        }
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
        self.arm()?;
        self.conn
            .write_all(line.as_bytes())
            .map_err(|e| self.io_error("send", &e))
    }

    /// The next frame. Outer error: fin's own (transport, timeout, deadline); inner error: what
    /// the server sent did not parse.
    fn read_frame(&mut self) -> Result<Result<Frame, CoreError>, CoreError> {
        loop {
            self.arm()?;
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
                return Ok(parse_frame(line.trim_end()));
            }
        }
    }

    /// Sends one unary request and waits for its `Exit`. Stream chunks for it are acknowledged
    /// and dropped; frames for other requests are ignored.
    pub fn call(&mut self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        self.request(tag, payload)
    }

    /// [`Client::call`], keeping apart what the server answered (inner: its value or its error,
    /// a defect or an unparsable frame) from fin's own failures (outer: transport, timeout).
    /// The handshake believes no server error before the server has proved itself.
    pub fn call_answer(
        &mut self,
        tag: &str,
        payload: Value,
    ) -> Result<Result<Value, CoreError>, CoreError> {
        self.answer(tag, payload)
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
        self.answer(tag, payload)?
    }

    fn answer(&mut self, tag: &str, payload: Value) -> Result<Result<Value, CoreError>, CoreError> {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.send(&request_frame(&id, tag, &payload, &self.headers))?;
        loop {
            let frame = match self.read_frame()? {
                Ok(frame) => frame,
                Err(e) => return Ok(Err(e)),
            };
            match frame {
                Frame::Exit { request_id, result } if request_id == id => return Ok(result),
                Frame::Chunk { request_id, .. } if request_id == id => {
                    self.send(&json!({ "_tag": "Ack", "requestId": id }))?;
                }
                Frame::Defect(defect) => {
                    return Ok(Err(CoreError::new(
                        "INTERNAL",
                        "the server reported a protocol defect",
                    )
                    .with_details(json!({ "defect": defect, "tag": tag }))
                    .with_fix("report this as a bug in finisherd")));
                }
                Frame::Exit { .. } | Frame::Chunk { .. } | Frame::Pong => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
