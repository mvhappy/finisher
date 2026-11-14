//! Framing (Decision 4): UTF-8 NDJSON, one message per line, `{"v":1,"t":"<type>","seq":n, …}`.
//! Lines ≤ [`MAX_LINE`]; a `checkpoint`'s `data` ≤ [`MAX_CHECKPOINT`]; `seq` starts at 1 in
//! each direction and grows by exactly 1. Both sides use this module: the runtimes decode
//! [`HostMsg`] and encode [`WorkerMsg`], the host the other way round.

use std::fmt;
use std::io::{self, BufRead};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::PROTOCOL;
use crate::msg_host::HostMsg;
use crate::msg_worker::WorkerMsg;

/// The longest line, without its newline.
pub const MAX_LINE: usize = 1 << 20;
/// The largest serialised `checkpoint.data`.
pub const MAX_CHECKPOINT: usize = 256 << 10;

/// Why a line was refused. Every variant is a protocol error (`protocolError`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    /// The line passes [`MAX_LINE`].
    TooLong { len: usize },
    /// Not JSON, not an object, or not the message its `t` names.
    Json(String),
    /// A `t` this side does not know (and the lenient rule does not apply).
    UnknownType(String),
    /// `seq` is not the next one.
    Seq { expected: u64, got: u64 },
    /// `v` is not [`PROTOCOL`] (on a message other than `hello`, which carries its own).
    Version { got: u64 },
    /// A `checkpoint` whose `data` passes [`MAX_CHECKPOINT`].
    CheckpointTooLarge { bytes: usize },
}

impl CodecError {
    /// A short name for `WORKER_CRASHED {protocol}` and logs.
    pub fn what(&self) -> &'static str {
        match self {
            CodecError::TooLong { .. } => "long",
            CodecError::Json(_) => "json",
            CodecError::UnknownType(_) => "unknownType",
            CodecError::Seq { .. } => "seq",
            CodecError::Version { .. } => "version",
            CodecError::CheckpointTooLarge { .. } => "checkpoint",
        }
    }
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CodecError::TooLong { len } => write!(f, "line of {len} bytes passes {MAX_LINE}"),
            CodecError::Json(e) => write!(f, "bad message: {e}"),
            CodecError::UnknownType(t) => write!(f, "unknown message type {t:?}"),
            CodecError::Seq { expected, got } => write!(f, "seq {got}, expected {expected}"),
            CodecError::Version { got } => write!(f, "protocol v{got}, expected v{PROTOCOL}"),
            CodecError::CheckpointTooLarge { bytes } => {
                write!(f, "checkpoint of {bytes} bytes passes {MAX_CHECKPOINT}")
            }
        }
    }
}

impl std::error::Error for CodecError {}

/// A decoded worker line: a known message, or (lenient decoding only, Decision 3) a message
/// of a type this side does not know, with its `id` if it has one.
#[derive(Debug, Clone, PartialEq)]
pub enum Decoded<M> {
    Msg(M),
    Unknown { t: String, id: Option<u64> },
}

/// Serialises `msg` as one line: adds `v` (unless the message carries it, as `hello` does)
/// and `seq`, and appends `\n`. Refuses a line over [`MAX_LINE`] and a checkpoint over
/// [`MAX_CHECKPOINT`].
pub fn encode<M: Serialize>(seq: u64, msg: &M) -> Result<Vec<u8>, CodecError> {
    let mut value = serde_json::to_value(msg).map_err(|e| CodecError::Json(e.to_string()))?;
    let Value::Object(map) = &mut value else {
        return Err(CodecError::Json("a message serialises as an object".into()));
    };
    check_checkpoint(map)?;
    map.entry("v").or_insert(Value::from(PROTOCOL));
    map.insert("seq".into(), Value::from(seq));
    let mut line = serde_json::to_vec(&value).map_err(|e| CodecError::Json(e.to_string()))?;
    if line.len() > MAX_LINE {
        return Err(CodecError::TooLong { len: line.len() });
    }
    line.push(b'\n');
    Ok(line)
}

fn check_checkpoint(map: &Map<String, Value>) -> Result<(), CodecError> {
    if map.get("t").and_then(Value::as_str) != Some("checkpoint") {
        return Ok(());
    }
    let bytes = map
        .get("data")
        .map(|d| serde_json::to_vec(d).map(|v| v.len()).unwrap_or(usize::MAX))
        .unwrap_or(0);
    if bytes > MAX_CHECKPOINT {
        return Err(CodecError::CheckpointTooLarge { bytes });
    }
    Ok(())
}

