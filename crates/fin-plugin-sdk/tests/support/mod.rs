//! A fake host speaking the envelope's host side, over in-memory pipes (the SDK's runtime on a
//! thread) or a child's stdio (a built plugin binary). Shared with the example plugin's tests
//! (`examples/plugins/rust-tools/tests/`) by path, so it uses only crates both have.

#![allow(dead_code)]

use std::io::{BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use fin_jobs::codec::{self, Decoded, Line, MAX_LINE};
use fin_jobs::{Dirs, HostMsg, JobError, Limits, Start, StartJob, WorkerMsg};
use fin_plugin_api::protocol::PluginContext;
use serde_json::{Value, json};

const WAIT: Duration = Duration::from_secs(20);

pub struct FakeHost {
    to: Option<Box<dyn Write + Send>>,
    seq: u64,
    from: Receiver<WorkerMsg>,
}

/// A recorded host call.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub method: String,
    pub params: Value,
}

impl FakeHost {
    /// Talks to a process writing `from` and reading `to`. Every line it sends must decode
    /// strictly (only known worker types).
    pub fn new(to: Box<dyn Write + Send>, from: Box<dyn Read + Send>) -> FakeHost {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(from);
            let mut buf = Vec::new();
            let mut seq = 1;
            while let Ok(Line::Line) = codec::read_line(&mut r, &mut buf, MAX_LINE) {
                match codec::decode_worker(&buf, seq, false) {
                    Ok(Decoded::Msg(m)) => {
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                    other => panic!(
                        "bad plugin line {:?}: {other:?}",
                        String::from_utf8_lossy(&buf)
                    ),
                }
                seq += 1;
            }
        });
        FakeHost {
            to: Some(to),
            seq: 0,
            from: rx,
        }
    }

    fn write(&mut self, bytes: &[u8]) {
        let to = self.to.as_mut().expect("stdin open");
        to.write_all(bytes).unwrap();
        to.flush().unwrap();
    }

    pub fn send(&mut self, msg: &HostMsg) {
        self.seq += 1;
        let line = codec::encode(self.seq, msg).unwrap();
        self.write(&line);
    }

    /// Sends `{"v":1,"t":t,"seq":<next>, ...fields}`: any type, known or not.
    pub fn send_type(&mut self, t: &str, fields: Value) {
        self.seq += 1;
        let mut v = json!({ "v": 1, "t": t, "seq": self.seq });
        if let (Value::Object(m), Value::Object(f)) = (&mut v, fields) {
            m.extend(f);
        }
        let mut line = v.to_string().into_bytes();
        line.push(b'\n');
        self.write(&line);
    }

    /// The next message, `alive` included.
    pub fn recv_any(&mut self) -> WorkerMsg {
        self.from.recv_timeout(WAIT).expect("a message within 20 s")
    }

    /// The next message other than `alive`.
    pub fn recv(&mut self) -> WorkerMsg {
        loop {
            match self.recv_any() {
                WorkerMsg::Alive {} => continue,
                m => return m,
            }
        }
    }

    /// The `hello` (the first message).
    pub fn hello(&mut self) -> fin_jobs::Hello {
        match self.recv_any() {
            WorkerMsg::Hello(h) => h,
            m => panic!("hello first, got {m:?}"),
        }
    }

    /// Answers every `call` with `answer` until the job's `result` or `error`, which it
    /// returns with the calls made.
    pub fn serve(
        &mut self,
        mut answer: impl FnMut(&Call) -> Result<Value, JobError>,
    ) -> (WorkerMsg, Vec<Call>) {
        let mut calls = Vec::new();
        loop {
            match self.recv() {
                WorkerMsg::Call { id, method, params } => {
                    let call = Call { method, params };
                    let (value, error) = match answer(&call) {
                        Ok(v) => (Some(v), None),
                        Err(e) => (None, Some(e)),
                    };
                    calls.push(call);
                    self.send(&HostMsg::Reply { id, value, error });
                }
                m @ (WorkerMsg::Result { .. } | WorkerMsg::Error { .. }) => return (m, calls),
                m => panic!("unexpected {m:?}"),
            }
        }
    }

    /// Closes the process's stdin (the host is gone).
    pub fn close(&mut self) {
        self.to = None;
    }
}

pub fn context() -> PluginContext {
    serde_json::from_value(json!({
        "hostApi": "1.1.0",
        "plugin": { "id": "@example/rust-tools", "version": "0.1.0" },
        "grants": [{ "perm": "project:read" }, { "perm": "project:write" }, { "perm": "net", "origins": ["https://api.example.org"] }],
        "dirs": { "plugin": "/plugins/rust-tools", "tmp": "/plugins/tmp/k" },
        "locale": "en",
        "limits": { "timeoutS": 10, "messageBytes": 1_048_576, "pageBytes": 786_432 }
    }))
    .unwrap()
}

/// A `start` of `plugin.command` v1 running `command` at `rev`, with [`context`] in `extra`.
pub fn command(id: &str, name: &str, args: Value, rev: u64) -> HostMsg {
    command_with(id, name, args, rev, |_| {})
}

pub fn command_with(
    id: &str,
    name: &str,
    args: Value,
    rev: u64,
    f: impl FnOnce(&mut Start),
) -> HostMsg {
    let mut s = Start {
        job: StartJob {
            id: id.into(),
            kind: "plugin.command".into(),
            kind_version: 1,
            params: json!({ "command": name, "args": args, "project": "p1", "branch": "main", "rev": rev }),
            checkpoint: None,
            attempt: 1,
            extra: Some(serde_json::to_value(context()).unwrap()),
        },
        limits: Limits::default(),
        files: vec![],
        helpers: vec![],
        dirs: Dirs {
            job: PathBuf::from("/job"),
            tmp: PathBuf::from("/job/tmp"),
        },
        diagnostics: false,
    };
    f(&mut s);
    HostMsg::Start(s)
}

/// `project.snapshot` pages of `bytes`, `page_bytes` each, all of `rev`.
pub fn snapshot_page(bytes: &[u8], page_bytes: usize, page: u32, rev: u64) -> Value {
    let chunks: Vec<&[u8]> = bytes.chunks(page_bytes.max(1)).collect();
    let pages = chunks.len().max(1) as u32;
    let chunk = chunks.get(page as usize).copied().unwrap_or(&[]);
    json!({ "bytesB64": B64.encode(chunk), "page": page, "pages": pages, "rev": rev })
}

/// The page a `project.snapshot` call asks for.
pub fn asked_page(call: &Call) -> u32 {
    call.params.get("page").and_then(Value::as_u64).unwrap_or(0) as u32
}

/// A `net.read` reply for `body` (`params.offset`, `params.length`), `eof` at the end.
pub fn read_page(body: &[u8], params: &Value) -> Value {
    let offset = params["offset"].as_u64().unwrap() as usize;
    let length = params["length"].as_u64().unwrap() as usize;
    let end = (offset + length).min(body.len());
    let chunk = &body[offset.min(end)..end];
    json!({ "bytesB64": B64.encode(chunk), "eof": end == body.len() })
}

/// A `net.fetch` reply head.
pub fn fetch_head(status: u16, length: usize) -> Value {
    json!({
        "status": status,
        "headers": [{ "name": "Content-Type", "value": "text/html" }],
        "handle": "h1",
        "length": length
    })
}

pub fn unanswered(call: &Call) -> Result<Value, JobError> {
    panic!("no call expected, got {call:?}")
}
