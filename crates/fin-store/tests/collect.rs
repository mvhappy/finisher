//! The project's collection flag (plan D, Task D3; Decision 13): `finisher.project.json`
//! `collect`, written by the lease holder, cached by every store, re-read by followers (one
//! `collect` event per change), kept by older Finishers' rewrites, and left as it was when
//! the file cannot be read.

mod integration_support;

use std::fs;

use fin_store::{CollectPolicy, ProjectFile, Store, StoreEvent};
use integration_support::*;

fn collects(events: &[StoreEvent]) -> Vec<CollectPolicy> {
    events
        .iter()
        .filter_map(|e| match e {
            StoreEvent::Collect(p) => Some(*p),
            _ => None,
        })
        .collect()
}

fn file_json(e: &Env) -> serde_json::Value {
    serde_json::from_slice(&fs::read(e.dir.join("finisher.project.json")).unwrap()).unwrap()
}

fn poll_events(e: &Env, s: &mut Store) -> Vec<StoreEvent> {
    e.poll(s).events
}

#[test]
fn set_collect_needs_lease() {
    let e = env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    assert_eq!(held_epoch(&b), None, "b follows");
    let err = fin_core::CoreError::from(b.set_collect(CollectPolicy::Never).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY", "{err:?}");
    assert!(file_json(&e).get("collect").is_none(), "nothing written");
    assert_eq!(b.collect_policy(), CollectPolicy::Default);
    a.set_collect(CollectPolicy::Never).unwrap();
    assert_eq!(a.collect_policy(), CollectPolicy::Never);
    assert_eq!(file_json(&e)["collect"], "never");
}

#[test]
fn collect_round_trips_and_older_file_keeps_it() {
    let e = env();
    let mut a = e.create("a");
    assert_eq!(a.collect_policy(), CollectPolicy::Default);
    // A key from a newer Finisher survives our rewrite, as ours survives an older one's
    // (`ProjectFile.other`).
    edit_project_file(&e.dir, |f| {
        f.insert("futureKey".into(), serde_json::json!({ "x": 1 }));
    });
    a.set_collect(CollectPolicy::Never).unwrap();
    let f = file_json(&e);
    assert_eq!(
        (f["collect"].as_str(), f["futureKey"]["x"].as_i64()),
        (Some("never"), Some(1))
    );
    drop(a);
    let mut a = e.open("a");
    assert_eq!(a.collect_policy(), CollectPolicy::Never, "read at open");
    a.set_collect(CollectPolicy::Default).unwrap();
    let f = file_json(&e);
    assert!(f.get("collect").is_none(), "default is written as no key");
    assert_eq!(f["futureKey"]["x"], 1);
    // An older Finisher's rewrite keeps `collect` in `other`; a typed read sees it again.
    let older = r#"{"id":"x","name":"n","schemaVersion":1,"collect":"never","z":2}"#;
    let pf: ProjectFile = serde_json::from_str(older).unwrap();
    assert_eq!(fin_store::policy_of(&pf), CollectPolicy::Never);
    let back = serde_json::to_value(&pf).unwrap();
    assert_eq!(
        (back["collect"].as_str(), back["z"].as_i64()),
        (Some("never"), Some(2))
    );
    // `"default"` reads as default; a value this build does not know is honoured as never.
    for (v, want) in [
        ("default", CollectPolicy::Default),
        ("someday", CollectPolicy::Never),
    ] {
        let pf: ProjectFile = serde_json::from_value(serde_json::json!({
            "id": "x", "name": "n", "schemaVersion": 1, "collect": v,
        }))
        .unwrap();
        assert_eq!(fin_store::policy_of(&pf), want, "{v}");
    }
}

#[test]
fn follower_reads_collect_after_reread() {
    let e = env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    assert!(collects(&poll_events(&e, &mut b)).is_empty());
    a.set_collect(CollectPolicy::Never).unwrap();
    // The holder's own change is queued once.
    assert_eq!(collects(&poll_events(&e, &mut a)), [CollectPolicy::Never]);
    assert!(collects(&poll_events(&e, &mut a)).is_empty());
    // The follower finds it at its next re-read, once.
    assert_eq!(collects(&poll_events(&e, &mut b)), [CollectPolicy::Never]);
    assert_eq!(b.collect_policy(), CollectPolicy::Never);
    for _ in 0..3 {
        assert!(
            collects(&poll_events(&e, &mut b)).is_empty(),
            "once per change"
        );
    }
    a.set_collect(CollectPolicy::Default).unwrap();
    assert_eq!(collects(&poll_events(&e, &mut b)), [CollectPolicy::Default]);
    // The wire form.
    let ev = fin_store::EngineEvent::from_store(&StoreEvent::Collect(CollectPolicy::Never), None);
    assert_eq!(
        serde_json::to_value(&ev).unwrap(),
        serde_json::json!({ "type": "collect", "collect": "never" })
    );
}

#[test]
fn unreadable_project_file_keeps_cached_collect() {
    let e = env();
    let mut a = e.create("a");
    let mut b = e.open("b");
    a.set_collect(CollectPolicy::Never).unwrap();
    assert_eq!(collects(&poll_events(&e, &mut b)), [CollectPolicy::Never]);
    let path = e.dir.join("finisher.project.json");
    let good = fs::read(&path).unwrap();
    fs::write(&path, b"{ not json").unwrap();
    let events = poll_events(&e, &mut b);
    assert!(collects(&events).is_empty(), "{events:?}");
    assert_eq!(b.collect_policy(), CollectPolicy::Never);
    fs::write(&path, good).unwrap();
    assert!(collects(&poll_events(&e, &mut b)).is_empty());
    assert_eq!(b.collect_policy(), CollectPolicy::Never);
}

#[test]
fn set_collect_refused_when_the_lease_went_between_ticks() {
    use fin_store::lease::record;
    let e = env();
    let mut a = e.create("a");
    assert_eq!(a.read_only(), None, "a holds the lease");
    // Another server takes the record over; `a` has not ticked since.
    let dot = e.dir.join(".finisher");
    let mut r = record::decode(&fs::read(dot.join("lease.json")).unwrap()).unwrap();
    (r.server_id, r.epoch) = ("serverb".into(), r.epoch + 1);
    fin_store::fsio::write_in_place(&dot.join("lease.json"), &record::encode(&r)).unwrap();
    let before = fs::read(e.dir.join("finisher.project.json")).unwrap();
    let err = fin_core::CoreError::from(a.set_collect(CollectPolicy::Never).unwrap_err());
    assert_eq!(err.code, "PROJECT_READ_ONLY", "{err:?}");
    assert_eq!(err.details["reason"], "LEASE_LOST");
    assert_eq!(
        fs::read(e.dir.join("finisher.project.json")).unwrap(),
        before
    );
    assert_eq!(a.collect_policy(), CollectPolicy::Default);
    let leftovers: Vec<_> = fs::read_dir(&e.dir)
        .unwrap()
        .map(|d| d.unwrap().file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
