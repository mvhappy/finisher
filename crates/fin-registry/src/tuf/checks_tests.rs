//! The layout checks `tough` does not make, one case each (8A review).

use serde_json::{Value, json};
use tough::schema::{Root, Signed, Snapshot, Targets};

use super::{Layout, delegations, root};

const EXPIRES: &str = "2030-01-01T00:00:00Z";

fn signed<T: serde::de::DeserializeOwned>(v: Value) -> Signed<T> {
    serde_json::from_value(json!({ "signed": v, "signatures": [] })).unwrap()
}

fn root_with(spec: &str, consistent: bool) -> Signed<Root> {
    signed(json!({
        "_type": "root",
        "spec_version": spec,
        "consistent_snapshot": consistent,
        "version": 1,
        "expires": EXPIRES,
        "keys": {},
        "roles": {},
    }))
}

fn role(name: &str, paths: &[&str], terminating: bool) -> Value {
    json!({
        "name": name,
        "keyids": [],
        "threshold": 1,
        "paths": paths,
        "terminating": terminating,
    })
}

fn targets(names: &[&str], roles: Vec<Value>) -> Signed<Targets> {
    let target = json!({ "length": 1, "hashes": { "sha256": "00" } });
    let targets: serde_json::Map<String, Value> = names
        .iter()
        .map(|n| (n.to_string(), target.clone()))
        .collect();
    let mut v = json!({
        "_type": "targets",
        "spec_version": "1.0.0",
        "version": 1,
        "expires": EXPIRES,
        "targets": targets,
    });
    if !roles.is_empty() {
        v["delegations"] = json!({ "keys": {}, "roles": roles });
    }
    signed(v)
}

fn snapshot(listed: &[&str]) -> Signed<Snapshot> {
    let meta: serde_json::Map<String, Value> = std::iter::once("targets")
        .chain(listed.iter().copied())
        .map(|r| (format!("{r}.json"), json!({ "version": 1 })))
        .collect();
    signed(json!({
        "_type": "snapshot",
        "spec_version": "1.0.0",
        "version": 1,
        "expires": EXPIRES,
        "meta": meta,
    }))
}

/// `top` with each delegated role loaded as `loaded[name]`.
fn load(mut top: Signed<Targets>, loaded: &[(&str, Signed<Targets>)]) -> Signed<Targets> {
    if let Some(d) = top.signed.delegations.as_mut() {
        for r in &mut d.roles {
            r.targets = loaded
                .iter()
                .find(|(n, _)| *n == r.name)
                .map(|(_, t)| t.clone());
        }
    }
    top
}

fn check(top: &Signed<Targets>, listed: &[&str]) -> Result<(), (String, Option<String>)> {
    delegations(top, &snapshot(listed), Layout::Finisher)
        .map_err(|e| (e.code.to_string(), e.role().map(String::from)))
}

fn delegation_error(role: &str) -> Result<(), (String, Option<String>)> {
    Err(("TUF_DELEGATION".into(), Some(role.into())))
}

#[test]
fn a_finisher_layout_passes() {
    let top = load(
        targets(&["advisories.json"], vec![role("acme", &["acme/*"], true)]),
        &[("acme", targets(&["acme/pack/1.0.0.finpkg"], vec![]))],
    );
    assert_eq!(check(&top, &["acme"]), Ok(()));
}

#[test]
fn non_terminating_delegation() {
    let top = targets(&[], vec![role("acme", &["acme/*"], false)]);
    assert_eq!(check(&top, &[]), delegation_error("acme"));
}

#[test]
fn patterns_other_than_publisher_star() {
    for paths in [
        &["*"][..],
        &["acme/pack/*"],
        &["beta/*"],
        &["acme/*", "beta/*"],
        &["acme/**"],
        &[],
    ] {
        let top = targets(&[], vec![role("acme", paths, true)]);
        assert_eq!(check(&top, &[]), delegation_error("acme"), "{paths:?}");
    }
    // Hash prefixes instead of paths.
    let mut r = role("acme", &[], true);
    r.as_object_mut().unwrap().remove("paths");
    r["path_hash_prefixes"] = json!(["ab"]);
    assert_eq!(check(&targets(&[], vec![r]), &[]), delegation_error("acme"));
    // Names that are no publisher's.
    for name in ["targets", "root", "Acme", "a", "acme/x"] {
        let top = targets(&[], vec![role(name, &[&format!("{name}/*")], true)]);
        assert_eq!(check(&top, &[]), delegation_error(name), "{name}");
    }
}

