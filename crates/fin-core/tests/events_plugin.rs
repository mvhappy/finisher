//! The plugin diagnostics kinds (8A plan Task 11): `plugin.lifecycle` and `registry.refresh`,
//! `PluginRef`, and the `BUNDLED_PLUGINS` and `PLUGIN_ACTIONS` tables. The emitters are
//! `fin-node`'s registry calls (Task 8), tested in `packages/fin-node/test/registry.test.ts`.
//! 8B Task 15: `plugin.stats`, `plugin.exit`, `plugin.command` and `worker.exit` `kv` 2,
//! emitted by `finisherd` through `fin-node`'s `recordPlugin` (`journal/plugin_ref.rs`).

#[path = "events_support/plugin.rs"]
mod plugin_samples;

use fin_core::events::names::{self, BUNDLED_PLUGINS, PLUGIN_ACTIONS, bundled_plugin_name};
use fin_core::events::names_jobs;
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
    for name in ["plugin.stats", "plugin.exit", "plugin.command"] {
        let k = kind(name, 1).unwrap_or_else(|| panic!("{name} v1"));
        assert!(!k.reserved && !k.training_only, "{name}");
        assert_eq!(k.sources, &[Source::Finisherd], "{name}");
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
    assert_eq!(p.plugin, Some(PluginRef::Bundled(Name::OTHER)));
    // A commit of a plan that is gone names no plugin: the field is absent, not free text.
    let mut gone = raw.clone();
    gone.as_object_mut().unwrap().remove("plugin");
    let p = decode_payload(spec, &gone).unwrap();
    let AnyPayload::PluginLifecycle(l) = &p else {
        panic!()
    };
    assert_eq!(l.plugin, None);
    assert!(serde_json::to_value(&p).unwrap().get("plugin").is_none());
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
            plugin: PluginRef::of(id, hash),
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

#[test]
fn worker_exit_kv2_carries_plugin() {
    assert!(kind("worker.exit", 1).is_none());
    let spec = kind("worker.exit", 2).unwrap();
    let schema = (spec.schema)();
    assert_eq!(
        schema["properties"]["plugin"]["$ref"],
        json!("#/$defs/PluginRef"),
        "{schema}"
    );
    let raw = json!({
        "role": "worker", "family": "plugin", "reason": "hung", "exitCode": 1,
        "plugin": { "hashed": hash("@acme/x").unwrap().as_str() },
        "jobsServed": 3, "lifetimeS": 40, "sandboxed": false,
    });
    let AnyPayload::WorkerExit(w) = decode_payload(spec, &raw).unwrap() else {
        panic!()
    };
    assert_eq!(w.plugin, PluginRef::of("@acme/x", hash));
    // `plugin.exit` comes from the same exit.
    let e = PluginExit::of(&w).unwrap();
    assert_eq!(e.reason, PluginExitReason::Timeout);
    assert_eq!((e.exit_code, e.jobs_served, e.uptime_s), (Some(1), 3, 40));
    assert_eq!(e.plugin, w.plugin.clone().unwrap());
    // A process of no plugin has no `plugin.exit`; a raw id is not a reference.
    let mut none = raw.clone();
    none.as_object_mut().unwrap().remove("plugin");
    let AnyPayload::WorkerExit(w) = decode_payload(spec, &none).unwrap() else {
        panic!()
    };
    assert!(PluginExit::of(&w).is_none());
    let mut bare = raw.clone();
    bare["plugin"] = json!("@acme/x");
    assert!(decode_payload(spec, &bare).is_err());
}

#[test]
fn plugin_exit_reasons_cover_every_exit() {
    use PluginExitReason as P;
    let want = [
        P::Ok,
        P::Idle,
        P::Ok,
        P::Killed,
        P::Killed,
        P::Crash,
        P::Timeout,
        P::Timeout,
        P::Killed,
        P::Crash,
        P::Oom,
        P::Protocol,
    ];
    let got: Vec<P> = ExitReason::ALL.iter().map(|r| P::of(*r)).collect();
    assert_eq!(got, want);
    let schema = (kind("plugin.exit", 1).unwrap().schema)();
    assert_eq!(
        schema["$defs"]["PluginExitReason"]["enum"],
        json!([
            "ok", "crash", "timeout", "killed", "oom", "protocol", "sandbox", "idle"
        ])
    );
}

#[test]
fn plugin_command_training_args_are_content() {
    let spec = kind("plugin.command", 1).unwrap();
    let schema = (spec.schema)();
    let args = &schema["properties"]["args"];
    assert_eq!(args["x-fin-class"], json!("content"), "{schema}");
    assert_eq!(args["x-fin-content"], json!("toolArgs"), "{schema}");
    // The command is a keyed hash, never its name.
    assert_eq!(
        schema["properties"]["command"]["$ref"],
        json!("#/$defs/Hashed")
    );
    let p = plugin_samples::samples(
        hash("x").unwrap(),
        Code::parse("PLUGIN_TX_INVALID").unwrap(),
    )
    .into_iter()
    .find(|p| p.kind().name == "plugin.command")
    .unwrap();
    let full = serde_json::to_value(&p).unwrap();
    assert_eq!(full["args"], json!({ "inline": { "gapMs": 40 } }));
    // Without training the arguments go, both typed and as JSON.
    let mut stripped = p.clone();
    stripped.strip_content();
    let v = serde_json::to_value(&stripped).unwrap();
    assert!(v.get("args").is_none(), "{v}");
    let mut red = full.clone();
    redact(spec, &mut red, Purposes::DIAGNOSTICS);
    assert!(red.get("args").is_none(), "{red}");
    assert_eq!(red["outcome"], json!("rebased"));
    assert_eq!(red["ops"], json!({ "item.trim": 2 }));
    // An op kind outside `OP_KINDS` reads as `other`.
    let mut raw = red.clone();
    raw["ops"] = json!({ "drop table": 1 });
    let AnyPayload::PluginCommand(c) = decode_payload(spec, &raw).unwrap() else {
        panic!()
    };
    assert_eq!(c.ops.0.keys().next(), Some(&Name::OTHER));
}

#[test]
fn stats_hold_no_free_text() {
    let mut payloads = plugin_samples::samples(
        hash("x").unwrap(),
        Code::parse("PROJECT_NOT_FOUND").unwrap(),
    );
    payloads.retain(|p| matches!(p.kind().name, "plugin.stats" | "plugin.exit"));
    assert_eq!(payloads.len(), 2);
    let enums = [
        "ok", "crash", "timeout", "killed", "oom", "protocol", "sandbox", "idle", "hashed",
        "bundled",
    ];
    let fields = [
        "plugin",
        "windowS",
        "families",
        "family",
        "calls",
        "errors",
        "reachDenials",
        "p50Micros",
        "p95Micros",
        "bytesInLog2",
        "bytesOutLog2",
        "timeouts",
        "crashes",
        "logDropped",
        "stderrDropped",
        "egress",
        "requests",
        "completed",
        "denied",
        "notHttps",
        "privateAddress",
        "secretMissing",
        "rateLimited",
        "tooLarge",
        "failed",
        "redirectsFollowed",
        "redirectsReturned",
        "reason",
        "exitCode",
        "signal",
        "jobsServed",
        "uptimeS",
    ];
    for p in &payloads {
        let v = serde_json::to_value(p).unwrap();
        let mut all = Vec::new();
        strings(&v, &mut all);
        for s in all {
            let ok = enums.contains(&s)
                || fields.contains(&s)
                || names::PLUGIN_METHOD_FAMILIES.get(s).is_some()
                || names_jobs::SIGNALS.get(s).is_some()
                || Hashed::parse(s).is_some()
                || Code::parse(s).is_some();
            assert!(ok, "{}: free string {s:?}", p.kind().name);
        }
    }
    // A family outside the table reads as `other`; a free-text error key does not decode.
    let spec = kind("plugin.stats", 1).unwrap();
    let mut raw = serde_json::to_value(&payloads[0]).unwrap();
    raw["families"][0]["family"] = json!("/home/someone/x");
    let AnyPayload::PluginStats(s) = decode_payload(spec, &raw).unwrap() else {
        panic!()
    };
    assert_eq!(s.families[0].family, Name::OTHER);
    raw["families"][0]["errors"] = json!({ "a free message": 1 });
    assert!(decode_payload(spec, &raw).is_err());
    raw["families"][0]["errors"] = json!({});
    raw["origin"] = json!("https://example.com");
    let p = decode_payload(spec, &raw).unwrap();
    assert!(!serde_json::to_string(&p).unwrap().contains("example"));
}

// `fin-node`'s `recordPlugin` builder (`plugin_payloads`).

fn id_ref(s: &str) -> Option<IdRef> {
    Some(IdRef::classify(s).unwrap_or_else(|_| IdRef::from_hashed(hash(s).unwrap())))
}

const ON: PluginKeys = PluginKeys {
    hash: &hash,
    id_ref: &id_ref,
    training: true,
};

fn exit_payload() -> Value {
    json!({ "role": "worker", "family": "plugin", "reason": "crashed", "exitCode": 3,
            "jobsServed": 2, "lifetimeS": 9, "sandboxed": false })
}

#[test]
fn plugin_ref_is_name_or_keyed_hash() {
    // A third-party plugin is the keyed hash; the raw id is nowhere in the entry.
    let ps = plugin_payloads("worker.exit", 2, "@acme/fancy", exit_payload(), &ON).unwrap();
    assert_eq!(ps.len(), 2);
    for p in &ps {
        let text = serde_json::to_string(p).unwrap();
        assert!(!text.contains("acme") && !text.contains("fancy"), "{text}");
        assert!(
            text.contains(hash("@acme/fancy").unwrap().as_str()),
            "{text}"
        );
    }
    let AnyPayload::PluginExit(e) = &ps[1] else {
        panic!()
    };
    assert_eq!(e.reason, PluginExitReason::Crash);
    // A bundled plugin is its table name.
    let ps = plugin_payloads(
        "plugin.exit",
        1,
        "@finisher/captions-basic",
        json!({
        "reason": "idle", "jobsServed": 1, "uptimeS": 300 }),
        &ON,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&ps[0]).unwrap()["plugin"],
        json!({ "bundled": "finisher/captions-basic" })
    );
    // A `plugin` the caller sent is replaced, never trusted.
    let mut forged = exit_payload();
    forged["plugin"] = json!({ "bundled": "finisher/captions-basic" });
    let ps = plugin_payloads("worker.exit", 2, "@acme/fancy", forged, &ON).unwrap();
    assert_eq!(
        serde_json::to_value(&ps[0]).unwrap()["plugin"],
        json!({ "hashed": hash("@acme/fancy").unwrap().as_str() })
    );
    // Without the key (off) nothing is built.
    let off = PluginKeys {
        hash: &|_| None,
        id_ref: &|_| None,
        training: false,
    };
    assert!(
        plugin_payloads("worker.exit", 2, "@acme/fancy", exit_payload(), &off)
            .unwrap()
            .is_empty()
    );
    // Only the plugin runtime's kinds.
    for (k, v) in [("worker.exit", 1), ("plugin.lifecycle", 1), ("job.done", 2)] {
        assert_eq!(
            plugin_payloads(k, v, "@acme/fancy", exit_payload(), &ON)
                .unwrap_err()
                .code,
            "EVENT_INVALID"
        );
    }
}

