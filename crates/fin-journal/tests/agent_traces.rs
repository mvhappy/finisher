//! The agent-trace store (sp7 §12.1; plan 7A Task 10): a second journal of the same code that
//! records training only, takes its caps from its own settings keys, evicts whole agent
//! sessions and follows the diagnostics journal's project policies.

mod support;

use std::sync::Arc;

use fin_core::events::names::MEDIA_TYPES;
use fin_core::events::{
    Actor, ActorKind, Basis, BlobRef, CollectPolicy, Ctx, IdRef, ProjectId, Purposes, Source,
};
use fin_journal::{
    CapsKeys, Consent, EnvOverrides, Journal, JournalOptions, Switch, TRACES_MAX_AGE,
    TRACES_MAX_BYTES,
};
use serde_json::json;
use support::*;

const A: &str = "01JS00000000000000000000AA";
const B: &str = "01JS00000000000000000000BB";

fn traces(c: &Arc<fin_journal::ManualClock>) -> JournalOptions {
    JournalOptions {
        clock: c.clone(),
        ..JournalOptions::agent_traces()
    }
}

fn agent(session: &str, project: Option<&str>) -> Option<Ctx> {
    Some(Ctx {
        project: project.and_then(ProjectId::parse),
        branch: None,
        tx_id: None,
        author: None,
        actor: Some(Actor {
            kind: ActorKind::Agent,
            session: IdRef::classify(session).ok(),
            call: None,
        }),
    })
}

fn sessions_on_disk(dir: &std::path::Path) -> Vec<String> {
    raw_lines(dir)
        .iter()
        .filter_map(|e| e.ctx.as_ref()?.actor.as_ref()?.session.as_ref())
        .map(|s| s.as_str().to_owned())
        .collect()
}

#[test]
fn defaults_are_two_gib_ninety_days_training_only() {
    let o = JournalOptions::agent_traces();
    assert_eq!(o.max_total_bytes, TRACES_MAX_BYTES);
    assert_eq!(TRACES_MAX_BYTES, 2 << 30);
    assert_eq!(o.max_age, TRACES_MAX_AGE);
    assert_eq!(TRACES_MAX_AGE.as_secs(), 90 * 86_400);
    assert_eq!(o.purposes, Purposes::TRAINING);
    assert_eq!(o.caps_keys, CapsKeys::AgentTraces);
    assert!(o.whole_sessions);
    // The diagnostics journal's own rules are unchanged.
    let d = JournalOptions::default();
    assert_eq!(
        (d.max_total_bytes, d.max_age.as_secs()),
        (128 << 20, 30 * 86_400)
    );
    assert_eq!(
        (d.purposes, d.caps_keys, d.whole_sessions),
        (Purposes::ALL, CapsKeys::Diagnostics, false)
    );
}

#[test]
fn records_nothing_and_makes_no_directory_without_training() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), traces(&c));
    assert_eq!(
        j.gate(),
        None,
        "diagnostics alone is off for the trace store"
    );
    assert_eq!(j.consent().diagnostics, Switch::Off);
    j.record(Source::Agent, agent(A, None), tool_result(None));
    assert!(
        j.attach_blob(
            Arc::from(&b"x"[..]),
            MEDIA_TYPES.get("application/json").unwrap()
        )
        .is_none()
    );
    j.flush().unwrap();
    assert!(!events(&dir).exists());

    // With training, it records, under training alone.
    let dir = tempfile::tempdir().unwrap();
    let j = Journal::open(&events(&dir), training(), traces(&c));
    assert_eq!(j.gate(), Some(Purposes::TRAINING));
    j.record(Source::Agent, agent(A, None), tool_result(None));
    j.flush().unwrap();
    let all = raw_lines(&events(&dir));
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].consent.purposes, Purposes::TRAINING);
    assert!(
        all[0].payload["output"].is_object(),
        "content kept: {:?}",
        all[0].payload
    );
}

