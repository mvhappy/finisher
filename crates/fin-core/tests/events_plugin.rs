//! The plugin diagnostics kinds (8A plan Task 11): `plugin.lifecycle` and `registry.refresh`,
//! `PluginRef`, and the `BUNDLED_PLUGINS` and `PLUGIN_ACTIONS` tables. The emitters are
//! `fin-node`'s registry calls (Task 8), tested in `packages/fin-node/test/registry.test.ts`.

#[path = "events_support/plugin.rs"]
mod plugin_samples;

use fin_core::events::names::{BUNDLED_PLUGINS, PLUGIN_ACTIONS, bundled_plugin_name};
use fin_core::events::payloads::*;
use fin_core::events::*;
use serde_json::{Value, json};

fn key(data: &[u8]) -> [u8; 16] {
    let full = blake3::keyed_hash(&[9; 32], data);
    full.as_bytes()[..16].try_into().unwrap()
}

fn hash(s: &str) -> Option<Hashed> {
    Some(Hashed::from_bytes(&key(s.as_bytes())))
}

fn strings<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::String(s) => out.push(s),
        Value::Array(a) => a.iter().for_each(|x| strings(x, out)),
        Value::Object(o) => o.iter().for_each(|(k, x)| {
            out.push(k);
            strings(x, out)
        }),
        _ => {}
    }
}

#[test]
fn kinds_registered_v1() {
    for name in ["plugin.lifecycle", "registry.refresh"] {
        let k = kind(name, 1).unwrap_or_else(|| panic!("{name} v1"));
        assert!(!k.reserved && !k.training_only, "{name}");
        assert_eq!(k.sources, &[Source::FinNode], "{name}");
        assert!(kind(name, 0).is_none(), "{name}");
    }
    for p in plugin_samples::samples(hash("x").unwrap(), Code::parse("TUF_ROLLBACK").unwrap()) {
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(decode_payload(p.kind(), &v).unwrap(), p);
        let ctx = ScrubCtx::default();
        assert_eq!(validate_payload(p.kind(), &v, &key, &ctx).unwrap(), p);
    }
    assert_eq!(
        PLUGIN_ACTIONS.entries(),
        [
            "plan",
            "install",
            "update",
            "downgrade",
            "remove",
            "enable",
            "disable",
            "revoke"
        ]
    );
}

#[test]
fn plugin_ref_never_free_text() {
    // A non-bundled id is hashed; the hash needs the journal (no hash, no reference).
    let r = PluginRef::of("@acme/fancy-captions", hash).unwrap();
    assert!(matches!(r, PluginRef::Hashed(_)));
    let text = serde_json::to_string(&r).unwrap();
    assert!(!text.contains("acme") && !text.contains("fancy"), "{text}");
    assert_eq!(PluginRef::of("@acme/fancy-captions", |_| None), None);
    // Free text sent as a bundled name reads as `other`; as a hash it does not decode.
    for raw in [
        "@acme/fancy-captions",
        "acme/fancy-captions",
        "finisher/not-shipped",
    ] {
        let r: PluginRef = serde_json::from_value(json!({ "bundled": raw })).unwrap();
        assert_eq!(r, PluginRef::Bundled(Name::OTHER), "{raw}");
    }
    for raw in ["@acme/fancy-captions", "k1:short", ""] {
        assert!(serde_json::from_value::<PluginRef>(json!({ "hashed": raw })).is_err());
    }
    assert!(serde_json::from_value::<PluginRef>(json!({ "id": "@acme/x" })).is_err());
    // Received payloads read through the same types.
    let spec = kind("plugin.lifecycle", 1).unwrap();
    let raw = json!({
        "action": "exfiltrate /home/someone", "outcome": "ok", "plugin": { "bundled": "@acme/x" },
        "source": "registry", "kind": "code", "permissionsAdded": 0, "risk": "low",
        "bytesLog2": 3, "micros": 1,
    });
    let AnyPayload::PluginLifecycle(p) =
        validate_payload(spec, &raw, &key, &ScrubCtx::default()).unwrap()
    else {
        panic!()
    };
    assert_eq!(p.action, Name::OTHER);
    assert_eq!(p.plugin, PluginRef::Bundled(Name::OTHER));
    let mut bad = raw.clone();
    bad["registry"] = json!("/srv/registries/team");
    let p = validate_payload(spec, &bad, &key, &ScrubCtx::default()).unwrap();
    assert!(!serde_json::to_string(&p).unwrap().contains("registries"));
}