#[test]
fn command_is_keyed_and_args_are_training_content() {
    let raw = json!({ "command": "tighten-gaps", "outcome": "accepted",
        "ops": { "item.trim": 2 }, "txId": "01JC0000000000000000000002", "micros": 5,
        "args": { "gapMs": 40 } });
    let ps = plugin_payloads("plugin.command", 1, "@acme/fancy", raw.clone(), &ON).unwrap();
    let AnyPayload::PluginCommand(c) = &ps[0] else {
        panic!()
    };
    assert_eq!(c.command, hash("@acme/fancy/tighten-gaps").unwrap());
    assert_eq!(
        c.tx_id.as_ref().map(|r| r.as_str()),
        Some("01JC0000000000000000000002")
    );
    assert_eq!(c.args, Some(Content(Body::Inline(json!({ "gapMs": 40 })))));
    assert!(!serde_json::to_string(c).unwrap().contains("tighten"));
    // Training off: no arguments built.
    let no_training = PluginKeys {
        training: false,
        ..ON
    };
    let ps = plugin_payloads(
        "plugin.command",
        1,
        "@acme/fancy",
        raw.clone(),
        &no_training,
    )
    .unwrap();
    let AnyPayload::PluginCommand(c) = &ps[0] else {
        panic!()
    };
    assert_eq!((c.args.as_ref(), c.args_omitted), (None, None));
    // Too large: omitted, and said so.
    let mut big = raw.clone();
    big["args"] = json!({ "text": "x".repeat(MAX_COMMAND_ARGS_BYTES) });
    let ps = plugin_payloads("plugin.command", 1, "@acme/fancy", big, &ON).unwrap();
    let AnyPayload::PluginCommand(c) = &ps[0] else {
        panic!()
    };
    assert_eq!(
        (c.args.is_none(), c.args_omitted),
        (true, Some(ArgsOmitted::TooLarge))
    );
    // An unknown op kind reads as `other`; a missing command is refused.
    let mut odd = raw.clone();
    odd["ops"] = json!({ "rm -rf": 1 });
    let ps = plugin_payloads("plugin.command", 1, "@acme/fancy", odd, &ON).unwrap();
    let AnyPayload::PluginCommand(c) = &ps[0] else {
        panic!()
    };
    assert_eq!(c.ops.0.keys().next(), Some(&Name::OTHER));
    let mut nameless = raw;
    nameless.as_object_mut().unwrap().remove("command");
    assert!(plugin_payloads("plugin.command", 1, "@acme/fancy", nameless, &ON).is_err());
}
