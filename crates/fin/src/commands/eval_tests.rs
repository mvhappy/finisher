//! Unit tests of `fin eval`'s payload (review #52): the timeline resolved from the snapshot is
//! always sent, with the snapshot's rev, so the server never re-resolves a default.

use std::cell::RefCell;
use std::path::Path;

use fin_core::{CommittedTx, Rational, Tx, new_project};

use super::*;
use crate::backend::{Probed, Snapshot, TxLookup};

/// A project at rev 7 whose server records each project call and answers `answer`.
struct Recording {
    calls: RefCell<Vec<(String, Value)>>,
    answer: Result<Value, CoreError>,
}

fn recording(answer: Result<Value, CoreError>) -> Recording {
    Recording {
        calls: RefCell::default(),
        answer,
    }
}

impl Backend for Recording {
    fn snapshot(&self, _branch: &str) -> Result<Snapshot, CoreError> {
        let mut project = new_project("p".into(), "P".into(), "30".parse()?)?;
        let mut tl2 = project.timelines[0].clone();
        tl2.id = "tl2".into();
        tl2.rate = "30000/1001".parse()?;
        project.timelines.push(tl2);
        Ok(Snapshot {
            rev: 7,
            project,
            generation: None,
        })
    }
    fn commit(&self, _tx: &Tx, _generation: Option<&Value>) -> Result<CommittedTx, CoreError> {
        unreachable!()
    }
    fn tx_get(&self, _tx_id: &str) -> Result<TxLookup, CoreError> {
        unreachable!()
    }
    fn project_call(&self, tag: &str, payload: Value) -> Result<Value, CoreError> {
        self.calls.borrow_mut().push((tag.to_string(), payload));
        self.answer.clone()
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
}

fn globals(timeline: Option<&str>) -> Globals {
    Globals {
        branch: Some("main".into()),
        timeline: timeline.map(str::to_string),
        ..Globals::default()
    }
}

#[test]
fn the_default_timeline_is_sent_with_the_snapshot_rev() {
    let b = recording(Ok(json!({ "timeline": "tl1" })));
    let frame = EvalCmd::Frame(FrameArgs { at: "f30".into() });
    let out = run(&b, &globals(None), &frame).unwrap();
    assert_eq!(out["timeline"], "tl1");
    let audio = EvalCmd::Audio(AudioArgs {
        range: "0..1s".into(),
    });
    run(&b, &globals(None), &audio).unwrap();
    let calls = b.calls.borrow();
    assert_eq!(calls[0].0, "eval.frame");
    assert_eq!(
        calls[0].1,
        json!({ "branch": "main", "timeline": "tl1", "rev": 7, "at": "1" })
    );
    assert_eq!(calls[1].0, "eval.audio");
    assert_eq!(
        calls[1].1,
        json!({ "branch": "main", "timeline": "tl1", "rev": 7, "from": "0", "to": "1" })
    );
}

#[test]
fn a_named_timeline_counts_frames_at_its_rate() {
    let b = recording(Ok(json!({ "timeline": "tl2" })));
    let frame = EvalCmd::Frame(FrameArgs { at: "f30".into() });
    let out = run(&b, &globals(Some("tl2")), &frame).unwrap();
    assert_eq!(out["timeline"], "tl2");
    let calls = b.calls.borrow();
    assert_eq!(
        calls[0].1,
        json!({ "branch": "main", "timeline": "tl2", "rev": 7, "at": "1001/1000" })
    );
}

#[test]
fn a_timeline_removed_meanwhile_is_a_retryable_conflict() {
    let gone = CoreError::new("CONFLICT", "timeline tl1 was removed")
        .with_details(json!({ "reason": "TIMELINE_GONE", "timeline": "tl1" }))
        .with_fix("read the project again and run the command again");
    let b = recording(Err(gone));
    let frame = EvalCmd::Frame(FrameArgs { at: "1s".into() });
    let e = run(&b, &globals(None), &frame).unwrap_err();
    assert_eq!(e.code, "CONFLICT");
    assert_eq!(crate::output::exit_code(&e.code), crate::output::CONFLICT);
}
