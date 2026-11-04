//! Contract test: replays every captured request fixture (`tests/fixtures/rpc/*.request.ndjson`,
//! from `pnpm --filter finisherd capture-rpc-fixtures`) against a live `finisherd` and checks
//! that its answers have the captured `_tag` structure and `exit` shape. Values that differ by
//! run (project id, temp dir, fiber ids) are compared by type only.

mod support;

use std::io::{BufRead, BufReader, Write};
use std::time::Duration;

use interprocess::local_socket::prelude::*;
use interprocess::local_socket::{GenericFilePath, Stream};
use serde_json::{Map, Value, json};
use support::Server;

const FIXTURE_PROJECT: &str = "01fixtureproject0000000000";

fn fixture(name: &str) -> Vec<String> {
    let path = format!("{}/tests/fixtures/rpc/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap().lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
}

/// `_tag` and `code` values are kept; every other leaf becomes its JSON type.
fn shape(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let keep = (k == "_tag" || k == "code") && v.is_string();
                    (k.clone(), if keep { v.clone() } else { shape(v) })
                })
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => {
            let mut shapes: Vec<Value> = items.iter().map(shape).collect();
            shapes.dedup();
            Value::Array(shapes)
        }
        Value::String(_) => json!("string"),
        Value::Number(_) => json!("number"),
        Value::Bool(_) => json!("bool"),
        Value::Null => Value::Null,
    }
}

struct Conn {
    io: BufReader<Stream>,
    /// Replaces fixture placeholders with this run's temp dir and project id.
    subst: Vec<(String, String)>,
}

impl Conn {
    fn open(server: &Server, subst: &[(String, String)]) -> Conn {
        let name = server.socket.as_str().to_fs_name::<GenericFilePath>().unwrap();
        let stream = Stream::connect(name).unwrap();
        // Synchronous Windows named pipes have no I/O timeouts (fin's client uses a worker).
        #[cfg(unix)]
        stream.set_recv_timeout(Some(Duration::from_secs(30))).unwrap();
        Conn { io: BufReader::new(stream), subst: subst.to_vec() }
    }

    fn send_line(&mut self, line: &str) {
        let mut line = line.to_string();
        for (from, to) in &self.subst {
            line = line.replace(from, to);
        }
        let out = self.io.get_mut();
        out.write_all(format!("{line}\n").as_bytes()).unwrap();
        out.flush().unwrap();
    }

    fn send(&mut self, frame: &Value) {
        self.send_line(&frame.to_string());
    }

    fn frame(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.io.read_line(&mut line).unwrap() > 0, "server closed the connection");
        serde_json::from_str(&line).unwrap()
    }

    /// Replays a unary scenario: every request line, then one answer per line.
    fn replay(&mut self, name: &str) -> Value {
        let requests = fixture(&format!("{name}.request.ndjson"));
        let expected = fixture(&format!("{name}.response.ndjson"));
        for line in &requests {
            self.send_line(line);
        }
        let mut last = Value::Null;
        for want in &expected {
            let got = self.frame();
            let want: Value = serde_json::from_str(want).unwrap();
            assert_eq!(shape(&got), shape(&want), "{name}: got {got}");
            last = got;
        }
        last
    }
}

fn commit(c: &mut Conn, id: &str, project_id: &str, tx: Value) {
    c.send(&json!({ "_tag": "Request", "id": id, "tag": "tx.commit", "payload": { "projectId": project_id, "tx": tx }, "headers": [] }));
    let got = c.frame();
    assert_eq!(got["exit"]["_tag"], "Success", "{got}");
}

fn tx(id: &str, base: u64, ops: Value) -> Value {
    json!({ "txId": id, "author": "test", "branch": "main", "baseRev": base, "schemaVersion": 1, "ops": ops })
}

