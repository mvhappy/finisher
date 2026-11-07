//! Unit tests of font planning in the `Builder` (plan 4C Decision 35): a write with caption
//! text sends its ops to `fonts.plan` and prepends the answer; offline it plans nothing and says
//! `measured: false`; other writes never ask; planned pins never change `argsHash`.
//!
//! `font.pin` ops arrive with Task 18, so the server's answer here is a stand-in op (a
//! `bin.add`): the Builder prepends whatever `fonts.plan` returns, unread.

use std::cell::RefCell;
use std::path::Path;

use fin_core::{
    BinAdd, CommittedTx, InsertMode, ItemBody, ItemInsert, Op, Rational, Track, TrackKind, Tx,
    new_project,
};

use super::*;
use crate::backend::{FontPlan, Probed, Snapshot};

/// A project with a caption track `t3`; commits succeed at rev 2. Records the commits sent and
/// how often `fonts.plan` was asked.
struct Planning {
    offline: bool,
    pins: Vec<Op>,
    sent: RefCell<Vec<Tx>>,
    asked: RefCell<usize>,
}

fn planning(offline: bool, pins: Vec<Op>) -> Planning {
    Planning {
        offline,
        pins,
        sent: RefCell::default(),
        asked: RefCell::default(),
    }
}

fn stand_in_pin() -> Op {
    Op::BinAdd(BinAdd {
        id: "$pin".into(),
        name: "pins".into(),
        parent: None,
    })
}

impl Backend for Planning {
    fn snapshot(&self, _branch: &str) -> Result<Snapshot, CoreError> {
        let mut project = new_project("p".into(), "P".into(), Rational::from_int(30))?;
        project.timelines[0]
            .tracks
            .push(Track::new("t3", TrackKind::Caption, "C1"));
        Ok(Snapshot {
            rev: 1,
            project,
            generation: None,
        })
    }
    fn commit(&self, tx: &Tx, _generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        self.sent.borrow_mut().push(tx.clone());
        Ok(serde_json::from_value(json!({
            "rev": 2, "tx": tx, "bindings": {}, "diff": { "changes": [], "shifts": [] },
        }))
        .unwrap())
    }
    fn tx_get(&self, _tx_id: &str) -> Result<TxLookup, CoreError> {
        Ok(TxLookup::Unknown)
    }
    fn project_call(&self, _tag: &str, _payload: Value) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn rebuild(&self) -> Result<(Value, bool), CoreError> {
        unreachable!()
    }
    fn probe(&self, _path: &Path) -> Result<Probed, CoreError> {
        unreachable!()
    }
    fn create_project(
        &self,
        _dir: &Path,
        _name: &str,
        _rate: Rational,
        _idem: Option<&str>,
    ) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn open_project(&self, _dir: &Path, _relocate: bool) -> Result<Value, CoreError> {
        unreachable!()
    }
    fn dry_run_only(&self) -> bool {
        self.offline
    }
    fn fonts_plan(&self, _branch: &str, _ops: &[Op]) -> Result<FontPlan, CoreError> {
        *self.asked.borrow_mut() += 1;
        Ok(FontPlan {
            ops: self.pins.clone(),
            notes: vec![json!({ "code": "FONT_NOT_IN_CATALOG", "family": "Nope" })],
        })
    }
}

fn caption() -> Op {
    let body: ItemBody = serde_json::from_value(json!({
        "kind": "caption", "style": "preset:bold",
        "words": [{ "text": "Hi", "start": "0", "end": "1" }],
    }))
    .unwrap();
    ItemInsert {
        id: "$cap".into(),
        track: "t3".into(),
        at: Rational::zero(),
        duration: Rational::from_int(1),
        mode: InsertMode::Overwrite,
        body,
    }
    .into()
}

/// Plans one caption insert (pushed directly: `fin caption` commands arrive with Task 24).
fn write_caption(backend: &Planning, globals: &Globals) -> Result<Value, CoreError> {
    let mut b = Builder::new(backend, globals)?;
    b.inputs
        .push(json!({ "cmd": "caption.add", "args": { "text": "Hi" } }));
    let start = b.len();
    b.push(caption())?;
    b.steps.push(start..b.len());
    b.finish(globals, json!({ "item": "$cap" }))
}

#[test]
fn builder_prepends_planned_pins() {
    let backend = planning(false, vec![stand_in_pin()]);
    let doc = write_caption(&backend, &Globals::default()).unwrap();
    assert_eq!(*backend.asked.borrow(), 1);
    let sent = backend.sent.borrow();
    let names: Vec<&str> = sent[0].ops.iter().map(Op::name).collect();
    assert_eq!(
        names,
        ["bin.add", "item.insert"],
        "pins first, then the step"
    );
    let fonts = &doc["resolved"]["fonts"];
    assert_eq!(fonts["planned"].as_array().unwrap().len(), 1);
    assert_eq!(fonts["planned"][0]["op"], "bin.add");
    assert_eq!(fonts["notes"][0]["code"], "FONT_NOT_IN_CATALOG");

    // An online dry run plans too, and shows the pins in its diff.
    let dry = Globals {
        dry_run: true,
        ..Globals::default()
    };
    let doc = write_caption(&planning(false, vec![stand_in_pin()]), &dry).unwrap();
    assert_eq!(doc["dryRun"], true);
    assert_eq!(doc["bindings"]["$pin"], "b1");
}

#[test]
fn planned_pins_do_not_change_args_hash() {
    let globals = Globals {
        idem_key: Some("k".into()),
        ..Globals::default()
    };
    let with = planning(false, vec![stand_in_pin()]);
    let without = planning(false, Vec::new());
    write_caption(&with, &globals).unwrap();
    write_caption(&without, &globals).unwrap();
    let (a, b) = (&with.sent.borrow()[0], &without.sent.borrow()[0]);
    assert_ne!(a.ops.len(), b.ops.len());
    assert!(a.args_hash.is_some());
    assert_eq!(a.args_hash, b.args_hash);
}

#[test]
fn offline_write_plans_no_pins() {
    let backend = planning(true, vec![stand_in_pin()]);
    let doc = write_caption(&backend, &Globals::default()).unwrap();
    assert_eq!(*backend.asked.borrow(), 0);
    assert_eq!(doc["dryRun"], true);
    assert_eq!(
        doc["resolved"]["fonts"],
        json!({ "planned": [], "measured": false })
    );
}

#[test]
fn writes_without_text_never_ask_for_pins() {
    let backend = planning(false, vec![stand_in_pin()]);
    let step = Write::Edit(Edit::Track(track::TrackCmd::Add(track::AddArgs {
        kind: "video".into(),
        index: None,
        name: None,
    })));
    let doc = write(&backend, &Globals::default(), step).unwrap();
    assert_eq!(*backend.asked.borrow(), 0);
    assert!(doc["resolved"].get("fonts").is_none());
    assert_eq!(backend.sent.borrow()[0].ops.len(), 1);
}