#[test]
fn lifecycle_events_hold_no_free_text() {
    // An install from a local registry, as `fin-node` builds it.
    let id = "@acme/fancy-captions";
    let (publisher, registry_id, registry_path) = ("Acme Studio", "team", "/srv/registries/team");
    let payloads: Vec<AnyPayload> = vec![
        PluginLifecycle {
            action: PLUGIN_ACTIONS.get("install").unwrap(),
            outcome: PluginOutcome::Ok,
            code: None,
            plugin: PluginRef::of(id, hash).unwrap(),
            source: PluginSourceName::Registry,
            kind: PluginKindName::Content,
            permissions_added: 0,
            risk: RiskName::Low,
            bytes_log2: log2_bucket(123_456),
            micros: 900,
        }
        .into(),
        RegistryRefresh {
            outcome: PluginOutcome::Refused,
            code: Code::parse("TUF_ROLLBACK"),
            roles_updated: 0,
            targets_log2: log2_bucket(2),
            stale_days: None,
            by_human: false,
            micros: 10,
        }
        .into(),
    ];
    let enums = [
        "ok", "refused", "failed", "bundled", "registry", "dev", "content", "code", "unknown",
        "low", "medium", "high",
    ];
    let fields = [
        "action",
        "outcome",
        "code",
        "plugin",
        "hashed",
        "source",
        "kind",
        "permissionsAdded",
        "risk",
        "bytesLog2",
        "micros",
        "rolesUpdated",
        "targetsLog2",
        "staleDays",
        "byHuman",
    ];
    for p in &payloads {
        let v = serde_json::to_value(p).unwrap();
        let text = v.to_string();
        for leak in [
            id,
            "acme",
            "fancy",
            publisher,
            registry_id,
            registry_path,
            "1.0.0",
        ] {
            assert!(!text.contains(leak), "{}: {leak} in {text}", p.kind().name);
        }
        let mut all = Vec::new();
        strings(&v, &mut all);
        for s in all {
            let ok = enums.contains(&s)
                || fields.contains(&s)
                || PLUGIN_ACTIONS.get(s).is_some()
                || Hashed::parse(s).is_some()
                || Code::parse(s).is_some();
            assert!(ok, "{}: free string {s:?}", p.kind().name);
        }
    }
}

#[test]
fn bundled_ref_is_a_name() {
    let r = PluginRef::of("@finisher/captions-basic", |_| {
        panic!("bundled ids are not hashed")
    });
    assert_eq!(r, Some(plugin_samples::bundled()));
    assert_eq!(
        serde_json::to_value(&r).unwrap(),
        json!({ "bundled": "finisher/captions-basic" })
    );
    for e in BUNDLED_PLUGINS.entries() {
        let id = format!("@{e}");
        assert!(bundled_plugin_name(&id).is_some(), "{id}");
        let (publisher, name) = e.split_once('/').unwrap();
        assert!(!publisher.is_empty() && !name.contains('/'), "{e}");
    }
    // Only `@`-prefixed ids are looked up.
    assert_eq!(bundled_plugin_name("finisher/captions-basic"), None);
    assert_eq!(log2_bucket(0), 0);
    assert_eq!(log2_bucket(1), 1);
    assert_eq!(log2_bucket(3), 2);
    assert_eq!(log2_bucket(1024), 11);
    assert_eq!(log2_bucket(u64::MAX), 64);
}
