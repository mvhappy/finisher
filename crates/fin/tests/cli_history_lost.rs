//! A history or branch write whose answer is lost (review of 2026-11-09, #8): the connection
//! drops after the request was sent. fin retries **once with the same key** (the server answers
//! a key it applied from its log); when that answer is lost too, the error names the key
//! (`details.txId`) and the exact `--idem-key` retry, and never says to start a server that is
//! running. A fake finisherd does the real handshake and drops the connection on the write.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use serde_json::{Value, json};
use support::{assert_error, fin_env, real_tempdir};

/// The writes keyed by an idempotency key that `fin` sends through `history::send_flagged`.
const WRITES: &[&str] = &[
    "history.undo",
    "history.redo",
    "history.checkpoint",
    "history.merge",
    "history.cherryPick",
    "history.revert",
    "branch.create",
    "branch.set",
];

struct Fake {
    _dir: tempfile::TempDir,
    sd: PathBuf,
    project: PathBuf,
    /// Every write request's payload, in order.
    writes: Arc<Mutex<Vec<Value>>>,
}

fn ok(frame: &Value, value: Value) -> Value {
    json!({ "_tag": "Exit", "requestId": frame["id"], "exit": { "_tag": "Success", "value": value } })
}

/// What the fake answers a request other than a write.
fn answer(frame: &Value) -> Value {
    match frame["tag"].as_str().unwrap_or("") {
        "project.open" => ok(frame, json!({ "projectId": "01lostproject0000000000000" })),
        "branch.list" => ok(
            frame,
            json!({ "branches": [{ "name": "main", "branchId": "01main", "archived": false }] }),
        ),
        _ => ok(frame, Value::Null),
    }
}

/// A fake finisherd: drops the connection on the first `drops` writes, then answers writes
/// with `{committed: []}`.
fn start(drops: usize) -> Fake {
    let (dir, root) = real_tempdir();
    let sd = root.join("sd");
    std::fs::create_dir_all(sd.join("run")).unwrap();
    let key = support::auth::give_key(&sd);
    let socket = support::fake_endpoint(&sd.join("run"), "fin.sock");
    let info = json!({ "pid": 1, "wsPort": 1, "socket": socket.display().to_string(), "protocolVersion": 2, "envId": "01test0000000000000000000a" });
    std::fs::write(sd.join("finisherd.json"), info.to_string()).unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("finisher.project.json"), "{}").unwrap();
    let name = socket.as_path().to_fs_name::<GenericFilePath>().unwrap();
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    let writes: Arc<Mutex<Vec<Value>>> = Arc::default();
    let dropped = Arc::new(AtomicUsize::new(0));
    let record = writes.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let (key, record, dropped) = (key.clone(), record.clone(), dropped.clone());
            std::thread::spawn(move || serve(conn, &key, &record, &dropped, drops));
        }
    });
    Fake {
        _dir: dir,
        sd,
        project,
        writes,
    }
}

fn serve(
    conn: interprocess::local_socket::Stream,
    key: &[u8],
    writes: &Mutex<Vec<Value>>,
    dropped: &AtomicUsize,
    drops: usize,
) {
    let mut io = BufReader::new(conn);
    let mut line = String::new();
    while io.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
        let frame: Value = serde_json::from_str(line.trim_end()).unwrap_or_default();
        line.clear();
        if frame["_tag"] != "Request" {
            continue;
        }
        let reply = if let Some(reply) = support::auth::handshake_answer(&frame, key) {
            reply
        } else if WRITES.contains(&frame["tag"].as_str().unwrap_or("")) {
            writes.lock().unwrap().push(frame["payload"].clone());
            if dropped.fetch_add(1, Ordering::SeqCst) < drops {
                // The request arrived; its answer never leaves.
                return;
            }
            ok(&frame, json!({ "committed": [] }))
        } else {
            answer(&frame)
        };
        if io
            .get_mut()
            .write_all(format!("{reply}\n").as_bytes())
            .is_err()
        {
            return;
        }
    }
}

fn run(fake: &Fake, args: &[&str]) -> support::Run {
    fin_env(
        &fake.sd,
        &fake.project,
        args,
        &[("FIN_DIAGNOSTICS", "off".as_ref())],
    )
}

/// Each keyed write, as a `fin` command line.
const COMMANDS: &[&[&str]] = &[
    &["undo"],
    &["redo"],
    &["checkpoint", "cp1"],
    &["merge", "feature", "--into", "main"],
    &["cherry-pick", "01origin", "--into", "main"],
    &["revert", "01merge"],
    &["branch", "new", "feature"],
    &["branch", "rename", "main", "trunk"],
];

fn keys(fake: &Fake) -> Vec<Value> {
    let writes = fake.writes.lock().unwrap();
    writes.iter().map(|w| w["txId"].clone()).collect()
}

#[test]
fn a_lost_answer_is_retried_once_with_the_same_key() {
    for args in COMMANDS {
        let fake = start(1);
        let run = run(&fake, args);
        assert_eq!(run.code, 0, "{args:?}: {}", run.stdout);
        let keys = keys(&fake);
        assert_eq!(keys.len(), 2, "{args:?}: {keys:?}");
        assert!(keys[0].is_string(), "{args:?}: {keys:?}");
        assert_eq!(keys[0], keys[1], "{args:?}: the retry keeps the key");
        let writes = fake.writes.lock().unwrap();
        assert_eq!(
            writes[0], writes[1],
            "{args:?}: the retry is the same request"
        );
        // Hashed with or without `--idem-key`: a manual retry with other args is refused.
        assert!(
            writes[0]["argsHash"]
                .as_str()
                .is_some_and(|h| h.starts_with("blake3:")),
            "{args:?}: {}",
            writes[0]
        );
    }
}

#[test]
fn a_second_lost_answer_names_the_key_to_retry_with() {
    for args in COMMANDS {
        let fake = start(2);
        let run = run(&fake, args);
        assert_error(&run, 5, "SERVER_UNAVAILABLE");
        let keys = keys(&fake);
        assert_eq!(keys.len(), 2, "{args:?}: one retry only: {keys:?}");
        assert_eq!(keys[0], keys[1], "{args:?}");
        let error = &run.json["error"];
        // A merge group's records are `{txId}-1`…: the key named is the base key it sent.
        assert_eq!(error["details"]["txId"], keys[0], "{args:?}: {error}");
        let key = keys[0].as_str().unwrap();
        let fix = error["fix"].as_str().unwrap();
        assert!(
            fix.contains(&format!("`--idem-key {key}`")),
            "{args:?}: {fix}"
        );
        assert!(!fix.contains("start finisherd"), "{args:?}: {fix}");
        let message = error["message"].as_str().unwrap();
        assert!(!message.contains("not reachable"), "{args:?}: {message}");
    }
}

#[test]
fn a_given_key_is_the_one_retried_and_named() {
    let fake = start(2);
    let run = run(&fake, &["--idem-key", "undo-k1", "undo"]);
    assert_error(&run, 5, "SERVER_UNAVAILABLE");
    assert_eq!(keys(&fake), [json!("undo-k1"), json!("undo-k1")]);
    assert_eq!(run.json["error"]["details"]["txId"], "undo-k1");
}
