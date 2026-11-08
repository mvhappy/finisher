//! `fin`'s side of plan D (Task D6) against a fake server that records what it is sent: the
//! actor headers on every request, `cli.command` fire-and-forget (and only when it may be),
//! the exit budget, the map. The journal itself is `cli_diagnostics_online.rs`.

mod support;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use serde_json::{Value, json};
use support::{fin_cmd, finish};

fn endpoint(root: &Path) -> PathBuf {
    support::fake_endpoint(root, "fake.sock")
}

/// What a connection does after it answered its first request.
#[derive(Clone, Copy)]
enum After {
    /// Keep reading (and recording) until the client goes.
    Serve,
    /// Hang up at once: the client's later writes meet a closed socket.
    Hangup,
    /// Stop reading and keep the socket open for a few seconds.
    Stall,
}

struct Fake {
    _dir: tempfile::TempDir,
    root: PathBuf,
    socket: PathBuf,
    requests: Arc<Mutex<Vec<Value>>>,
    /// `exit` values to answer a tag with, in order (else `Success {}`).
    replies: Replies,
}

type Replies = Arc<Mutex<HashMap<String, Vec<Value>>>>;

impl Fake {
    fn start(after: After) -> Fake {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        // Both server data `run` may use hold the key the fake proves itself with.
        support::auth::give_key(&root.join("sd"));
        support::auth::give_key(&root.join("no-server"));
        let socket = endpoint(&root);
        let name = socket.as_path().to_fs_name::<GenericFilePath>().unwrap();
        let listener = ListenerOptions::new().name(name).create_sync().unwrap();
        let requests: Arc<Mutex<Vec<Value>>> = Arc::default();
        let seen = requests.clone();
        let replies: Replies = Arc::default();
        let scripted = replies.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                let (seen, scripted) = (seen.clone(), scripted.clone());
                std::thread::spawn(move || serve(conn, after, &seen, &scripted));
            }
        });
        Fake {
            _dir: dir,
            root,
            socket,
            requests,
            replies,
        }
    }

    /// Makes the fake's address one that nothing listens on (a server that is gone).
    fn vanish(&mut self) {
        if cfg!(windows) {
            // A pipe exists while its listener does; a fresh name was never created.
            self.socket = endpoint(&self.root);
        } else {
            std::fs::remove_file(&self.socket).unwrap();
        }
    }

    fn reply_with(&self, tag: &str, exits: Vec<Value>) {
        self.replies.lock().unwrap().insert(tag.to_string(), exits);
    }

    /// A server-data directory whose `finisherd.json` points at the fake.
    fn discovery(&self, enabled: Option<bool>) -> PathBuf {
        let sd = self.root.join("sd");
        std::fs::create_dir_all(&sd).unwrap();
        let mut info =
            json!({ "pid": 1, "wsPort": 2, "socket": self.socket, "protocolVersion": 2 });
        if let Some(on) = enabled {
            info["diagnostics"] = json!({ "enabled": on });
        }
        std::fs::write(sd.join("finisherd.json"), info.to_string()).unwrap();
        sd
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn records(&self) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|r| r["tag"] == "diagnostics.record")
            .collect()
    }

    /// Waits for `n` records (the write completed before `fin` exited; the server's thread may
    /// lag a moment).
    fn wait_records(&self, n: usize) -> Vec<Value> {
        let end = Instant::now() + Duration::from_secs(5);
        while self.records().len() < n && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.records()
    }
}

fn base64_key() -> Vec<u8> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(support::auth::FAKE_SERVER_KEY)
        .unwrap()
}