#[test]
fn nested_delegation() {
    let inner = targets(&[], vec![role("beta", &["acme/beta/*"], true)]);
    let top = load(
        targets(&[], vec![role("acme", &["acme/*"], true)]),
        &[("acme", inner)],
    );
    assert_eq!(check(&top, &["acme"]), delegation_error("acme"));
    // An empty delegations object is no delegation.
    let mut empty = targets(&[], vec![]);
    empty.signed.delegations = Some(tough::schema::Delegations {
        keys: Default::default(),
        roles: Vec::new(),
    });
    let top = load(
        targets(&[], vec![role("acme", &["acme/*"], true)]),
        &[("acme", empty)],
    );
    assert_eq!(check(&top, &["acme"]), Ok(()));
}

#[test]
fn more_than_500_delegations() {
    let names: Vec<String> = (0..=500).map(|i| format!("pub{i:03}")).collect();
    let roles = |n: usize| {
        names[..n]
            .iter()
            .map(|p| role(p, &[&format!("{p}/*")], true))
            .collect::<Vec<_>>()
    };
    assert_eq!(check(&targets(&[], roles(500)), &[]), Ok(()));
    let e = delegations(&targets(&[], roles(501)), &snapshot(&[]), Layout::Generic).unwrap_err();
    assert_eq!(e.code, "REGISTRY_INVALID");
    assert_eq!(e.details["reason"], "tooManyDelegations");
}

#[test]
fn listed_but_not_loaded() {
    let top = targets(&[], vec![role("acme", &["acme/*"], true)]);
    // Not in the snapshot and not loaded: a publisher with nothing published yet.
    assert_eq!(check(&top, &[]), Ok(()));
    assert_eq!(check(&top, &["acme"]), delegation_error("acme"));
    // The generic layout keeps this rule alone.
    let any = targets(&[], vec![role("x", &["*"], false)]);
    assert!(delegations(&any, &snapshot(&[]), Layout::Generic).is_ok());
    let e = delegations(&any, &snapshot(&["x"]), Layout::Generic).unwrap_err();
    assert_eq!((e.code, e.role()), ("TUF_DELEGATION", Some("x")));
}

#[test]
fn targets_outside_the_publisher_or_not_plain_paths() {
    for name in [
        "beta/pack/1.0.0.finpkg",
        "acme",
        "acme/./pack.finpkg",
        "acme/../beta/pack.finpkg",
        "acme/a\\b.finpkg",
        "acme/con.finpkg",
        "acme/PROGRA~1.JSO",
    ] {
        let top = load(
            targets(&[], vec![role("acme", &["acme/*"], true)]),
            &[("acme", targets(&[name], vec![]))],
        );
        assert_eq!(check(&top, &["acme"]), delegation_error("acme"), "{name}");
    }
    // Top-level `targets` lists registry-wide files: plain paths too.
    for name in [
        "../advisories.json",
        "a\\b.json",
        "nul.json",
        "x/./y.json",
        "x:y",
    ] {
        let top = targets(&[name], vec![]);
        assert_eq!(check(&top, &[]), delegation_error("targets"), "{name}");
    }
    assert_eq!(check(&targets(&["advisories.json"], vec![]), &[]), Ok(()));
}

#[test]
fn consistent_snapshots_and_spec_1x() {
    assert!(root(&root_with("1.0.0", true)).is_ok());
    assert!(root(&root_with("1.0.31", true)).is_ok());
    for (spec, consistent) in [
        ("1.0.0", false),
        ("2.0.0", true),
        ("0.9", true),
        ("10.0", true),
    ] {
        let e = root(&root_with(spec, consistent)).unwrap_err();
        assert_eq!(
            (e.code, e.details["reason"].as_str()),
            ("REGISTRY_INVALID", Some("layout")),
            "{spec} {consistent}"
        );
    }
}
