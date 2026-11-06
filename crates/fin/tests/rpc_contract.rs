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
    std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// `_tag` and `code` values are kept; every other leaf becomes its JSON type.
fn shape(v: &Value) -> Value {
    shape_to(v, usize::MAX)
}

/// [`shape`] down to `depth` levels; anything deeper is `"any"` (fin-store reports whose
/// fields are `null` or a value depending on timing, such as `lastExportMs`).
fn shape_to(v: &Value, depth: usize) -> Value {
    if depth == 0 {
        return json!("any");
    }
    let shape = |v: &Value| shape_to(v, depth - 1);
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
        let name = server
            .socket
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .unwrap();
        let stream = Stream::connect(name).unwrap();
        // Synchronous Windows named pipes have no I/O timeouts (fin's client uses a worker).
        #[cfg(unix)]
        stream
            .set_recv_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        Conn {
            io: BufReader::new(stream),
            subst: subst.to_vec(),
        }
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
        assert!(
            self.io.read_line(&mut line).unwrap() > 0,
            "server closed the connection"
        );
        serde_json::from_str(&line).unwrap()
    }

    /// Replays a unary scenario: every request line, then one answer per line.
    fn replay(&mut self, name: &str) -> Value {
        self.replay_to(name, usize::MAX)
    }

    /// [`Conn::replay`] comparing frames down to `depth` levels only.
    fn replay_to(&mut self, name: &str, depth: usize) -> Value {
        let requests = fixture(&format!("{name}.request.ndjson"));
        let expected = fixture(&format!("{name}.response.ndjson"));
        for line in &requests {
            self.send_line(line);
        }
        let mut last = Value::Null;
        for want in &expected {
            let got = self.frame();
            let want: Value = serde_json::from_str(want).unwrap();
            assert_eq!(
                shape_to(&got, depth),
                shape_to(&want, depth),
                "{name}: got {got}"
            );
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

/// One request that must succeed (setup between captured scenarios).
fn rpc(c: &mut Conn, project_id: &str, tag: &str, payload: Value) {
    let mut payload = payload;
    payload["projectId"] = json!(project_id);
    c.send(&json!({ "_tag": "Request", "id": "1", "tag": tag, "payload": payload, "headers": [] }));
    let got = c.frame();
    assert_eq!(got["exit"]["_tag"], "Success", "{tag}: {got}");
}

fn tx(id: &str, base: u64, ops: Value) -> Value {
    json!({ "txId": id, "author": "test", "branch": "main", "baseRev": base, "schemaVersion": 1, "ops": ops })
}

#[test]
fn captured_frames_replay_against_a_live_server() {
    let Some(server) = Server::start("captured_frames_replay_against_a_live_server") else {
        return;
    };
    // Substituted inside JSON string literals: escape it (Windows paths have backslashes).
    let tmp = serde_json::to_string(&server.root.display().to_string()).unwrap();
    let mut subst = vec![("/fixture".to_string(), tmp[1..tmp.len() - 1].to_string())];

    let hello = Conn::open(&server, &subst).replay("hello");
    assert_eq!(hello["exit"]["value"]["protocolVersion"], 1, "{hello}");
    Conn::open(&server, &subst).replay("ping");
    let created = Conn::open(&server, &subst).replay("project-create");
    let project_id = created["exit"]["value"]["projectId"]
        .as_str()
        .unwrap()
        .to_string();
    subst.push((FIXTURE_PROJECT.to_string(), project_id.clone()));
    let retried = Conn::open(&server, &subst).replay("project-create-retry");
    assert_eq!(
        retried["exit"]["value"]["projectId"], project_id,
        "{retried}"
    );
    let committed = Conn::open(&server, &subst).replay("tx-commit");
    assert_eq!(
        committed["exit"]["value"]["bindings"],
        json!({ "$a": "a1" })
    );

    // The capture script's setup between scenarios: insert c1, then delete it (revs 2 and 3).
    let mut setup = Conn::open(&server, &subst);
    let insert = json!({ "op": "clip.insert", "id": "$c", "track": "t1", "at": "0", "asset": "a1", "sourceIn": "0", "duration": "5" });
    commit(&mut setup, "1", &project_id, tx("tx-2", 1, json!([insert])));
    commit(
        &mut setup,
        "2",
        &project_id,
        tx("tx-3", 2, json!([{ "op": "clip.delete", "clip": "c1" }])),
    );

    let conflict = Conn::open(&server, &subst).replay("conflict");
    assert_eq!(
        conflict["exit"]["cause"][0]["error"]["details"]["againstTx"],
        "tx-3"
    );

    replay_subscribe(&server, &subst, &project_id);
    Conn::open(&server, &subst).replay("subscribe-error");
    replay_storage(&server, &subst, &project_id);
}

/// Frame → exit → value → the value's keys: what a storage report's shape is checked to.
const REPORT_KEYS: usize = 4;

/// The storage RPCs (plan 4A Task 15), in the capture script's order.
fn replay_storage(server: &Server, subst: &[(String, String)], project_id: &str) {
    let snap = Conn::open(server, subst).replay("snapshot-get");
    assert_eq!(snap["exit"]["value"]["generation"]["n"], 0, "{snap}");
    let status = Conn::open(server, subst).replay_to("project-status", REPORT_KEYS);
    assert_eq!(
        status["exit"]["value"]["lease"]["state"], "held",
        "{status}"
    );
    assert_eq!(status["exit"]["value"]["busy"], Value::Null);
    Conn::open(server, subst).replay_to("project-takeover", REPORT_KEYS);
    let verify = Conn::open(server, subst).replay_to("project-verify", REPORT_KEYS);
    assert_eq!(verify["exit"]["value"]["ok"], true, "{verify}");
    Conn::open(server, subst).replay_to("project-compact", REPORT_KEYS);
    Conn::open(server, subst).replay("project-recover");
    for name in ["project-acknowledge-rewind", "project-acknowledge-salvage"] {
        let ack = Conn::open(server, subst).replay(name);
        assert_eq!(
            ack["exit"]["value"],
            json!({ "acknowledged": false }),
            "{ack}"
        );
    }
    let got = Conn::open(server, subst).replay("tx-get");
    assert_eq!(got["exit"]["value"]["tx"]["txId"], "tx-1", "{got}");
    let unknown = Conn::open(server, subst).replay("tx-get-unknown");
    assert_eq!(unknown["exit"]["value"], Value::Null);
    let stale = Conn::open(server, subst).replay("tx-commit-stale");
    assert_eq!(
        stale["exit"]["cause"][0]["error"]["code"],
        "HISTORY_REWOUND"
    );
    let stale = Conn::open(server, subst).replay("ops-subscribe-stale");
    assert_eq!(
        stale["exit"]["cause"][0]["error"]["details"]["reason"],
        "GENERATION"
    );
    let relocated = Conn::open(server, subst).replay("project-open-relocate");
    assert_eq!(relocated["exit"]["value"]["rev"], 4, "{relocated}");
    replay_events(server, subst);
    replay_history(server, subst, project_id);
    let rebuilt = Conn::open(server, subst).replay_to("project-rebuild", REPORT_KEYS);
    assert_eq!(rebuilt["exit"]["value"]["reason"], "manual", "{rebuilt}");
}

/// The branch and history RPCs (plan 4B Task 18), in the capture script's order, with its
/// setup between scenarios (main is at rev 4 here).
fn replay_history(server: &Server, subst: &[(String, String)], project_id: &str) {
    let mut setup = Conn::open(server, subst);
    let on_branch = |id: &str, branch: &str, base: u64, ops: Value| {
        let mut t = tx(id, base, ops);
        t["branch"] = json!(branch);
        t
    };
    let clip = |at: u64| json!({ "op": "clip.insert", "id": "$c", "track": "t1", "at": at.to_string(), "asset": "a1", "sourceIn": "0", "duration": "5" });
    let delete = json!({ "op": "clip.delete", "clip": "c2" });
    let who = "fixture";
    commit(
        &mut setup,
        "1",
        project_id,
        tx("fx-m0", 4, json!([clip(0)])),
    );
    let created = Conn::open(server, subst).replay("branch-create");
    assert_eq!(created["exit"]["value"]["branch"]["id"], "br1", "{created}");
    let listed = Conn::open(server, subst).replay("branch-list");
    assert_eq!(
        listed["exit"]["value"]["branches"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    commit(
        &mut setup,
        "2",
        project_id,
        on_branch("fx-e1", "br1", 5, json!([delete])),
    );
    commit(
        &mut setup,
        "3",
        project_id,
        on_branch("fx-e2", "br1", 6, json!([clip(20)])),
    );
    commit(&mut setup, "4", project_id, tx("fx-m1", 6, json!([delete])));
    let conflict = Conn::open(server, subst).replay("history-merge-conflict");
    assert_eq!(conflict["exit"]["cause"][0]["error"]["code"], "CONFLICT");
    let merged = Conn::open(server, subst).replay("history-merge");
    assert_eq!(
        merged["exit"]["value"]["committed"]
            .as_array()
            .unwrap()
            .len(),
        1,
        "{merged}"
    );
    assert_eq!(
        merged["exit"]["value"]["skipped"][0]["reason"], "REMOVED",
        "{merged}"
    );
    let undo = Conn::open(server, subst).replay("history-undo");
    assert_eq!(undo["exit"]["value"]["undone"], json!(["fx-mg1"]), "{undo}");
    let redo = Conn::open(server, subst).replay("history-redo");
    assert_eq!(
        redo["exit"]["value"]["committed"].as_array().unwrap().len(),
        1,
        "{redo}"
    );
    let cp = Conn::open(server, subst).replay("history-checkpoint");
    assert_eq!(cp["exit"]["value"]["tx"]["txId"], "fx-c1", "{cp}");
    rpc(
        &mut setup,
        project_id,
        "branch.create",
        json!({ "txId": "fx-b2", "author": who, "name": "other", "from": "main" }),
    );
    commit(
        &mut setup,
        "5",
        project_id,
        on_branch("fx-p1", "br2", 11, json!([clip(40)])),
    );
    let pick = Conn::open(server, subst).replay("history-cherry-pick");
    assert_eq!(
        pick["exit"]["value"]["committed"].as_array().unwrap().len(),
        1,
        "{pick}"
    );
    let revert = Conn::open(server, subst).replay("history-revert");
    assert_eq!(
        revert["exit"]["value"]["undone"],
        json!(["fx-mg1"]),
        "{revert}"
    );
    let set = Conn::open(server, subst).replay("branch-set");
    assert_eq!(set["exit"]["value"]["tx"]["txId"], "fx-s1", "{set}");
    let dismissed = Conn::open(server, subst).replay("project-recover-dismiss-unknown");
    assert_eq!(
        dismissed["exit"]["cause"][0]["error"]["code"], "NOT_FOUND",
        "{dismissed}"
    );
    let recover = Conn::open(server, subst).replay("project-recover-groups");
    assert_eq!(recover["exit"]["value"]["dryRun"], true, "{recover}");
}

/// `project-events`: the first chunk (the status now), its ack, then the interrupt. Status
/// changes may add chunks before the interrupt lands; each is acked.
fn replay_events(server: &Server, subst: &[(String, String)]) {
    let requests: Vec<String> = fixture("project-events.request.ndjson");
    let expected: Vec<Value> = fixture("project-events.response.ndjson")
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tags: Vec<Value> = requests
        .iter()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["_tag"].clone())
        .collect();
    assert_eq!(tags, [json!("Request"), json!("Ack"), json!("Interrupt")]);
    let mut c = Conn::open(server, subst);
    c.send_line(&requests[0]);
    let first = c.frame();
    // Chunk → values → item → its keys.
    assert_eq!(shape_to(&first, 4), shape_to(&expected[0], 4), "{first}");
    assert_eq!(first["values"][0]["type"], "status");
    assert_eq!(first["values"][0]["status"]["lease"]["state"], "held");
    c.send_line(&requests[1]);
    c.send_line(&requests[2]);
    let exit = loop {
        let frame = c.frame();
        if frame["_tag"] != "Chunk" {
            break frame;
        }
        c.send_line(&requests[1]);
    };
    assert_eq!(shape(&exit), shape(expected.last().unwrap()), "{exit}");
}

/// `ops-subscribe` is a stream: the backlog may arrive in fewer chunks than captured, so the
/// replay follows the protocol (ack every chunk, commit rev 4 live, then interrupt) and compares
/// each frame kind's shape with the captured ones.
fn replay_subscribe(server: &Server, subst: &[(String, String)], project_id: &str) {
    let requests: Vec<Value> = fixture("ops-subscribe.request.ndjson")
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let expected: Vec<Value> = fixture("ops-subscribe.response.ndjson")
        .iter()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let tags: Vec<&str> = requests
        .iter()
        .map(|r| r["_tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, ["Request", "Ack", "Ack", "Ack", "Interrupt"]);
    let envelope = |chunk: &Value| {
        shape(&json!({ "_tag": chunk["_tag"], "requestId": chunk["requestId"], "values": [] }))
    };
    let by_rev: Vec<(u64, Value)> = expected
        .iter()
        .filter(|f| f["_tag"] == "Chunk")
        .flat_map(|f| {
            f["values"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| (v["rev"].as_u64().unwrap(), shape(v)))
        })
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
            let want = &by_rev
                .iter()
                .find(|(r, _)| *r == rev)
                .unwrap_or_else(|| panic!("unexpected rev {rev}"))
                .1;
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