fn serve(
    conn: interprocess::local_socket::Stream,
    after: After,
    seen: &Mutex<Vec<Value>>,
    scripted: &Replies,
) {
    let mut reader = BufReader::new(conn);
    let mut first = true;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if frame["_tag"] != "Request" {
            continue;
        }
        // The handshake is answered and not recorded: the tests are about what follows it.
        let key = base64_key();
        if let Some(reply) = support::auth::handshake_answer(&frame, &key) {
            let out = reader.get_mut();
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
            continue;
        }
        seen.lock().unwrap().push(frame.clone());
        if frame["tag"] == "diagnostics.record" {
            continue;
        }
        let next = scripted
            .lock()
            .unwrap()
            .get_mut(frame["tag"].as_str().unwrap_or_default())
            .filter(|v| !v.is_empty())
            .map(|v| v.remove(0));
        let exit = next.unwrap_or_else(|| json!({ "_tag": "Success", "value": {} }));
        let reply = json!({ "_tag": "Exit", "requestId": frame["id"], "exit": exit });
        let out = reader.get_mut();
        let _ = writeln!(out, "{reply}");
        let _ = out.flush();
        if first {
            first = false;
            match after {
                After::Serve => {}
                After::Hangup => return,
                After::Stall => {
                    std::thread::sleep(Duration::from_secs(5));
                    return;
                }
            }
        }
    }
}

/// `fin fonts status`: one server call, no project.
fn run(fake: &Fake, sd: Option<&Path>, env: &[(&str, &str)]) -> (support::Run, Duration) {
    let nowhere = fake.root.join("no-server");
    let mut cmd = fin_cmd(sd.unwrap_or(&nowhere), &fake.root, &["fonts", "status"]);
    if sd.is_none() {
        cmd.env("FIN_SERVER", &fake.socket);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let out = cmd.output().unwrap();
    (finish(&["fonts", "status"], out), started.elapsed())
}

fn none_after_a_moment(fake: &Fake) {
    std::thread::sleep(Duration::from_millis(300));
    assert!(fake.records().is_empty(), "{:?}", fake.records());
}

#[test]
fn cli_command_sent_when_enabled() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    let (r, _) = run(&fake, Some(&sd), &[]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    let records = fake.wait_records(1);
    assert_eq!(records.len(), 1, "{records:?}");
    let tags: Vec<_> = fake.requests().iter().map(|r| r["tag"].clone()).collect();
    assert_eq!(
        tags,
        ["fonts.status", "diagnostics.record"],
        "sent last, on the same connection"
    );
    let rec = &records[0]["payload"];
    assert_eq!(
        (&rec["kind"], &rec["kv"]),
        (&json!("cli.command"), &json!(1))
    );
    let p = &rec["payload"];
    assert_eq!(p["name"], "fonts.status");
    assert_eq!(
        (&p["exit"], &p["online"], &p["dryRun"]),
        (&json!(0), &json!(true), &json!(false))
    );
    assert!(p.get("flags").is_none() && p.get("txIds").is_none(), "{p}");
    assert!(p["micros"].as_u64().unwrap() > 0);
    // Unkeyed on the wire (the server keys it): `blake3:<64 hex>`.
    let hash = p["argsHash"].as_str().unwrap();
    assert!(
        hash.starts_with("blake3:") && hash.len() == 7 + 64,
        "{hash}"
    );
    // An older server wrote no `diagnostics`: send, the server's gate decides.
    let older = Fake::start(After::Serve);
    let sd = older.discovery(None);
    run(&older, Some(&sd), &[]);
    assert_eq!(older.wait_records(1).len(), 1);
}

#[test]
fn nothing_sent_when_disabled() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(false));
    let (r, _) = run(&fake, Some(&sd), &[]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    none_after_a_moment(&fake);
    assert_eq!(fake.requests().len(), 1, "the call itself still went");
}

#[test]
fn nothing_sent_with_do_not_track_or_fin_diagnostics_off() {
    // `FIN_SERVER` skips the discovery file: only fin's own environment can stop the event.
    for (name, value) in [("DO_NOT_TRACK", "1"), ("FIN_DIAGNOSTICS", "off")] {
        let fake = Fake::start(After::Serve);
        let (r, _) = run(&fake, None, &[(name, value)]);
        assert_eq!(r.code, 0, "{name}: {}", r.stdout);
        none_after_a_moment(&fake);
    }
    // Controls: `DO_NOT_TRACK=0` and `FIN_DIAGNOSTICS=on` do not stop it.
    for (name, value) in [("DO_NOT_TRACK", "0"), ("FIN_DIAGNOSTICS", "on")] {
        let fake = Fake::start(After::Serve);
        run(&fake, None, &[(name, value)]);
        assert_eq!(fake.wait_records(1).len(), 1, "{name}={value}");
    }
}

