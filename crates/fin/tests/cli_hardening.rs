//! fin's client hardening (sub-project 5A Task 10, Decisions 36–37) against fake servers that
//! speak just enough NDJSON: a frame cap (a server line over 16 MiB is `PROTOCOL_ERROR {max}`)
//! and an overall deadline (`FIN_DEADLINE_S`: `SERVER_TIMEOUT {deadline}` once the run's time
//! is up, however fast each single answer is). The deadline test is scaled down (decimal
//! seconds) so it runs in under two seconds.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, ListenerOptions};
use serde_json::{Value, json};
use support::{assert_error, fin_env, real_tempdir};

/// A fake finisherd: writes `finisherd.json` into `sd` and answers each request line on the
/// first connection with `answer(n, request)` (`None` closes the connection).
fn fake_server(
    sd: &Path,
    answer: impl Fn(usize, &Value) -> Option<Vec<u8>> + Send + 'static,
) -> JoinHandle<usize> {
    std::fs::create_dir_all(sd.join("run")).unwrap();
    let key = support::auth::give_key(sd);
    let socket: PathBuf = support::fake_endpoint(&sd.join("run"), "fin.sock");
    let name = socket.as_path().to_fs_name::<GenericFilePath>().unwrap();
    let listener = ListenerOptions::new().name(name).create_sync().unwrap();
    let info = json!({ "pid": 1, "wsPort": 1, "socket": socket.display().to_string(), "protocolVersion": 2, "envId": "01test0000000000000000000a" });
    std::fs::write(sd.join("finisherd.json"), info.to_string()).unwrap();
    std::thread::spawn(move || {
        let stream = listener.accept().unwrap();
        let mut reader = BufReader::new(stream);
        let mut n = 0;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                return n;
            }
            let request: Value = serde_json::from_str(&line).unwrap();
            if request["_tag"] != "Request" {
                continue;
            }
            // The handshake is answered at once and not counted.
            if let Some(reply) = support::auth::handshake_answer(&request, &key) {
                if reader
                    .get_mut()
                    .write_all(format!("{reply}\n").as_bytes())
                    .is_err()
                {
                    return n;
                }
                continue;
            }
            n += 1;
            let Some(bytes) = answer(n, &request) else {
                return n;
            };
            if reader.get_mut().write_all(&bytes).is_err() {
                return n;
            }
            let _ = reader.get_mut().flush();
        }
    })
}

fn exit_line(request: &Value, value: Value) -> Vec<u8> {
    let frame = json!({ "_tag": "Exit", "requestId": request["id"], "exit": { "_tag": "Success", "value": value } });
    format!("{frame}\n").into_bytes()
}

#[test]
fn fin_frame_cap() {
    let (_tmp, root) = real_tempdir();
    let sd = root.join("sd");
    let server = fake_server(&sd, |_, request| {
        // A 17 MiB Exit line.
        Some(exit_line(
            request,
            json!({ "pad": "x".repeat(17 * 1024 * 1024) }),
        ))
    });
    let run = fin_env(
        &sd,
        &root,
        &["view", "--project", "film"],
        &[("FIN_DEADLINE_S", "30".as_ref())],
    );
    assert_error(&run, 6, "PROTOCOL_ERROR");
    assert_eq!(run.json["error"]["details"]["max"], 16 * 1024 * 1024);
    drop(server);
}

#[test]
fn fin_overall_deadline() {
    let (_tmp, root) = real_tempdir();
    let sd = root.join("sd");
    // Every answer takes 0.25 s and the run may take 0.4 s: each call alone is far within its
    // 30 s timeout, but the second cannot finish in time.
    let server = fake_server(&sd, |n, request| {
        std::thread::sleep(Duration::from_millis(250));
        Some(match n {
            1 => exit_line(
                request,
                json!({ "projectId": "01fixtureproject0000000000", "rev": 0 }),
            ),
            _ => exit_line(request, json!({ "slow": true })),
        })
    });
    let started = Instant::now();
    // `fin undo` sends `project.open`, then `history.undo`.
    let run = fin_env(
        &sd,
        &root,
        &["undo", "--project", "film"],
        &[("FIN_DEADLINE_S", "0.4".as_ref())],
    );
    let took = started.elapsed();
    assert_error(&run, 5, "SERVER_TIMEOUT");
    assert_eq!(run.json["error"]["details"]["deadline"], 0.4);
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains("FIN_DEADLINE_S")
    );
    // The undo may have landed: the key to retry with is named too (review #8).
    let key = run.json["error"]["details"]["txId"].as_str().unwrap();
    assert!(
        run.json["error"]["fix"]
            .as_str()
            .unwrap()
            .contains(&format!("--idem-key {key}"))
    );
    assert!(took < Duration::from_secs(5), "{took:?}");
    assert_eq!(server.join().unwrap(), 2);
    // A bad value is a usage error before anything connects.
    let run = fin_env(
        &sd,
        &root,
        &["view", "--project", "film"],
        &[("FIN_DEADLINE_S", "soon".as_ref())],
    );
    assert_error(&run, 2, "USAGE");
}
