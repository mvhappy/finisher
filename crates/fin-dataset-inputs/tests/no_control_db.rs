//! `dataset_builder_never_reads_control_db` (plan 7A Task 10; sp7 §12.1, §16.2): the
//! builder's input readers, run against a server-data directory that holds a `control.db`
//! beside both stores, open only the stores' files.

use std::cell::RefCell;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use fin_core::events::names::MEDIA_TYPES;
use fin_core::events::payloads::{AgentToolResult, Attribution, Body};
use fin_core::events::{Basis, Content, IdRef, Purposes, Source};
use fin_dataset_inputs::{Disk, Files, Store, blob, entries};
use fin_journal::{Consent, Journal, JournalOptions};
use serde_json::json;

/// Records every path listed or read.
#[derive(Default)]
struct Spy {
    touched: RefCell<Vec<PathBuf>>,
}

impl Files for Spy {
    fn list(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        self.touched.borrow_mut().push(dir.to_path_buf());
        Disk.list(dir)
    }
    fn read(&self, file: &Path) -> io::Result<Vec<u8>> {
        self.touched.borrow_mut().push(file.to_path_buf());
        Disk.read(file)
    }
}

fn result(output: Body) -> fin_core::events::AnyPayload {
    AgentToolResult {
        call_id: IdRef::classify("01JC0000000000000000000009").unwrap(),
        ok: true,
        code: None,
        exit: None,
        micros: 7,
        tx_ids: vec![],
        attribution: Attribution::Exact,
        provenance: None,
        truncated: false,
        output: Some(Content(output)),
    }
    .into()
}

#[test]
fn dataset_builder_never_reads_control_db() {
    let dir = tempfile::tempdir().unwrap();
    let sd = dir.path();
    std::fs::write(sd.join("control.db"), b"SQLite format 3\0").unwrap();
    std::fs::create_dir_all(sd.join("projects")).unwrap();
    let on = Consent {
        purposes: Purposes::ALL,
        rev: 1,
        basis: Basis::User,
    };
    let diag = Journal::open(&sd.join("events"), on, JournalOptions::default());
    let traces = Journal::open(&sd.join("agent-traces"), on, JournalOptions::agent_traces());
    diag.record(
        Source::Agent,
        None,
        result(Body::Inline(json!({"text": "meta"}))),
    );
    let r = traces
        .attach_blob(
            Arc::from(&br#"{"messages":[]}"#[..]),
            MEDIA_TYPES.get("application/json").unwrap(),
        )
        .unwrap();
    traces.record(Source::Agent, None, result(Body::Blob(r.clone())));
    diag.close();
    traces.close();

    let spy = Spy::default();
    let d = entries(&spy, sd, Store::Diagnostics).unwrap();
    let t = entries(&spy, sd, Store::AgentTraces).unwrap();
    assert!(d.iter().any(|e| e.kind.as_str() == "agent.toolResult"));
    assert_eq!(
        t.iter()
            .filter(|e| e.kind.as_str() == "agent.toolResult")
            .count(),
        1
    );
    assert!(
        blob(&spy, sd, Store::AgentTraces, r.hash.hex())
            .unwrap()
            .is_some()
    );

    let touched = spy.touched.borrow();
    assert!(!touched.is_empty());
    for p in touched.iter() {
        assert!(!p.to_string_lossy().contains("control.db"), "{p:?}");
        assert!(
            p.starts_with(sd.join("events")) || p.starts_with(sd.join("agent-traces")),
            "only the two stores: {p:?}"
        );
    }
}
