//! The example plugin as a real process (the binary cargo built for this package) against the
//! SDK's fake host, and its manifest against 8A's `validate`.

#[path = "../../../../crates/fin-plugin-sdk/tests/support/mod.rs"]
mod support;

use std::path::Path;
use std::process::{Child, Command, Stdio};

use fin_core::model::{Marker, new_project};
use fin_core::{Item, ItemBody, MemIds, Rational, Tx, apply_tx};
use fin_jobs::{HostMsg, KindVersion, WorkerMsg};
use fin_plugin_api::validate::{
    PackageFiles, SourceClass, StructuralChecks, ValidateCtx, validate,
};
use serde_json::{Value, json};
use support::*;

const DIR: &str = env!("CARGO_MANIFEST_DIR");

fn spawn(warm: bool) -> (FakeHost, Child) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rust-tools"));
    if warm {
        cmd.arg("--warm");
    }
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let host = FakeHost::new(
        Box::new(child.stdin.take().unwrap()),
        Box::new(child.stdout.take().unwrap()),
    );
    (host, child)
}

fn t(n: i64) -> Rational {
    Rational::from_int(n)
}

/// Five one-second items on `V1` (`adj1`…`adj5` at 0…4 s) and one on `A1` (`adj6` at 2 s);
/// silence spans over 1–3 s and 3.5–9.5 s, and a point marker named `silence` at 0.
fn project() -> fin_core::Project {
    let mut p = new_project("p1".into(), "Talk".into(), t(25)).unwrap();
    let tl = &mut p.timelines[0];
    for i in 0..5 {
        let item = Item::new(format!("adj{}", i + 1), t(i), t(1), ItemBody::Adjustment {});
        tl.tracks[0].items.push(item);
    }
    let a = Item::new("adj6", t(2), t(1), ItemBody::Adjustment {});
    tl.tracks[1].items.push(a);
    let marker = |id: &str, at: Rational, duration: Option<Rational>, label: &str| Marker {
        id: id.into(),
        at,
        duration,
        label: label.into(),
        color: None,
    };
    tl.markers = vec![
        marker("m1", t(0), None, "silence"),
        marker("m2", t(1), Some(t(2)), "silence"),
        marker("m3", Rational::new(7, 2).unwrap(), Some(t(6)), "silence"),
        marker("m4", t(0), Some(t(10)), "chapter"),
    ];
    p
}

fn serve_snapshot(bytes: &[u8]) -> impl FnMut(&Call) -> Result<Value, fin_jobs::JobError> + '_ {
    move |c| match c.method.as_str() {
        "project.snapshot" => Ok(snapshot_page(bytes, 4096, asked_page(c), 3)),
        "log" => Ok(json!({})),
        m => panic!("unexpected call {m}"),
    }
}

#[test]
fn trim_silence_returns_ops_from_the_snapshot() {
    let p = project();
    let bytes = serde_json::to_vec(&p).unwrap();
    let (mut host, mut child) = spawn(false);
    let h = host.hello();
    assert_eq!(
        (h.family.as_str(), h.warm, h.envelope.clone()),
        ("plugin", false, None)
    );
    assert_eq!(h.kinds, [KindVersion::new("plugin.command", 1)]);
    host.send(&command("j1", "trim-silence", json!({}), 3));
    let (m, calls) = host.serve(serve_snapshot(&bytes));
    let WorkerMsg::Result { value, .. } = m else {
        panic!("{m:?}")
    };
    let lift = |id: &str| json!({ "op": "item.delete", "item": id, "mode": "lift" });
    assert_eq!(
        value,
        json!({ "kind": "tx", "ops": [lift("adj2"), lift("adj3"), lift("adj5"), lift("adj6")] })
    );
    assert!(calls.iter().any(|c| c.method == "project.snapshot"));
    assert!(child.wait().unwrap().success(), "a cold plugin exits 0");

    // The proposal applies under fin-core.
    let tx: Tx = serde_json::from_value(json!({
        "txId": "t1", "author": "local", "branch": "main", "baseRev": 3,
        "schemaVersion": p.schema_version, "ops": value["ops"],
    }))
    .unwrap();
    let applied = apply_tx(&p, &tx, &mut MemIds::after(&p)).unwrap();
    let left: Vec<_> = applied.project.timelines[0].tracks[0]
        .items
        .iter()
        .map(|i| i.id.as_str())
        .collect();
    // `adj4` (3–4 s) straddles the second span's start (3.5 s) and stays.
    assert_eq!(left, ["adj1", "adj4"]);
}