/// The object of a line, its `t` and its checked `seq` and `v`.
fn frame(line: &[u8], expect_seq: u64) -> Result<(Map<String, Value>, String), CodecError> {
    let line = trim_newline(line);
    if line.len() > MAX_LINE {
        return Err(CodecError::TooLong { len: line.len() });
    }
    let value: Value = serde_json::from_slice(line).map_err(|e| CodecError::Json(e.to_string()))?;
    let Value::Object(map) = value else {
        return Err(CodecError::Json("not an object".into()));
    };
    let t = match map.get("t") {
        Some(Value::String(t)) => t.clone(),
        _ => return Err(CodecError::Json("no string `t`".into())),
    };
    let seq = map
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or_else(|| CodecError::Json("no integer `seq`".into()))?;
    if seq != expect_seq {
        return Err(CodecError::Seq {
            expected: expect_seq,
            got: seq,
        });
    }
    if t != "hello" {
        let v = map
            .get("v")
            .and_then(Value::as_u64)
            .ok_or_else(|| CodecError::Json("no integer `v`".into()))?;
        if v != u64::from(PROTOCOL) {
            return Err(CodecError::Version { got: v });
        }
    }
    Ok((map, t))
}

fn trim_newline(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// Decodes a worker's (or helper's) line. With `lenient_minor` (a plugin process whose minor
/// is above the host's, Decision 3) an unknown `t` is [`Decoded::Unknown`]; otherwise it is
/// [`CodecError::UnknownType`].
pub fn decode_worker(
    line: &[u8],
    expect_seq: u64,
    lenient_minor: bool,
) -> Result<Decoded<WorkerMsg>, CodecError> {
    let (map, t) = frame(line, expect_seq)?;
    if !WorkerMsg::TYPES.contains(&t.as_str()) {
        if lenient_minor {
            let id = map.get("id").and_then(Value::as_u64);
            return Ok(Decoded::Unknown { t, id });
        }
        return Err(CodecError::UnknownType(t));
    }
    check_checkpoint(&map)?;
    let msg =
        serde_json::from_value(Value::Object(map)).map_err(|e| CodecError::Json(e.to_string()))?;
    Ok(Decoded::Msg(msg))
}

/// Decodes a host's line (the runtimes).
pub fn decode_host(line: &[u8], expect_seq: u64) -> Result<HostMsg, CodecError> {
    let (map, t) = frame(line, expect_seq)?;
    if !HostMsg::TYPES.contains(&t.as_str()) {
        return Err(CodecError::UnknownType(t));
    }
    serde_json::from_value(Value::Object(map)).map_err(|e| CodecError::Json(e.to_string()))
}

/// Decodes a host's line for a plugin process (plan 8B Decision 21): a `t` this side does not
/// know (from a newer host) is [`Decoded::Unknown`], not an error. Everything else is checked
/// as in [`decode_host`].
pub fn decode_host_lenient(line: &[u8], expect_seq: u64) -> Result<Decoded<HostMsg>, CodecError> {
    let (map, t) = frame(line, expect_seq)?;
    if !HostMsg::TYPES.contains(&t.as_str()) {
        let id = map.get("id").and_then(Value::as_u64);
        return Ok(Decoded::Unknown { t, id });
    }
    serde_json::from_value(Value::Object(map))
        .map(Decoded::Msg)
        .map_err(|e| CodecError::Json(e.to_string()))
}

/// What [`read_line`] read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line {
    /// A line is in the buffer (without its newline; a last line without one counts).
    Line,
    /// The line passed the cap; the rest of it was read and dropped.
    TooLong { len: usize },
    /// End of the stream.
    Eof,
}

/// Reads one line into `buf` (cleared first) without ever holding more than `max` bytes of
/// it. Retries `Interrupted`.
pub fn read_line<R: BufRead + ?Sized>(
    r: &mut R,
    buf: &mut Vec<u8>,
    max: usize,
) -> io::Result<Line> {
    buf.clear();
    let mut len = 0usize;
    let mut over = false;
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(if over {
                Line::TooLong { len }
            } else if len == 0 {
                Line::Eof
            } else {
                Line::Line
            });
        }
        let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (chunk.len(), false),
        };
        len += take;
        if !over {
            if len > max {
                over = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(if done { take + 1 } else { take });
        if done {
            return Ok(if over {
                Line::TooLong { len }
            } else {
                Line::Line
            });
        }
    }
}