#[test]
fn caps_come_from_the_trace_keys() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let privacy = json!({"training": "on", "maxMiB": 16, "retentionDays": 1, "tracesMaxMiB": 64, "tracesRetentionDays": 7});
    let (j, w) = Journal::open_with_settings(
        &events(&dir),
        Some(&privacy),
        EnvOverrides::default(),
        traces(&c),
    );
    assert_eq!(w, None);
    assert_eq!(j.caps(), (7 * 86_400_000, 64 << 20));
    // The diagnostics journal reads its own keys from the same section.
    let dir2 = tempfile::tempdir().unwrap();
    let (d, _) = Journal::open_with_settings(
        &events(&dir2),
        Some(&privacy),
        EnvOverrides::default(),
        opts(&c),
    );
    assert_eq!(d.caps(), (86_400_000, 16 << 20));
    // Out of range is refused like the diagnostics keys.
    for bad in [
        json!({"tracesMaxMiB": 63}),
        json!({"tracesRetentionDays": 0}),
    ] {
        assert!(
            fin_journal::PrivacySettings::parse(Some(&bad)).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn eviction_removes_whole_sessions_and_their_late_entries() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 6000,
        ..traces(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    // Session A starts first; B overlaps it; then B alone fills the store.
    for _ in 0..4 {
        j.record(Source::Agent, agent(A, None), tool_result(None));
    }
    for _ in 0..4 {
        j.record(Source::Agent, agent(A, None), tool_result(None));
        j.record(Source::Agent, agent(B, None), tool_result(None));
    }
    j.flush().unwrap();
    assert!(sessions_on_disk(&events(&dir)).contains(&A.to_owned()));
    for _ in 0..40 {
        j.record(Source::Agent, agent(B, None), tool_result(None));
        j.flush().unwrap();
    }
    let on_disk = sessions_on_disk(&events(&dir));
    assert!(!on_disk.contains(&A.to_owned()), "A went whole");
    assert!(on_disk.contains(&B.to_owned()));
    // A late entry of an evicted session is not written.
    j.record(Source::Agent, agent(A, None), tool_result(None));
    j.flush().unwrap();
    assert!(!sessions_on_disk(&events(&dir)).contains(&A.to_owned()));
}

#[test]
fn evicted_sessions_are_remembered_across_a_restart() {
    // Task 10 review M2: the evicted ids are in `state.json`, so a late entry of an evicted
    // session is dropped by the next run of the store too.
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = || JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 6000,
        ..traces(&c)
    };
    let j = Journal::open(&events(&dir), training(), o());
    for _ in 0..4 {
        j.record(Source::Agent, agent(A, None), tool_result(None));
    }
    j.flush().unwrap();
    for _ in 0..40 {
        j.record(Source::Agent, agent(B, None), tool_result(None));
        j.flush().unwrap();
    }
    assert!(
        !sessions_on_disk(&events(&dir)).contains(&A.to_owned()),
        "A evicted"
    );
    j.close();
    let state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(events(&dir).join("state.json")).unwrap()).unwrap();
    assert!(
        state["evictedSessions"]
            .as_array()
            .unwrap()
            .contains(&json!(A)),
        "{state}"
    );
    let j = Journal::open(&events(&dir), training(), o());
    j.record(Source::Agent, agent(A, None), tool_result(None));
    j.record(Source::Agent, agent(B, None), tool_result(None));
    j.flush().unwrap();
    let on_disk = sessions_on_disk(&events(&dir));
    assert!(
        !on_disk.contains(&A.to_owned()),
        "a late A entry was written after the restart"
    );
    assert!(on_disk.contains(&B.to_owned()));
    j.close();
}

#[test]
fn the_diagnostics_journal_never_evicts_by_session() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 6000,
        ..opts(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    j.record(Source::Agent, agent(A, None), tool_result(None));
    for _ in 0..60 {
        j.record(Source::Agent, agent(B, None), tool_result(None));
        j.flush().unwrap();
    }
    // A's file went by size; a late A entry is still written (no session memory here).
    j.record(Source::Agent, agent(A, None), tool_result(None));
    j.flush().unwrap();
    assert!(sessions_on_disk(&events(&dir)).contains(&A.to_owned()));
}

