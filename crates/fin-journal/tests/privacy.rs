//! Redaction at write and the project flag (plan D, Task D3; Decisions 10–14): what reaches
//! the disk never holds more than the consent in force when it is recorded and when it is
//! written; a `never` or unknown project records nothing, and only `never` purges.

mod support;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use fin_core::events::{Basis, CollectPolicy, Purposes, Source};
use fin_journal::{EnvOverrides, Journal, JournalOptions, ProjectPolicy};
use serde_json::json;
use support::*;

fn open(dir: &tempfile::TempDir, privacy: serde_json::Value, o: JournalOptions) -> Journal {
    Journal::open_with_settings(&events(dir), Some(&privacy), EnvOverrides::default(), o).0
}

fn has_intent(e: &fin_core::events::Envelope) -> bool {
    e.payload.get("intent").is_some()
}

#[test]
fn withdrawal_is_recorded_then_takes_effect() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({"training": "on"}), opts(&c));
    j.record(Source::FinNode, None, commit_with_intent(2));
    let before = j.consent();
    j.apply_settings(Some(&json!({"training": "off"})));
    assert_eq!(
        j.gate(),
        Some(Purposes::DIAGNOSTICS),
        "the gate moves at once"
    );
    j.record(Source::FinNode, None, commit_with_intent(2));
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    let kinds: Vec<_> = all.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["log.commit", "consent.changed", "log.commit"]);
    let changed = &all[1];
    assert_eq!(
        changed.consent.rev, before.rev,
        "stamped with the old consent"
    );
    assert_eq!(changed.consent.purposes, Purposes::ALL);
    assert_eq!(changed.payload["from"], json!(["diagnostics", "training"]));
    assert_eq!(changed.payload["to"], json!(["diagnostics"]));
    assert_eq!(changed.payload["rev"], before.rev + 1);
    assert!(!has_intent(&all[2]), "content stops after the withdrawal");
    // Withdrawing everything is still journaled, under the old consent.
    j.apply_settings(Some(&json!({"training": "off", "diagnostics": "off"})));
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    let last = all.last().unwrap();
    assert_eq!(last.kind.as_str(), "consent.changed");
    assert_eq!(last.payload["to"], json!([]));
    assert_eq!(last.consent.purposes, Purposes::DIAGNOSTICS);
}

#[test]
fn queued_content_after_withdrawal_never_hits_disk() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        channel: 64,
        ..opts(&c)
    };
    let j = open(&dir, json!({"training": "on"}), o);
    // Something on disk first, so the writer is warm.
    j.record(Source::FinNode, None, commit(0));
    j.flush().unwrap();
    assert!(j.pause_writer());
    for _ in 0..100 {
        j.record(Source::FinNode, None, commit_with_intent(4));
    }
    j.apply_settings(Some(&json!({"training": "off"})));
    j.resume_writer();
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    let commits = of_kind(&all, "log.commit");
    assert!(commits.len() > 1);
    for e in &commits[1..] {
        assert!(!has_intent(e), "queued content on disk");
        assert_eq!(e.consent.purposes, Purposes::DIAGNOSTICS);
    }
    let text: String = files(&events(&dir))
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    assert!(!text.contains("\"intent\""));
}

#[test]
fn consent_change_is_not_dropped_with_full_channel() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        channel: 8,
        ..opts(&c)
    };
    let j = open(&dir, json!({}), o);
    assert!(j.pause_writer());
    for i in 0..50 {
        j.record(Source::FinNode, None, commit(i));
    }
    j.apply_settings(Some(&json!({"training": "on"})));
    j.apply_settings(Some(&json!({"training": "off"})));
    j.resume_writer();
    let all = read_all(&j);
    assert_eq!(of_kind(&all, "consent.changed").len(), 2);
    assert!(
        reported_drops(&all) > 0,
        "data was dropped, control was not"
    );
}

#[test]
fn off_builds_no_payload() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({"diagnostics": "off"}), opts(&c));
    static BUILT: AtomicU64 = AtomicU64::new(0);
    let build = || {
        BUILT.fetch_add(1, Ordering::SeqCst);
        commit(1)
    };
    for _ in 0..100 {
        // The call sites' rule: check the gate before building.
        if j.gate().is_some() {
            j.record(Source::FinNode, None, build());
        }
    }
    assert_eq!(BUILT.load(Ordering::SeqCst), 0);
    j.apply_settings(Some(&json!({})));
    if j.gate().is_some() {
        j.record(Source::FinNode, None, build());
    }
    assert_eq!(BUILT.load(Ordering::SeqCst), 1);
}

#[test]
fn diagnostics_only_writes_no_content() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({"training": "off"}), opts(&c));
    j.record(Source::FinNode, None, commit_with_intent(3));
    j.record(Source::Agent, None, tool_result(None));
    let all = read_all(&j);
    assert_eq!(all.len(), 2);
    assert!(!has_intent(&all[0]));
    assert!(all[1].payload.get("output").is_none(), "{:?}", all[1]);
    assert_eq!(all[1].payload["micros"], 7, "the metadata stays");
}