#[test]
fn actor_headers_on_every_request() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    run(&fake, Some(&sd), &[]);
    fake.wait_records(1);
    let requests = fake.requests();
    assert_eq!(requests.len(), 2);
    for r in &requests {
        assert_eq!(r["headers"], json!([["fin-actor-kind", "user"]]), "{r}");
    }
}

#[test]
fn agent_actor_from_fin_session_and_fin_call() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    run(
        &fake,
        Some(&sd),
        &[("FIN_SESSION", "sess-1"), ("FIN_CALL", "call-7")],
    );
    fake.wait_records(1);
    let want = json!([
        ["fin-actor-kind", "agent"],
        ["fin-actor-session", "sess-1"],
        ["fin-actor-call", "call-7"]
    ]);
    for r in fake.requests() {
        assert_eq!(r["headers"], want, "{r}");
    }
    // A session without a call; an over-long id is dropped, not sent.
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    let long = "x".repeat(300);
    run(
        &fake,
        Some(&sd),
        &[("FIN_SESSION", "sess-1"), ("FIN_CALL", &long)],
    );
    fake.wait_records(1);
    for r in fake.requests() {
        assert_eq!(
            r["headers"],
            json!([["fin-actor-kind", "agent"], ["fin-actor-session", "sess-1"]])
        );
    }
}

#[test]
fn slow_server_never_delays_exit_beyond_budget() {
    // After its first answer the server neither reads nor closes. `fin` writes the event
    // (a few hundred bytes the socket buffer takes) and leaves.
    let fake = Fake::start(After::Stall);
    let sd = fake.discovery(Some(true));
    let (r, took) = run(&fake, Some(&sd), &[]);
    assert_eq!(r.code, 0, "{}", r.stdout);
    assert!(took < Duration::from_secs(2), "took {took:?}");
}

#[test]
fn record_failure_never_changes_exit_or_stdout() {
    let quiet = {
        let fake = Fake::start(After::Serve);
        run(&fake, None, &[("FIN_DIAGNOSTICS", "off")]).0
    };
    for after in [After::Hangup, After::Serve, After::Stall] {
        let fake = Fake::start(after);
        let (r, _) = run(&fake, None, &[]);
        assert_eq!(
            (r.code, &r.stdout, &r.stderr),
            (quiet.code, &quiet.stdout, &quiet.stderr)
        );
    }
    // A server that is gone altogether: the call fails, and nothing else is added to it.
    let fake = Fake::start(After::Serve);
    let dead = fake.root.join("nobody.sock");
    let mut cmd = fin_cmd(&fake.root.join("sd"), &fake.root, &["fonts", "status"]);
    cmd.env("FIN_SERVER", &dead);
    let r = finish(&["fonts", "status"], cmd.output().unwrap());
    assert_eq!(
        (r.code, &r.json["error"]["code"]),
        (5, &json!("SERVER_UNAVAILABLE"))
    );
    assert!(fake.requests().is_empty());
}

#[test]
fn map_still_under_budget() {
    let dir = tempfile::tempdir().unwrap();
    let out = fin_cmd(dir.path(), dir.path(), &[]).output().unwrap();
    let r = finish(&[], out);
    assert_eq!(r.code, 0);
    assert!(
        r.stdout.len().div_ceil(4) < 1500,
        "{} tokens",
        r.stdout.len().div_ceil(4)
    );
    assert!(
        r.json["groups"]
            .as_array()
            .unwrap()
            .contains(&json!("diagnostics"))
    );
    let commands = r.json["commands"].as_array().unwrap();
    let line = commands
        .iter()
        .find(|c| c.as_str().unwrap().starts_with("diagnostics:"))
        .unwrap();
    assert_eq!(line, "diagnostics: show export purge project consent");
}
/// `fin <args>` against the fake through discovery; `online`-less runs have no connection of
/// their own.
fn run_args(fake: &Fake, sd: &Path, args: &[&str]) -> support::Run {
    let out = fin_cmd(sd, &fake.root, args).output().unwrap();
    finish(args, out)
}