#[test]
fn captured_frames_replay_against_a_live_server() {
    let Some(server) = Server::start("captured_frames_replay_against_a_live_server") else { return };
    // Substituted inside JSON string literals: escape it (Windows paths have backslashes).
    let tmp = serde_json::to_string(&server.root.display().to_string()).unwrap();
    let mut subst = vec![("/fixture".to_string(), tmp[1..tmp.len() - 1].to_string())];

    let hello = Conn::open(&server, &subst).replay("hello");
    assert_eq!(hello["exit"]["value"]["protocolVersion"], 1, "{hello}");
    Conn::open(&server, &subst).replay("ping");
    let created = Conn::open(&server, &subst).replay("project-create");
    let project_id = created["exit"]["value"]["projectId"].as_str().unwrap().to_string();
    subst.push((FIXTURE_PROJECT.to_string(), project_id.clone()));
    let committed = Conn::open(&server, &subst).replay("tx-commit");
    assert_eq!(committed["exit"]["value"]["bindings"], json!({ "$a": "a1" }));

    // The capture script's setup between scenarios: insert c1, then delete it (revs 2 and 3).
    let mut setup = Conn::open(&server, &subst);
    let insert = json!({ "op": "clip.insert", "id": "$c", "track": "t1", "at": "0", "asset": "a1", "sourceIn": "0", "duration": "5" });
    commit(&mut setup, "1", &project_id, tx("tx-2", 1, json!([insert])));
    commit(&mut setup, "2", &project_id, tx("tx-3", 2, json!([{ "op": "clip.delete", "clip": "c1" }])));

    let conflict = Conn::open(&server, &subst).replay("conflict");
    assert_eq!(conflict["exit"]["cause"][0]["error"]["details"]["againstTx"], "tx-3");

    replay_subscribe(&server, &subst, &project_id);
    Conn::open(&server, &subst).replay("subscribe-error");
}

/// `ops-subscribe` is a stream: the backlog may arrive in fewer chunks than captured, so the
/// replay follows the protocol (ack every chunk, commit rev 4 live, then interrupt) and compares
/// each frame kind's shape with the captured ones.
fn replay_subscribe(server: &Server, subst: &[(String, String)], project_id: &str) {
    let requests: Vec<Value> = fixture("ops-subscribe.request.ndjson").iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let expected: Vec<Value> = fixture("ops-subscribe.response.ndjson").iter().map(|l| serde_json::from_str(l).unwrap()).collect();
    let tags: Vec<&str> = requests.iter().map(|r| r["_tag"].as_str().unwrap()).collect();
    assert_eq!(tags, ["Request", "Ack", "Ack", "Ack", "Interrupt"]);
    let envelope = |chunk: &Value| shape(&json!({ "_tag": chunk["_tag"], "requestId": chunk["requestId"], "values": [] }));
    let by_rev: Vec<(u64, Value)> = expected
        .iter()
        .filter(|f| f["_tag"] == "Chunk")
        .flat_map(|f| f["values"].as_array().unwrap().iter().map(|v| (v["rev"].as_u64().unwrap(), shape(v))))
        .collect();
    let exit_shape = shape(expected.last().unwrap());

    let mut c = Conn::open(server, subst);
    c.send_line(&requests[0].to_string());
    let mut revs = Vec::new();
    let mut committed_live = false;
    let exit = loop {
        let frame = c.frame();
        if frame["_tag"] != "Chunk" {
            break frame;
        }
        assert_eq!(envelope(&frame), envelope(&expected[0]), "{frame}");
        for v in frame["values"].as_array().unwrap() {
            let rev = v["rev"].as_u64().unwrap();
            let want = &by_rev.iter().find(|(r, _)| *r == rev).unwrap_or_else(|| panic!("unexpected rev {rev}")).1;
            assert_eq!(&shape(v), want, "rev {rev}: {v}");
            revs.push(rev);
        }
        c.send(&requests[1]);
        if revs.contains(&3) && !committed_live {
            let mut writer = Conn::open(server, subst);
            let asset = json!({ "op": "asset.add", "id": "$b", "path": "/media/a.mov", "duration": "600",
                "video": { "width": 1920, "height": 1080, "rate": "30" }, "audio": null, "fingerprint": "fp" });
            commit(&mut writer, "1", project_id, tx("tx-4", 3, json!([asset])));
            committed_live = true;
        }
        if revs.contains(&4) {
            std::thread::sleep(Duration::from_millis(50));
            c.send(&requests[4]);
        }
    };
    assert_eq!(revs, [2, 3, 4]);
    assert_eq!(shape(&exit), exit_shape, "{exit}");
}

#[test]
fn server_tests_ran_or_explicitly_skipped() {
    support::server_tests_ran_or_explicitly_skipped();
}