#[test]
fn training_writes_content() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({"training": "on"}), opts(&c));
    j.record(Source::FinNode, None, commit_with_intent(3));
    j.record(Source::Agent, None, tool_result(None));
    let all = read_all(&j);
    assert!(has_intent(&all[0]));
    assert_eq!(
        all[1].payload["output"],
        json!({ "inline": { "text": "inline output" } })
    );
    assert_eq!(all[0].consent.purposes, Purposes::ALL);
    assert_eq!(all[0].consent.basis, Basis::User);
}

#[test]
fn stamp_is_recorded_purposes_intersect_gate() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        flush_every: Duration::from_secs(3600),
        flush_bytes: 1 << 20,
        ..opts(&c)
    };
    // Training only: entries keep both parts under [training].
    let j = open(&dir, json!({"training": "on", "diagnostics": "off"}), o);
    j.record(Source::FinNode, None, commit_with_intent(1));
    assert!(j.pause_writer());
    // Recorded under [training], written under [diagnostics]: nothing in common, dropped.
    j.set_gate_for_test(diagnostics());
    j.record(Source::FinNode, None, commit(2));
    j.resume_writer();
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    let commits = of_kind(&all, "log.commit");
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0].consent.purposes, Purposes::DIAGNOSTICS);
    assert_eq!(commits[0].payload["micros"], 2);
}

#[test]
fn never_project_entries_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({}), opts(&c));
    j.set_project_policy(P1, Some(CollectPolicy::Never));
    j.set_project_policy(&P2.to_uppercase(), Some(CollectPolicy::Default));
    j.record(Source::FinNode, in_project(P1), commit(1));
    j.record(Source::FinNode, in_project(P2), commit(2));
    j.record(Source::FinNode, None, commit(3));
    let all = read_all(&j);
    let micros: Vec<_> = all
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [2, 3]);
    assert_eq!(j.status().dropped, 0, "a policy is not a drop");
    let p = j.project_policies();
    assert_eq!(p.get(P1), Some(&ProjectPolicy::Never));
    assert_eq!(
        p.get(P2),
        Some(&ProjectPolicy::Default),
        "upper case is lowered"
    );
}

#[test]
fn unknown_project_drops_but_does_not_purge() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = open(&dir, json!({}), opts(&c));
    // Never heard of: unknown.
    j.record(Source::FinNode, in_project(P1), commit(1));
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    j.record(Source::FinNode, in_project(P1), commit(2));
    // Closed: unknown again. Entries recorded before it still go in.
    j.record(Source::FinNode, in_project(P1), commit(3));
    j.set_project_policy(P1, None);
    j.record(Source::FinNode, in_project(P1), commit(4));
    let all = read_all(&j);
    let micros: Vec<_> = all
        .iter()
        .map(|e| e.payload["micros"].as_u64().unwrap())
        .collect();
    assert_eq!(micros, [2, 3], "nothing purged, nothing new");
    assert_eq!(j.project_policies().get(P1), Some(&ProjectPolicy::Unknown));
}

#[test]
fn setting_never_purges_entries_and_blobs() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 4096,
        ..opts(&c)
    };
    let j = open(&dir, json!({"training": "on"}), o);
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    j.set_project_policy(P2, Some(CollectPolicy::Default));
    let bytes: std::sync::Arc<[u8]> = vec![7u8; 3000].into();
    let blob = j.attach_blob(bytes, media("image/png")).unwrap();
    for i in 0..30 {
        let project = if i % 2 == 0 { P1 } else { P2 };
        j.record(Source::FinNode, in_project(project), commit(i));
    }
    j.record(Source::Agent, in_project(P1), tool_result(Some(blob)));
    j.flush().unwrap();
    assert_eq!(blob_files(&events(&dir)).len(), 1);
    assert!(files(&events(&dir)).len() > 2, "entries span files");
    j.set_project_policy(P1, Some(CollectPolicy::Never));
    j.record(Source::FinNode, in_project(P1), commit(99));
    let all = read_all(&j);
    let in_p1 = all
        .iter()
        .filter(|e| {
            e.ctx
                .as_ref()
                .and_then(|c| c.project.as_ref())
                .map(|p| p.as_str())
                == Some(P1)
        })
        .count();
    assert_eq!(in_p1, 0, "every P1 entry purged, none new");
    assert_eq!(of_kind(&all, "log.commit").len(), 15, "P2 untouched");
    assert!(
        blob_files(&events(&dir)).is_empty(),
        "its blob went with it"
    );
    assert_eq!(
        raw_lines(&events(&dir)).len(),
        all.len(),
        "whole lines only"
    );
}

fn media(m: &str) -> fin_core::events::Name {
    fin_core::events::names::MEDIA_TYPES.get(m).unwrap()
}