#[test]
fn trim_silence_takes_its_arguments_warm() {
    let bytes = serde_json::to_vec(&project()).unwrap();
    let (mut host, mut child) = spawn(true);
    assert!(host.hello().warm);
    let args = json!({ "label": "chapter", "mode": "ripple" });
    host.send(&command("j1", "trim-silence", args, 3));
    let (m, _) = host.serve(serve_snapshot(&bytes));
    let WorkerMsg::Result { value, .. } = m else {
        panic!("{m:?}")
    };
    assert_eq!(value["ops"].as_array().unwrap().len(), 6);
    assert_eq!(value["ops"][0]["mode"], "ripple");
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    // The same process serves the next job, here a name it does not implement.
    host.send(&command("j2", "nope", json!({}), 3));
    let (m, _) = host.serve(unanswered);
    let WorkerMsg::Error { error, .. } = m else {
        panic!("{m:?}")
    };
    assert_eq!(error.code, "PLUGIN_COMMAND_ARGS");
    assert_eq!(host.recv(), WorkerMsg::Ready {});
    host.send(&HostMsg::Shutdown {});
    assert!(child.wait().unwrap().success());
}

#[test]
fn fetch_title_uses_net_fetch() {
    let body = b"<!doctype html><HTML><head><Title>\n Example Domain </title></head></html>";
    let (mut host, mut child) = spawn(false);
    host.hello();
    host.send(&command(
        "j1",
        "fetch-title",
        json!({ "path": "/about" }),
        3,
    ));
    let (m, calls) = host.serve(|c| match c.method.as_str() {
        "net.fetch" => Ok(fetch_head(200, body.len())),
        "net.read" => Ok(read_page(body, &c.params)),
        m => panic!("unexpected call {m}"),
    });
    let WorkerMsg::Result { value, .. } = m else {
        panic!("{m:?}")
    };
    assert_eq!(
        value,
        json!({ "kind": "data", "value": { "status": 200, "title": "Example Domain" } })
    );
    assert_eq!(
        calls[0].params["request"],
        json!({ "method": "GET", "url": "https://api.example.org/about" })
    );
    assert!(child.wait().unwrap().success());

    // A broker refusal is the command's error, as the host sent it.
    let (mut host, mut child) = spawn(false);
    host.hello();
    host.send(&command("j1", "fetch-title", json!({}), 3));
    let (m, _) = host.serve(|_| Err(fin_jobs::JobError::new("EGRESS_PRIVATE_ADDRESS", "refused")));
    let WorkerMsg::Error { error, .. } = m else {
        panic!("{m:?}")
    };
    assert_eq!(error.code, "EGRESS_PRIVATE_ADDRESS");
    assert!(child.wait().unwrap().success());
}

#[test]
fn host_gone_ends_the_process() {
    let (mut host, mut child) = spawn(true);
    host.hello();
    host.close();
    assert_eq!(child.wait().unwrap().code(), Some(fin_jobs::EXIT_HOST_GONE));
}

/// The package as the source tree has it; the per-platform executables are build outputs.
struct SourceTree<'a>(&'a Path);

impl PackageFiles for SourceTree<'_> {
    fn exists(&self, path: &str) -> bool {
        path.starts_with("bin/") || self.0.join(path).is_file()
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

#[test]
fn manifest_passes_validate() {
    let dir = Path::new(DIR);
    let bytes = std::fs::read(dir.join("finisher-plugin.json")).unwrap();
    let ctx = ValidateCtx {
        host_api: fin_plugin_api::host_api(),
        platform: None,
        source: SourceClass::Registry,
        node: None,
        content_checks: &StructuralChecks,
    };
    let m = validate(&bytes, &SourceTree(dir), &ctx).unwrap_or_else(|e| panic!("{e:#?}"));
    let names: Vec<_> = m
        .contributes
        .commands
        .iter()
        .map(|c| (c.name.as_str(), c.writes))
        .collect();
    assert_eq!(names, [("trim-silence", true), ("fetch-title", false)]);
}
