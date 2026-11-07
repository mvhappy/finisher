//! Export and the upload view (plan D, Task D3; Decisions 30 and 31): what is stored, or
//! exactly what an uploader may ship now: closed files, the current consent, the projects'
//! current flags, content only with training stamped, current and asked for.

mod support;

use fin_core::events::{CollectPolicy, Purposes, Source};
use fin_journal::{Filter, Journal, JournalOptions};
use support::*;

/// Options that close a file after a few entries.
fn small(c: &std::sync::Arc<fin_journal::ManualClock>) -> JournalOptions {
    JournalOptions {
        max_file_bytes: 1500,
        ..opts(c)
    }
}

/// Starts a new file, so everything recorded before is in closed files.
fn rotate(j: &Journal, c: &fin_journal::ManualClock) {
    c.advance(DAY);
    j.record(Source::FinNode, None, commit(0));
    j.flush().unwrap();
}

#[test]
fn export_as_upload_applies_current_consent() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), training(), small(&c));
    let b = j
        .attach_blob(
            vec![5u8; 128].into(),
            fin_core::events::names::MEDIA_TYPES
                .get("image/png")
                .unwrap(),
        )
        .unwrap();
    j.record(Source::FinNode, None, commit_with_intent(2));
    j.record(Source::Agent, None, tool_result(Some(b)));
    rotate(&j, &c);
    let f = Filter::default();
    // Training on now: the training view has the content.
    let up: Vec<_> = j.export(&f, Some(Purposes::TRAINING)).collect();
    assert!(up[0].payload.get("intent").is_some());
    assert!(up[1].payload["output"]["blob"]["hash"].is_string());
    // The diagnostics view never has content.
    let up: Vec<_> = j.export(&f, Some(Purposes::DIAGNOSTICS)).collect();
    assert!(up[0].payload.get("intent").is_none());
    assert_eq!(up[0].consent.purposes, Purposes::DIAGNOSTICS);
    // Training withdrawn: no content and no blob reference, whatever was stamped.
    j.set_gate_for_test(diagnostics());
    let up: Vec<_> = j.export(&f, Some(Purposes::ALL)).collect();
    assert!(!up.is_empty());
    for e in &up {
        assert!(!e.consent.purposes.has_training(), "{e:?}");
        let text = serde_json::to_string(&e.payload).unwrap();
        assert!(!text.contains("intent") && !text.contains("kb:"), "{text}");
    }
    assert!(j.export(&f, Some(Purposes::TRAINING)).next().is_none());
    // The stored view still has everything: it is the user's own data.
    let stored: Vec<_> = j.export(&f, None).collect();
    assert!(stored[0].payload.get("intent").is_some());
    // Everything withdrawn: nothing to upload.
    j.set_gate_for_test(fin_journal::Consent::OFF);
    assert!(j.export(&f, Some(Purposes::ALL)).next().is_none());
}

#[test]
fn export_as_upload_reads_closed_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.record(Source::FinNode, None, commit(1));
    j.flush().unwrap();
    let f = Filter::default();
    assert_eq!(j.export(&f, None).count(), 1);
    assert_eq!(
        j.export(&f, Some(Purposes::DIAGNOSTICS)).count(),
        0,
        "still open"
    );
    rotate(&j, &c);
    let up: Vec<_> = j.export(&f, Some(Purposes::DIAGNOSTICS)).collect();
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].payload["micros"], 1);
}

#[test]
fn export_as_upload_skips_never_and_unknown_projects_now() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), opts(&c));
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    j.set_project_policy(P2, Some(CollectPolicy::Default));
    j.record(Source::FinNode, in_project(P1), commit(1));
    j.record(Source::FinNode, in_project(P2), commit(2));
    j.record(Source::FinNode, None, commit(3));
    rotate(&j, &c);
    let f = Filter::default();
    let micros = |j: &Journal| -> Vec<u64> {
        j.export(&f, Some(Purposes::DIAGNOSTICS))
            .map(|e| e.payload["micros"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(micros(&j), [1, 2, 3], "the open file is not in the view");
    // P2 closes (unknown now): kept on disk, not uploaded.
    j.set_project_policy(P2, None);
    assert_eq!(micros(&j), [1, 3]);
    assert_eq!(j.export(&f, None).count(), 4, "nothing purged");
    // P1 becomes never: purged, so gone from both.
    j.set_project_policy(P1, Some(CollectPolicy::Never));
    // (The purge closed the open file, so its entry is in the view now.)
    assert_eq!(micros(&j), [3, 0]);
    assert_eq!(j.export(&f, None).count(), 3);
}

#[test]
fn export_limit_keeps_newest() {
    let dir = tempfile::tempdir().unwrap();
    let c = clock();
    let j = Journal::open(&events(&dir), diagnostics(), small(&c));
    j.set_project_policy(P1, Some(CollectPolicy::Default));
    for i in 1..=30 {
        let ctx = if i % 2 == 0 { in_project(P1) } else { None };
        j.record(Source::FinNode, ctx, commit(i));
    }
    rotate(&j, &c);
    let f = Filter {
        limit: Some(5),
        project: Some(P1.to_uppercase()),
        ..Filter::default()
    };
    for view in [None, Some(Purposes::DIAGNOSTICS)] {
        let got: Vec<u64> = j
            .export(&f, view)
            .map(|e| e.payload["micros"].as_u64().unwrap())
            .collect();
        assert_eq!(got, [22, 24, 26, 28, 30], "{view:?}: newest last");
    }
    let since = Filter {
        since: Some(T0 + 3_600_000),
        kinds: vec!["log.commit".into()],
        limit: Some(0),
        ..Filter::default()
    };
    assert_eq!(j.export(&since, None).count(), 0);
}
