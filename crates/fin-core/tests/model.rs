use fin_core::{
    Clip, IdAlloc, IdKind, MemIds, Project, Rational, SCHEMA_VERSION, TrackKind, json_schema,
    new_project,
};

fn rate() -> Rational {
    "30000/1001".parse().unwrap()
}

#[test]
fn new_project_has_default_timeline_and_tracks() {
    let p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    assert_eq!(p.timeline.id, "tl1");
    assert_eq!(p.timeline.tracks.len(), 2);
    assert_eq!(
        (p.timeline.tracks[0].id.as_str(), p.timeline.tracks[0].kind),
        ("t1", TrackKind::Video)
    );
    assert_eq!(
        (p.timeline.tracks[1].id.as_str(), p.timeline.tracks[1].kind),
        ("t2", TrackKind::Audio)
    );
    assert_eq!(p.rate, rate());
    assert_eq!(p.schema_version, SCHEMA_VERSION);
}

#[test]
fn project_json_round_trips_byte_identically() {
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    p.timeline.tracks[0].items.push(Clip {
        id: "c1".into(),
        asset: "a1".into(),
        start: Rational::zero(),
        source_in: "1/2".parse().unwrap(),
        duration: "62/5".parse().unwrap(),
    });
    let json = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, p);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert!(json.contains("\"schemaVersion\":1") && json.contains("\"sourceIn\":\"1/2\""));
}

#[test]
fn schema_has_the_contract_defs() {
    let schema = json_schema();
    let defs = schema["$defs"].as_object().unwrap();
    for name in ["Project", "Tx", "Op", "Diff", "CoreError", "CommittedTx"] {
        assert!(defs.contains_key(name), "missing {name}");
    }
    assert_eq!(defs["Rational"]["type"], "string");
    assert_eq!(schema, json_schema(), "schema must be deterministic");
}

#[test]
fn mem_ids_continue_after_a_project() {
    let p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    let mut ids = MemIds::after(&p);
    assert_eq!(ids.next(IdKind::Timeline), "tl2");
    assert_eq!(ids.next(IdKind::Track), "t3");
    assert_eq!(ids.next(IdKind::Track), "t4");
    assert_eq!(ids.next(IdKind::Clip), "c1");
    assert_eq!(ids.next(IdKind::Asset), "a1");
}

#[test]
fn mem_ids_skip_past_highest_suffix() {
    let mut p = new_project("p1".into(), "Demo".into(), rate()).unwrap();
    p.timeline.tracks[0].items.push(Clip {
        id: "c7".into(),
        asset: "a9".into(),
        start: Rational::zero(),
        source_in: Rational::zero(),
        duration: Rational::from_int(1),
    });
    let mut ids = MemIds::after(&p);
    assert_eq!(ids.next(IdKind::Clip), "c8");
}

#[test]
fn new_project_rejects_non_positive_rate() {
    for bad in [0, -30] {
        let e = new_project("p".into(), "n".into(), Rational::from_int(bad)).unwrap_err();
        assert_eq!(e.code, "INVALID_RANGE");
    }
}

#[test]
fn schema_requires_details_and_keeps_rational_refs() {
    let schema = json_schema();
    let err = &schema["$defs"]["CoreError"];
    let required: Vec<&str> = err["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(required.contains(&"details"), "{required:?}");
    assert!(err["properties"].get("details").is_some());
    for (def, field) in [
        ("Project", "rate"),
        ("Clip", "start"),
        ("Clip", "sourceIn"),
        ("Clip", "duration"),
    ] {
        let f = &schema["$defs"][def]["properties"][field];
        assert_eq!(f["$ref"], "#/$defs/Rational", "{def}.{field}");
    }
}