#[test]
fn a_blob_shared_by_two_sessions_outlives_one_of_them() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let o = JournalOptions {
        max_file_bytes: 2048,
        max_total_bytes: 9000,
        ..traces(&c)
    };
    let j = Journal::open(&events(&dir), training(), o);
    let media = MEDIA_TYPES.get("application/json").unwrap();
    let system: Arc<[u8]> = Arc::from(&br#"{"role":"system","content":"You edit timelines."}"#[..]);
    let r1: BlobRef = j.attach_blob(system.clone(), media).unwrap();
    j.record(Source::Agent, agent(A, None), tool_result(Some(r1.clone())));
    j.flush().unwrap();
    // B starts on the next UTC day: a file of its own, so evicting A's file keeps B.
    c.set(T0 + DAY.as_millis() as u64 + 1000);
    let r2 = j.attach_blob(system, media).unwrap();
    assert_eq!(r1, r2, "content addressed");
    j.record(Source::Agent, agent(B, None), tool_result(Some(r2)));
    j.flush().unwrap();
    assert_eq!(blob_files(&events(&dir)).len(), 1);
    // Fill with B until A is evicted: the blob stays, B still references it.
    for _ in 0..80 {
        if !sessions_on_disk(&events(&dir)).contains(&A.to_owned()) {
            break;
        }
        j.record(Source::Agent, agent(B, None), tool_result(None));
        j.flush().unwrap();
    }
    assert!(!sessions_on_disk(&events(&dir)).contains(&A.to_owned()));
    let refs = raw_lines(&events(&dir))
        .iter()
        .filter(|e| e.payload["output"]["blob"].is_object())
        .count();
    assert_eq!(refs, 1, "B's reference is still there");
    assert_eq!(blob_files(&events(&dir)).len(), 1);
}

#[test]
fn follows_the_diagnostics_journals_project_policies() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let diag = Journal::open(&dir.path().join("events"), training(), opts(&c));
    // Known before the trace store opens: replayed to it.
    let _p1 = diag.open_project(P1, CollectPolicy::Default);
    let t = Journal::open(&dir.path().join("agent-traces"), training(), traces(&c));
    t.follow_projects(&diag);
    assert!(t.records_project(P1));
    assert!(!t.records_project(P2), "unknown");
    // Told after: forwarded.
    let p2 = diag.open_project(P2, CollectPolicy::Default);
    assert!(t.records_project(P2));
    t.record(Source::Agent, agent(A, Some(P1)), tool_result(None));
    t.record(Source::Agent, agent(B, Some(P2)), tool_result(None));
    t.flush().unwrap();
    assert_eq!(raw_lines(&dir.path().join("agent-traces")).len(), 2);
    // `never` on the leader purges the follower's entries of that project.
    diag.set_project_policy(P1, Some(CollectPolicy::Never));
    assert!(!t.records_project(P1));
    t.record(Source::Agent, agent(A, Some(P1)), tool_result(None));
    t.flush().unwrap();
    let left = sessions_on_disk(&dir.path().join("agent-traces"));
    assert_eq!(left, vec![B.to_owned()]);
    // The last scope closing makes the project unknown for both.
    drop(p2);
    diag.flush().unwrap();
    assert!(!t.records_project(P2));
    assert!(!diag.records_project(P2));
}

#[test]
fn a_masked_low_level_consent_keeps_its_basis() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(
        &events(&dir),
        Consent {
            purposes: Purposes::ALL,
            rev: 3,
            basis: Basis::User,
        },
        traces(&c),
    );
    let s = j.consent();
    assert_eq!(s.diagnostics, Switch::Off);
    assert_eq!(s.basis, Basis::User);
    assert_eq!(s.purposes(), Purposes::TRAINING);
}

#[test]
fn a_follower_keys_ids_with_the_leaders_key_but_blobs_with_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let diag = Journal::open(&dir.path().join("events"), training(), opts(&c));
    let t = Journal::open(
        &dir.path().join("agent-traces"),
        training(),
        JournalOptions {
            key: diag.shared_keyer(),
            ..traces(&c)
        },
    );
    assert_eq!(t.hash("tx-1"), diag.hash("tx-1"));
    assert_eq!(t.id_ref("not-a-ulid"), diag.id_ref("not-a-ulid"));
    // After `purge all` the leader keys with a new key, and so does the follower.
    let before = diag.hash("tx-1");
    diag.purge(fin_journal::PurgeSelector::All).unwrap();
    assert_ne!(diag.hash("tx-1"), before);
    assert_eq!(t.hash("tx-1"), diag.hash("tx-1"));
    // Blob addresses are the store's own key's.
    let media = MEDIA_TYPES.get("application/json").unwrap();
    let a = t.attach_blob(Arc::from(&b"{}"[..]), media).unwrap();
    let b = diag.attach_blob(Arc::from(&b"{}"[..]), media).unwrap();
    assert_ne!(a.hash, b.hash);
}