#[test]
fn runs_that_never_called_the_server_are_recorded_too() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    assert_eq!(run_args(&fake, &sd, &["help", "clip"]).code, 0);
    assert_eq!(run_args(&fake, &sd, &["schema", "param"]).code, 0);
    assert_eq!(
        run_args(&fake, &sd, &["clip", "insert"]).code,
        2,
        "a usage error"
    );
    let records = fake.wait_records(3);
    assert_eq!(records.len(), 3, "{records:?}");
    let seen: Vec<(&str, u64, bool)> = records
        .iter()
        .map(|r| {
            let p = &r["payload"]["payload"];
            (
                p["name"].as_str().unwrap(),
                p["exit"].as_u64().unwrap(),
                p["online"].as_bool().unwrap(),
            )
        })
        .collect();
    // `online`: this command itself talked to the server. None of these did.
    assert_eq!(
        seen,
        [
            ("help", 0, false),
            ("schema.param", 0, false),
            ("other", 2, false)
        ]
    );
    assert_eq!(records[2]["payload"]["payload"]["code"], "USAGE");
    assert!(
        records
            .iter()
            .all(|r| r["headers"] == json!([["fin-actor-kind", "user"]]))
    );
    // Off means off here too.
    let off = Fake::start(After::Serve);
    let sd = off.discovery(Some(false));
    run_args(&off, &sd, &["help"]);
    none_after_a_moment(&off);
    // No server to be found, or one that is gone: nothing is recorded, and nothing is slower.
    let mut gone = Fake::start(After::Serve);
    gone.vanish();
    let sd = gone.discovery(Some(true));
    let started = Instant::now();
    assert_eq!(run_args(&gone, &sd, &["help"]).code, 0);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(
        run_args(&gone, &gone.root.join("nowhere"), &["help"]).code,
        0
    );
    none_after_a_moment(&gone);
}

#[test]
fn an_agent_without_a_usable_session_is_still_an_agent() {
    let long = "x".repeat(300);
    for (session, expect) in [
        ("", json!([["fin-actor-kind", "agent"]])),
        (long.as_str(), json!([["fin-actor-kind", "agent"]])),
    ] {
        let fake = Fake::start(After::Serve);
        let sd = fake.discovery(Some(true));
        run(&fake, Some(&sd), &[("FIN_SESSION", session)]);
        fake.wait_records(1);
        for r in fake.requests() {
            assert_eq!(r["headers"], expect, "{r}");
        }
    }
}

#[test]
fn an_export_that_fails_midway_ends_with_its_error_document() {
    let fake = Fake::start(After::Serve);
    let sd = fake.discovery(Some(true));
    let entry = |n: u32| json!({ "v": 1, "id": format!("E{n}"), "ts": n, "kind": "cli.command" });
    let failure = failure_of("JOURNAL_UNAVAILABLE");
    fake.reply_with(
        "diagnostics.export",
        vec![
            json!({ "_tag": "Success", "value": { "lines": [entry(1), entry(2)], "next": "E2" } }),
            failure,
        ],
    );
    let out = fin_cmd(&sd, &fake.root, &["diagnostics", "export"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    let text = String::from_utf8(out.stdout).unwrap();
    let docs: Vec<Value> = text
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(docs.len(), 3, "{text}");
    assert_eq!(
        (&docs[0]["id"], &docs[1]["id"]),
        (&json!("E1"), &json!("E2"))
    );
    assert_eq!(docs[2]["error"]["code"], "JOURNAL_UNAVAILABLE");
    // The second request paged on from the first answer's `next`.
    let exports: Vec<_> = fake
        .requests()
        .into_iter()
        .filter(|r| r["tag"] == "diagnostics.export")
        .collect();
    assert_eq!(exports[1]["payload"]["cursor"], "E2");
    // An export that fails at once prints the error alone.
    fake.reply_with(
        "diagnostics.export",
        vec![failure_of("JOURNAL_UNAVAILABLE")],
    );
    let out = fin_cmd(&sd, &fake.root, &["diagnostics", "export"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(5));
    assert_eq!(String::from_utf8(out.stdout).unwrap().lines().count(), 1);
}

fn failure_of(code: &str) -> Value {
    json!({ "_tag": "Failure", "cause": [{ "_tag": "Fail", "error": { "code": code, "message": "m", "details": {} } }] })
}
