//! The event model (plan D, Task D1): the envelope, the kind registry, the field classes, the
//! name tables, redaction and scrubbing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use fin_core::events::class::{NAME_PATTERN, is_valid_name};
use fin_core::events::names::{self, MEDIA_EXTS, OP_KINDS, TABLES};
use fin_core::events::payloads::*;
use fin_core::events::scrub::MAX_SCRUBBED;
use fin_core::events::*;
use schemars::JsonSchema;
use serde_json::{Map, Value, json};

mod events_samples;

use events_samples::*;

fn golden(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/events_golden")
        .join(name)
}

/// Compares `actual` (pretty JSON) with a golden file; `FIN_EVENTS_BLESS=1` rewrites it.
fn check_golden(name: &str, actual: &str) {
    let path = golden(name);
    if std::env::var("FIN_EVENTS_BLESS").is_ok_and(|v| v == "1") {
        std::fs::write(&path, format!("{actual}\n")).unwrap();
    }
    let want = std::fs::read_to_string(&path).unwrap();
    assert_eq!(actual, want.trim_end(), "{name} drifted");
}

#[test]
fn envelope_json_matches_golden() {
    let payload = LogCommit {
        ops: counts([(OP_KINDS.get("item.trim").unwrap(), 1)]),
        base_rev: Some(41),
        intent: None,
        ..log_commit()
    };
    let with_ctx = Envelope {
        v: ENVELOPE_VERSION,
        id: ulid("01JC0000000000000000000000"),
        ts: 1_794_571_200_123,
        session: ulid("01JC0000000000000000000001"),
        source: Source::FinNode,
        kind: KindName::parse("log.commit").unwrap(),
        kv: 1,
        consent: ConsentStamp {
            rev: 3,
            purposes: Purposes::DIAGNOSTICS,
            basis: Basis::Default,
        },
        ctx: Some(Ctx {
            project: ProjectId::parse("01jb0000000000000000000000"),
            branch: BranchId::parse("br1"),
            tx_id: Some(id(TX)),
            author: Some(Hashed::parse("k1:3f000000000000000000000000000000").unwrap()),
            actor: Some(Actor {
                kind: ActorKind::Agent,
                session: Some(id(SESSION)),
                call: Some(IdRef::from_hashed(
                    Hashed::parse("k1:9a000000000000000000000000000000").unwrap(),
                )),
            }),
        }),
        payload: serde_json::to_value(AnyPayload::from(payload)).unwrap(),
    };
    let text = serde_json::to_string_pretty(&with_ctx).unwrap();
    check_golden("envelope.json", &text);
    assert_eq!(serde_json::from_str::<Envelope>(&text).unwrap(), with_ctx);

    let no_ctx = Envelope {
        id: ulid("01JC0000000000000000000004"),
        ts: 1_794_571_200_999,
        source: Source::Finisherd,
        kind: KindName::of(kind("session.end", 1).unwrap()),
        consent: ConsentStamp {
            rev: 4,
            purposes: Purposes::ALL,
            basis: Basis::User,
        },
        ctx: None,
        payload: serde_json::to_value(AnyPayload::from(SessionEnd { uptime_s: 3600 })).unwrap(),
        ..with_ctx
    };
    let text = serde_json::to_string_pretty(&no_ctx).unwrap();
    check_golden("envelope_no_ctx.json", &text);
    assert!(!text.contains("\"ctx\""));
    assert_eq!(serde_json::from_str::<Envelope>(&text).unwrap(), no_ctx);
}

#[test]
fn purposes_serialise_sorted_and_basis_orders_weakest_first() {
    assert_eq!(
        serde_json::to_value(Purposes::ALL).unwrap(),
        json!(["diagnostics", "training"])
    );
    assert_eq!(serde_json::to_value(Purposes::NONE).unwrap(), json!([]));
    let read: Purposes = serde_json::from_value(json!(["training", "diagnostics"])).unwrap();
    assert_eq!(read, Purposes::ALL);
    assert!(serde_json::from_value::<Purposes>(json!(["training", "training"])).is_err());
    assert_eq!(Purposes::from_bits(0xff), Purposes::ALL);
    assert_eq!(
        Purposes::ALL.intersect(Purposes::DIAGNOSTICS),
        Purposes::DIAGNOSTICS
    );
    assert!(!Purposes::DIAGNOSTICS.has_training());
    assert_eq!(Basis::User.weaker(Basis::Default), Basis::Default);
    assert_eq!(Basis::Env.weaker(Basis::User), Basis::Env);
}

#[test]
fn every_kind_round_trips() {
    for p in samples() {
        let spec = p.kind();
        let v = serde_json::to_value(&p).unwrap();
        let back = decode_payload(spec, &v).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
        assert_eq!(back, p, "{}", spec.name);
        assert_eq!(serde_json::to_value(&back).unwrap(), v, "{}", spec.name);
    }
}

#[test]
fn any_payload_covers_every_kind() {
    let sampled: Vec<&str> = samples().iter().map(|p| p.kind().name).collect();
    let registry: Vec<&str> = kinds().iter().map(|k| k.name).collect();
    assert_eq!(sampled, registry, "one sample per kind, in registry order");
    let unique: BTreeSet<&str> = registry.iter().copied().collect();
    assert_eq!(unique.len(), registry.len(), "kind names repeat");
    for k in kinds() {
        let (domain, verb) = k.name.split_once('.').expect("domain.verb");
        for seg in [domain, verb] {
            assert!(
                seg.starts_with(|c: char| c.is_ascii_lowercase()),
                "{}",
                k.name
            );
            assert!(seg.chars().all(|c| c.is_ascii_alphanumeric()), "{}", k.name);
        }
        assert_eq!(k.reserved, k.version == 0, "{}", k.name);
        assert!(!k.sources.is_empty(), "{}", k.name);
        assert!(std::ptr::eq(kind(k.name, k.version).unwrap(), k));
    }
    for name in ["agent.turn", "agent.toolCall", "agent.toolResult"] {
        assert!(kind(name, 1).unwrap().training_only, "{name}");
    }
    for name in [
        "ui.action",
        "review.verdict",
        "preview.stats",
        "render.sample",
    ] {
        assert!(kind(name, 0).unwrap().reserved, "{name}");
    }
    let training_only: Vec<&str> = kinds()
        .iter()
        .filter(|k| k.training_only)
        .map(|k| k.name)
        .collect();
    assert_eq!(
        training_only,
        [
            "agent.turn",
            "agent.toolCall",
            "agent.toolResult",
            "render.sample"
        ]
    );
}

#[test]
fn approx_bytes_bounds_serialised_size() {
    for p in samples() {
        let n = serde_json::to_string(&p).unwrap().len();
        let a = p.approx_bytes();
        if n >= 64 {
            assert!(
                a * 2 >= n && a <= n * 2,
                "{}: approx {a}, serialised {n}",
                p.kind().name
            );
        } else {
            assert!(a <= 128, "{}: approx {a}, serialised {n}", p.kind().name);
        }
    }
}

/// An independent walk of a kind's schema (not `redact`'s): every string leaf outside content
/// is an enum or classed, every meta string has a pattern or an enum, every map key is
/// classed, nothing is an any-schema outside content, and a `BlobRef` appears only inside
/// content. Inside content it goes on: the plain strings there are listed (`raw_in_content`),
/// so a test can say where content may hold free text. An unresolved `$ref` panics.
#[derive(Default)]
struct Check {
    issues: Vec<String>,
    content: Vec<(String, Option<String>)>,
    scrubbed_outside: Vec<String>,
    blobs_inside: Vec<String>,
    raw_in_content: Vec<String>,
}

fn is_string_schema(obj: &Map<String, Value>) -> bool {
    let ty = obj.get("type");
    ty == Some(&json!("string"))
        || ty
            .and_then(Value::as_array)
            .is_some_and(|a| a.contains(&json!("string")))
}

impl Check {
    fn of(schema: &Value) -> Check {
        let mut c = Check::default();
        let defs = schema
            .get("$defs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        c.walk(&defs, schema, "", None, false, &mut Vec::new());
        c
    }

    fn walk(
        &mut self,
        defs: &Map<String, Value>,
        node: &Value,
        path: &str,
        field: Option<String>,
        in_content: bool,
        stack: &mut Vec<String>,
    ) {
        let obj = match node {
            Value::Bool(true) if !in_content => {
                return self
                    .issues
                    .push(format!("{path}: any-schema outside content"));
            }
            Value::Object(o) => o,
            _ => return,
        };
        if obj.is_empty() && !in_content {
            return self
                .issues
                .push(format!("{path}: empty schema outside content"));
        }
        let field = obj
            .get("x-fin-content")
            .and_then(Value::as_str)
            .map(String::from)
            .or(field);
        let mut in_content = in_content;
        match obj.get("x-fin-class").and_then(Value::as_str) {
            Some("content") => {
                if !in_content {
                    self.content.push((path.to_string(), field.clone()));
                }
                in_content = true;
            }
            Some("meta") => {
                let checked = ["pattern", "enum", "const"]
                    .iter()
                    .any(|k| obj.contains_key(*k));
                if is_string_schema(obj) && !checked {
                    self.issues
                        .push(format!("{path}: meta string without a pattern"));
                }
                return;
            }
            Some("scrubbed") => {
                if !in_content {
                    self.scrubbed_outside.push(path.to_string());
                }
                return;
            }
            Some(other) => return self.issues.push(format!("{path}: unknown class {other}")),
            None => {}
        }
        if obj.get("x-fin-blob") == Some(&Value::Bool(true)) {
            if in_content {
                self.blobs_inside.push(path.to_string());
            } else {
                self.issues.push(format!("{path}: BlobRef outside content"));
            }
            return;
        }
        if let Some(r) = obj.get("$ref").and_then(Value::as_str) {
            let name = r.rsplit('/').next().unwrap().to_string();
            if !stack.contains(&name) {
                stack.push(name.clone());
                let def = defs
                    .get(&name)
                    .cloned()
                    .unwrap_or_else(|| panic!("{path}: unresolved $ref {r}"));
                self.walk(defs, &def, path, field.clone(), in_content, stack);
                stack.pop();
            }
        }
        let plain =
            is_string_schema(obj) && !obj.contains_key("enum") && !obj.contains_key("const");
        if plain && in_content {
            self.raw_in_content.push(path.to_string());
        }
        if !in_content {
            if plain {
                self.issues.push(format!("{path}: unclassified string"));
            }
            let open_map = obj
                .get("additionalProperties")
                .is_some_and(|a| a != &json!(false));
            if open_map {
                match obj.get("propertyNames") {
                    Some(keys) => {
                        self.walk(defs, keys, &format!("{path}{{key}}"), None, false, stack)
                    }
                    None => self.issues.push(format!("{path}: map keys unclassified")),
                }
            }
            if obj.contains_key("patternProperties") {
                self.issues.push(format!("{path}: pattern-keyed map"));
            }
        }
        if let Some(props) = obj.get("properties").and_then(Value::as_object) {
            for (k, v) in props {
                let p = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                self.walk(defs, v, &p, None, in_content, stack);
            }
        }
        for key in ["items", "additionalProperties"] {
            if let Some(v) = obj.get(key).filter(|v| v.is_object() || v == &&json!(true)) {
                self.walk(
                    defs,
                    v,
                    &format!("{path}[]"),
                    field.clone(),
                    in_content,
                    stack,
                );
            }
        }
        for key in ["prefixItems", "anyOf", "oneOf", "allOf"] {
            for v in obj.get(key).and_then(Value::as_array).into_iter().flatten() {
                self.walk(defs, v, path, field.clone(), in_content, stack);
            }
        }
    }
}

fn check(name: &str, version: u16) -> Check {
    Check::of(&(kind(name, version).unwrap().schema)())
}

#[test]
fn payload_strings_are_classified() {
    for k in kinds() {
        let c = Check::of(&(k.schema)());
        assert!(c.issues.is_empty(), "{}: {:?}", k.name, c.issues);
        assert!(
            c.scrubbed_outside.is_empty(),
            "{}: {:?}",
            k.name,
            c.scrubbed_outside
        );
    }
    // The walk itself catches a plain string, a free-keyed map and an any-schema.
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Bad {
        text: String,
        by_name: BTreeMap<String, u64>,
        raw: Value,
        fine: Option<Code>,
    }
    let bad = Check::of(&serde_json::to_value(schemars::schema_for!(Bad)).unwrap());
    let mut flagged: Vec<&str> = bad
        .issues
        .iter()
        .map(|i| i.split(':').next().unwrap())
        .collect();
    flagged.sort();
    assert_eq!(flagged, ["by_name", "raw", "text"], "{:?}", bad.issues);
}

#[test]
fn content_fields_name_their_kind() {
    let mut seen = BTreeMap::new();
    for k in kinds() {
        for (path, field) in Check::of(&(k.schema)()).content {
            let parsed = field.as_deref().and_then(ContentKind::parse);
            assert!(
                parsed.is_some(),
                "{}.{path}: names no content kind ({field:?})",
                k.name
            );
            seen.insert(format!("{}.{path}", k.name), parsed.unwrap());
        }
    }
    let want = BTreeMap::from([
        ("agent.approval.subject".to_string(), ContentKind::UserText),
        ("agent.toolCall.args".into(), ContentKind::ToolArgs),
        ("agent.toolResult.output".into(), ContentKind::ToolResult),
        ("agent.turn.bodyBlake3".into(), ContentKind::ModelInput),
        ("agent.turn.messagesDelta".into(), ContentKind::ModelInput),
        ("agent.turn.output".into(), ContentKind::ModelOutput),
        ("agent.turn.system".into(), ContentKind::ModelInput),
        ("agent.turn.tools".into(), ContentKind::ModelInput),
        ("error.panic.message".into(), ContentKind::ErrorMessage),
        ("log.commit.intent".into(), ContentKind::TxIntent),
    ]);
    assert_eq!(seen, want);
    // `redact`'s own walk agrees.
    for k in kinds() {
        let mine: BTreeSet<String> = content_paths(k).iter().map(|p| p.to_string()).collect();
        let theirs: BTreeSet<String> = Check::of(&(k.schema)())
            .content
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(mine, theirs, "{}", k.name);
        assert!(
            content_paths(k).iter().all(|p| p.kind.is_some()),
            "{}",
            k.name
        );
    }
}

#[test]
fn blob_ref_only_inside_content() {
    for k in kinds() {
        let c = Check::of(&(k.schema)());
        assert!(
            !c.issues.iter().any(|i| i.contains("BlobRef")),
            "{}: {:?}",
            k.name,
            c.issues
        );
    }
    assert_eq!(
        check("agent.turn", 1).blobs_inside,
        [
            "messagesDelta.blob",
            "output.blob",
            "system.blob",
            "tools.blob"
        ]
    );
    #[derive(JsonSchema)]
    #[allow(dead_code)]
    struct Loose {
        frame: BlobRef,
    }
    let loose = Check::of(&serde_json::to_value(schemars::schema_for!(Loose)).unwrap());
    assert_eq!(loose.issues, ["frame: BlobRef outside content"]);
}

#[test]
fn blob_refs_follow_content() {
    let turn = samples()
        .into_iter()
        .find(|p| p.kind().name == "agent.turn")
        .unwrap();
    assert_eq!(
        turn.blobs().cloned().collect::<Vec<_>>(),
        [blob(), blob(), blob()]
    );
    let spec = turn.kind();
    let mut v = serde_json::to_value(&turn).unwrap();
    assert_eq!(blob_refs(spec, &v), [blob(), blob(), blob()]);
    redact(spec, &mut v, Purposes::DIAGNOSTICS);
    assert!(blob_refs(spec, &v).is_empty());
    assert_eq!(decode_payload(spec, &v).unwrap().blobs().count(), 0);
    let others: usize = samples()
        .iter()
        .filter(|p| p.kind().name != "agent.turn")
        .map(|p| p.blobs().count())
        .sum();
    assert_eq!(others, 0);
}

/// The Rust sources under `dir`, recursively, with their paths.
fn sources(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap() {
            let p = entry.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|e| e == "rs") {
                out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    out
}

fn events_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/events")
}

#[test]
fn name_has_no_free_constructor() {
    let squash = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    for (path, text) in sources(&events_dir()) {
        let flat = squash(&text);
        for banned in [
            "for Name {",
            "From<String> for Name",
            "From<&str> for Name",
            "From<&'static str> for Name",
            "FromStr for Name",
            "Default for Name",
            "Deserialize<'de> for Name",
        ] {
            let allowed = [
                "Debug for Name {",
                "Display for Name {",
                "Serialize for Name {",
                "JsonSchema for Name {",
            ];
            if banned == "for Name {" {
                for (i, _) in flat.match_indices(banned) {
                    let before = &flat[..i + banned.len()];
                    assert!(
                        allowed.iter().any(|a| before.ends_with(a))
                            || before.ends_with("impl Name {"),
                        "{}: an unexpected impl on Name: …{}",
                        path.display(),
                        &before[before.len().saturating_sub(60)..]
                    );
                }
            } else {
                assert!(!flat.contains(banned), "{}: {banned}", path.display());
            }
        }
        if let Some(i) = flat.find("pub struct Name(") {
            let derive = &flat[flat[..i].rfind("#[derive(").unwrap()..i];
            for bad in ["Default", "Deserialize"] {
                assert!(!derive.contains(bad), "Name derives {bad}");
            }
            assert!(flat[i..].starts_with("pub struct Name(&'static str);"));
        }
    }
    // The compile-fail doctests on `Name` stay, and its constructors are crate-private.
    let class = std::fs::read_to_string(events_dir().join("class.rs")).unwrap();
    assert!(class.matches("```compile_fail").count() >= 5);
    assert!(class.contains("pub(crate) const fn from_static(s: &'static str) -> Name"));
    assert!(!class.contains("debug_assert!(is_valid_name"));
    let names_rs = std::fs::read_to_string(events_dir().join("names.rs")).unwrap();
    assert!(names_rs.contains("pub(crate) const fn new("));

    // Every `Name::from_static` call in the workspace takes a literal that matches the pattern
    // (`NameTable::get`'s, over a static table, is the one exception).
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for (path, text) in sources(&crates) {
        if path.ends_with("fin-core/tests/events.rs") {
            continue;
        }
        // Doc comments (the compile-fail doctests) are not code.
        let code: String = text
            .lines()
            .filter(|l| !l.trim_start().starts_with("///"))
            .map(|l| format!("{l}\n"))
            .collect();
        for (i, _) in code.match_indices("Name::from_static(") {
            let rest = &code[i + "Name::from_static(".len()..];
            if path.ends_with("events/names.rs") && rest.starts_with("e)") {
                continue;
            }
            let lit = rest
                .strip_prefix('"')
                .and_then(|r| r.split_once('"'))
                .map(|(l, _)| l)
                .unwrap_or_else(|| {
                    panic!("{}: Name::from_static without a literal", path.display())
                });
            assert!(
                is_valid_name(lit),
                "{}: {lit:?} does not match {NAME_PATTERN}",
                path.display()
            );
        }
    }
    assert_eq!(Name::OTHER.as_str(), "other");
}

#[test]
fn name_tables_match_pattern() {
    for t in TABLES {
        let mut seen = BTreeSet::new();
        for e in t.entries() {
            assert!(is_valid_name(e), "{}: {e:?}", t.table);
            assert!(seen.insert(*e), "{}: {e:?} repeats", t.table);
            assert_eq!(t.get(e).unwrap().as_str(), *e);
        }
    }
    assert_eq!(OP_KINDS.entries(), fin_core::Op::NAMES);
    assert!(OP_KINDS.get("item.trim").is_some());
    for t in [
        &names::CLI_COMMANDS,
        &names::CLI_FLAGS,
        &names::RPC_METHODS,
        &names::DETAIL_KEYS,
    ] {
        assert!(t.get("other").is_some(), "{}", t.table);
    }
    for stream in ["ops.subscribe", "project.events"] {
        assert!(names::RPC_METHODS.get(stream).is_some());
    }
}

#[test]
fn unknown_table_value_reads_as_other() {
    let spec = kind("cli.command", 1).unwrap();
    let raw = json!({
        "name": "rm -rf /home/someone", "flags": ["dry-run", "--secret value"], "micros": 1,
        "exit": 0, "online": true, "dryRun": false,
    });
    let AnyPayload::CliCommand(c) =
        validate_payload(spec, &raw, &key, &ScrubCtx::default()).unwrap()
    else {
        panic!()
    };
    assert_eq!(c.name, Name::OTHER);
    assert_eq!(
        c.flags.iter().map(Name::as_str).collect::<Vec<_>>(),
        ["dry-run", "other"]
    );

    let spec = kind("log.commit", 1).unwrap();
    let raw = json!({
        "record": "edit", "outcome": "accepted", "micros": 3,
        "ops": { "item.trim": 1, "made.up": 2, "also.made.up": 3 },
    });
    let AnyPayload::LogCommit(c) = decode_payload(spec, &raw).unwrap() else {
        panic!()
    };
    let ops: Vec<(&str, u64)> = c.ops.0.iter().map(|(k, n)| (k.as_str(), *n)).collect();
    assert_eq!(ops, [("item.trim", 1), ("other", 5)]);

    let path = |ext: &str| PathRef::new(PathRoot::Library, Some(ext), h('1'));
    assert_eq!(path("MP4").ext.map(|e| e.as_str()), Some("mp4"));
    assert_eq!(path("exe").ext, None);
    let read: PathRef =
        serde_json::from_value(json!({ "root": "project", "ext": "secret", "hash": h('1') }))
            .unwrap();
    assert_eq!(read.ext, None);
    assert_eq!(MEDIA_EXTS.get("other"), None);
}

#[test]
fn id_ref_keeps_only_ulids_raw() {
    assert_eq!(IdRef::classify(TX).unwrap().as_str(), TX);
    for raw in [
        "idem-key-7f3a",                      // an idempotency key
        "session-of-someone-on-their-laptop", // a session name with a user name in it
        "01jc0000000000000000000002",         // a lower-case ULID is not canonical
        "81JC0000000000000000000002",         // overflows 128 bits
        "01JC000000000000000000000I",         // not Crockford base 32
        "01JC000000000000000000002",          // 25 characters
        "",
    ] {
        assert_eq!(IdRef::classify(raw), Err(NeedsHash), "{raw:?}");
        assert!(
            serde_json::from_value::<IdRef>(json!(raw)).is_err(),
            "{raw:?}"
        );
    }
    let hashed = IdRef::from_hashed(h('9'));
    assert!(hashed.is_hashed());
    assert_eq!(
        serde_json::from_value::<IdRef>(json!(hashed.as_str())).unwrap(),
        hashed
    );
    assert!(!id(TX).is_hashed());
    // A payload that carries a raw id does not decode.
    let spec = kind("agent.toolResult", 1).unwrap();
    let raw = json!({
        "callId": "01JC0000000000000000000006", "ok": true, "micros": 1,
        "txIds": ["idem-key-7f3a"], "attribution": "exact",
    });
    assert_eq!(
        decode_payload(spec, &raw).unwrap_err().code,
        "EVENT_INVALID"
    );
}

#[test]
fn src_loc_accepts_only_repo_paths() {
    for ok in [
        "crates/fin-node/src/journal.rs:42",
        "apps/finisherd/src/main.ts:7",
        "crates/a.rs:1234567",
    ] {
        assert!(SrcLoc::parse(ok).is_some(), "{ok}");
    }
    for bad in [
        "/home/someone/.cargo/registry/src/index.crates.io-0/serde-1.0.0/src/de.rs:10",
        "node_modules/effect/dist/esm/Effect.js:1",
        "apps/finisherd/node_modules/effect/dist/esm/Effect.js:1",
        "/abs/crates/fin-core/src/lib.rs:1",
        "crates/../etc/passwd:1",
        "crates/./x.rs:1",
        "crates//x.rs:1",
        "crates/x.rs",
        "crates/x.rs:",
        "crates/x.rs:12345678",
        "crates/X.rs:1",
        "C:\\crates\\x.rs:1",
        "lib/x.rs:1",
    ] {
        assert!(SrcLoc::parse(bad).is_none(), "{bad}");
        assert!(
            serde_json::from_value::<SrcLoc>(json!(bad)).is_err(),
            "{bad}"
        );
    }
    let win = SrcLoc::from_file_line("crates\\fin-node\\src\\tasks.rs", 3).unwrap();
    assert_eq!(win.as_str(), "crates/fin-node/src/tasks.rs:3");
    assert!(SrcLoc::from_file_line("C:\\Users\\someone\\.cargo\\registry\\x.rs", 3).is_none());
}

#[test]
fn log_commit_intent_is_content() {
    let spec = kind("log.commit", 1).unwrap();
    let paths = content_paths(spec);
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0].path, [Seg::Key("intent".into())]);
    assert_eq!(paths[0].kind, Some(ContentKind::TxIntent));
    let mut v = serde_json::to_value(AnyPayload::from(log_commit())).unwrap();
    assert!(v["intent"][0]["ops"].is_array());
    redact(spec, &mut v, Purposes::DIAGNOSTICS);
    assert_eq!(
        v,
        json!({"record":"edit","outcome":"rebased","ops":{"item.trim":2},"baseRev":39,"rebasedOver":2,"micros":812})
    );
}

#[test]
fn panic_message_is_content_location_is_meta() {
    let spec = kind("error.panic", 1).unwrap();
    let paths = content_paths(spec);
    assert_eq!(
        paths.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
        ["message"]
    );
    assert_eq!(paths[0].kind, Some(ContentKind::ErrorMessage));
    let schema = (spec.schema)();
    for (field, def) in [("location", "SrcLoc"), ("messageHash", "Hashed")] {
        assert_eq!(
            schema["properties"][field]["$ref"],
            json!(format!("#/$defs/{def}"))
        );
        assert_eq!(schema["$defs"][def]["x-fin-class"], json!("meta"), "{def}");
    }
    assert_eq!(
        schema["$defs"]["Scrubbed"]["x-fin-class"],
        json!("scrubbed")
    );
    let p = samples()
        .into_iter()
        .find(|p| p.kind().name == "error.panic")
        .unwrap();
    let mut v = serde_json::to_value(&p).unwrap();
    assert_eq!(v["message"], json!("index out of range at <path>"));
    redact(spec, &mut v, Purposes::DIAGNOSTICS);
    assert_eq!(v.get("message"), None);
    assert_eq!(v["location"], json!("crates/fin-node/src/tasks.rs:120"));
    assert_eq!(v["where"], json!("finNode"));
}

/// The values at a content path.
fn at<'a>(v: &'a Value, path: &[Seg]) -> Vec<&'a Value> {
    match path {
        [] => vec![v],
        [Seg::Key(k), rest @ ..] => v.get(k.as_str()).map(|x| at(x, rest)).unwrap_or_default(),
        [Seg::Items, rest @ ..] => v
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|x| at(x, rest))
            .collect(),
        [Seg::Values, rest @ ..] => v
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
            .flat_map(|x| at(x, rest))
            .collect(),
    }
}

#[test]
fn redact_removes_every_content_path() {
    let mut removed = 0;
    for p in samples() {
        let spec = p.kind();
        let full = serde_json::to_value(&p).unwrap();
        for purposes in [Purposes::DIAGNOSTICS, Purposes::NONE] {
            let mut v = full.clone();
            redact(spec, &mut v, purposes);
            for c in content_paths(spec) {
                removed += at(&full, &c.path).len();
                assert!(at(&v, &c.path).is_empty(), "{}: {c} survived", spec.name);
            }
            let back = decode_payload(spec, &v).unwrap_or_else(|e| panic!("{}: {e}", spec.name));
            assert_eq!(back.blobs().count(), 0, "{}", spec.name);
        }
    }
    assert_eq!(
        removed,
        2 * 9,
        "every sample content field was filled and removed"
    );
}

#[test]
fn redact_keeps_content_with_training() {
    for p in samples() {
        let full = serde_json::to_value(&p).unwrap();
        for purposes in [Purposes::TRAINING, Purposes::ALL] {
            let mut v = full.clone();
            redact(p.kind(), &mut v, purposes);
            assert_eq!(v, full, "{}", p.kind().name);
        }
    }
}

#[test]
fn training_only_kinds_keep_metadata_under_diagnostics() {
    // A reserved kind (`render.sample`) has an empty payload: nothing to keep yet.
    for p in samples()
        .into_iter()
        .filter(|p| p.kind().training_only && !p.kind().reserved)
    {
        let spec = p.kind();
        let full = serde_json::to_value(&p).unwrap();
        let mut v = full.clone();
        redact(spec, &mut v, Purposes::DIAGNOSTICS);
        let content: BTreeSet<String> = content_paths(spec).iter().map(|c| c.to_string()).collect();
        let kept: BTreeSet<String> = v.as_object().unwrap().keys().cloned().collect();
        let all: BTreeSet<String> = full.as_object().unwrap().keys().cloned().collect();
        assert_eq!(
            kept,
            all.difference(&content).cloned().collect(),
            "{}",
            spec.name
        );
        assert!(!kept.is_empty(), "{}", spec.name);
        decode_payload(spec, &v).unwrap();
    }
    let turn = samples()
        .into_iter()
        .find(|p| p.kind().name == "agent.turn")
        .unwrap();
    let mut v = serde_json::to_value(&turn).unwrap();
    redact(turn.kind(), &mut v, Purposes::DIAGNOSTICS);
    assert_eq!(v["inputTokens"], json!(1200));
    assert_eq!(v["latencyMs"], json!(2100));
    assert!(v.get("messagesDelta").is_none() && v.get("output").is_none());
}

#[test]
fn unknown_kind_and_version_refused() {
    assert!(kind("log.commit", 2).is_none());
    assert!(kind("log.commit", 0).is_none());
    assert!(kind("made.up", 1).is_none());
    assert_eq!(
        require_kind("made.up", 1).unwrap_err().code,
        "EVENT_KIND_UNKNOWN"
    );
    assert_eq!(
        require_kind("agent.turn", 2).unwrap_err().code,
        "EVENT_KIND_UNKNOWN"
    );
    let fake = KindSpec {
        name: "made.up",
        version: 1,
        reserved: false,
        training_only: false,
        sources: &[Source::Fin],
        schema: || json!({}),
    };
    assert_eq!(
        decode_payload(&fake, &json!({})).unwrap_err().code,
        "EVENT_KIND_UNKNOWN"
    );
    assert_eq!(
        validate_payload(&fake, &json!({}), &key, &ScrubCtx::default())
            .unwrap_err()
            .code,
        "EVENT_KIND_UNKNOWN"
    );
    let commit = kind("log.commit", 1).unwrap();
    for bad in [
        json!({ "record": "edit" }),
        json!("text"),
        json!({ "record": "edit", "outcome": "accepted", "micros": 1, "code": "not a code" }),
    ] {
        assert_eq!(
            decode_payload(commit, &bad).unwrap_err().code,
            "EVENT_INVALID",
            "{bad}"
        );
    }
}

#[test]
fn oversized_name_or_code_rejected_by_pattern() {
    let long = |c: char, n: usize| c.to_string().repeat(n);
    assert!(Code::parse(&format!("E{}", long('X', 63))).is_some());
    assert!(Code::parse(&format!("E{}", long('X', 64))).is_none());
    assert!(
        Code::parse("E").is_none()
            && Code::parse("lower").is_none()
            && Code::parse("1ABC").is_none()
    );
    assert!(is_valid_name(&format!("a{}", long('b', 63))));
    assert!(!is_valid_name(&format!("a{}", long('b', 64))));
    assert!(
        !is_valid_name("")
            && !is_valid_name("Upper")
            && !is_valid_name("a\nb")
            && !is_valid_name("a\"b")
            && !is_valid_name("item trim")
    );
    for bad in [
        format!("k1:{}", long('a', 31)),
        format!("k1:{}", long('a', 33)),
        format!("k1:{}", long('A', 32)),
        format!("k2:{}", long('a', 32)),
    ] {
        assert!(Hashed::parse(&bad).is_none(), "{bad}");
    }
    assert!(BlobHash::parse(&format!("kb:{}", long('0', 64))).is_some());
    assert!(BlobHash::parse(&format!("kb:{}", long('0', 65))).is_none());
    assert!(Version::parse("1.2.3-rc.1").is_some());
    for bad in [
        "1.2",
        "1.2.3.4",
        "123456.0.0",
        "1.2.3-",
        "1.2.3-UPPER",
        &format!("1.2.3-{}", long('a', 33)),
    ] {
        assert!(Version::parse(bad).is_none(), "{bad}");
    }
    // Any digit count, as the store accepts.
    assert!(BranchId::parse("br123456789").is_some());
    assert!(BranchId::parse("br12345678901234567890").is_some());
    for bad in ["br", "feature-x", "Main", "br1a", "BR1", "br-1"] {
        assert!(BranchId::parse(bad).is_none(), "{bad}");
    }
    assert!(ProjectId::parse("01jb0000000000000000000000").is_some());
    assert!(ProjectId::parse("01JB0000000000000000000000").is_none());
    assert!(ProjectId::parse("01jb000000000000000000000i").is_none());
}

#[test]
fn scrub_replaces_home_data_tmp_user_host_and_absolute_paths() {
    let posix = ScrubCtx {
        home: Some("/home/alice".into()),
        server_data: Some("/home/alice/.local/share/finisher/".into()),
        tmp: Some("/tmp".into()),
        user: Some("alice".into()),
        host: Some("alice-laptop".into()),
    };
    let s = |t: &str| Scrubbed::new(t, &posix).as_str().to_string();
    // A directory keeps its placeholder; the rest of its path goes.
    assert_eq!(
        s("open /home/alice/.local/share/finisher/p/x.fdb failed"),
        "open <data>/<path> failed"
    );
    assert_eq!(s("read /home/alice/Videos/a.mp4"), "read <home>/<path>");
    assert_eq!(s("in /tmp/fin-1/x"), "in <tmp>/<path>");
    assert_eq!(s("cwd is /home/alice"), "cwd is <home>");
    // Not inside a longer name: `/home/alice2` is another path.
    assert_eq!(s("see /home/alice2/x"), "see <path>");
    assert_eq!(s("Alice on ALICE-LAPTOP"), "<user> on <host>");
    assert_eq!(s("open /var/lib/thing/file: denied"), "open <path> denied");
    assert_eq!(s("path=\"/etc/x\" (~/notes/y)"), "path=\"<path>\" (<path>)");
    assert_eq!(s("ratio 3/4, and/or 1 / 2"), "ratio 3/4, and/or 1 / 2");
    assert_eq!(s("x at /"), "x at /");
    // A path right after a quote runs to the matching quote, spaces and all.
    assert_eq!(
        s("open \"/home/alice/My Videos/a b.mp4\" failed"),
        "open \"<home>/<path>\" failed"
    );
    assert_eq!(s("open '/srv/My Media/a.mov' now"), "open '<path>' now");
    // Unquoted, a path goes on over a word or two when one holds a separator.
    assert_eq!(
        s("load /Users/x/My Movies/a.mov failed"),
        "load <path> failed"
    );
    assert_eq!(s("copy /a/b to /c/d"), "copy <path> to <path>");
    // `~user/` and `~\`.
    assert_eq!(s("open ~bob/notes/x and ~\\y\\z"), "open <path> and <path>");
    // A user name inside a placeholder is not replaced; a user named like one is.
    let odd = ScrubCtx {
        user: Some("path".into()),
        ..ScrubCtx::default()
    };
    assert_eq!(
        Scrubbed::new("path /a/b of path", &odd).as_str(),
        "<user> <path> of <user>"
    );

    let windows = ScrubCtx {
        home: Some("C:\\Users\\Bob".into()),
        user: Some("bob".into()),
        ..ScrubCtx::default()
    };
    let w = |t: &str| Scrubbed::new(t, &windows).as_str().to_string();
    assert_eq!(w("load c:\\users\\bob\\clip.mov"), "load <home>\\<path>");
    assert_eq!(w("D:\\media\\a.wav not found"), "<path> not found");
    assert_eq!(w("D:/media/a.wav not found"), "<path> not found");
    assert_eq!(w("share \\\\nas\\bob\\x"), "share <path>");
    assert_eq!(w("hello bob, bobby"), "hello <user>, <user>by");
    assert_eq!(
        w("run C:\\Program Files\\App\\x.dll failed"),
        "run <path> failed"
    );
    assert_eq!(
        w("run C:\\Program Files (x86)\\App\\x.dll failed"),
        "run <path> failed"
    );
    assert_eq!(
        w("run \"C:\\Program Files\\My App\\x.exe\" -v"),
        "run \"<path>\" -v"
    );
    assert_eq!(
        w("open \"C:\\Users\\Bob\\My Documents\\a.mov\""),
        "open \"<home>\\<path>\""
    );
    // Unicode case: the user and host match in any case.
    let unicode = ScrubCtx {
        home: Some("/home/Ölçü".into()),
        user: Some("Ölçü".into()),
        host: Some("ÉCRAN-PC".into()),
        ..ScrubCtx::default()
    };
    assert_eq!(
        Scrubbed::new("ölçü@écran-pc in /HOME/ÖLÇÜ/x", &unicode).as_str(),
        "<user>@<host> in <home>/<path>"
    );
    // A name shorter than 3 characters is replaced only as a whole word.
    let short = ScrubCtx {
        user: Some("al".into()),
        ..ScrubCtx::default()
    };
    assert_eq!(
        Scrubbed::new("al: also albert", &short).as_str(),
        "<user>: also albert"
    );
    // Long text is cut on a character boundary, and the input is capped first.
    let long = "é".repeat(MAX_SCRUBBED);
    let cut = Scrubbed::new(&long, &ScrubCtx::default());
    assert!(cut.as_str().len() <= MAX_SCRUBBED && cut.as_str().ends_with('…'));
    let huge = format!(
        "{} /home/alice/x",
        "a".repeat(fin_core::events::scrub::MAX_INPUT)
    );
    let cut = Scrubbed::new(&huge, &posix);
    assert!(cut.as_str().len() <= MAX_SCRUBBED && !cut.as_str().contains("alice"));
    // A received value is scrubbed again (absolute paths at least), and with a context too.
    let read: Scrubbed = serde_json::from_value(json!("bad /home/alice/x for alice")).unwrap();
    assert_eq!(read.as_str(), "bad <path> for alice");
    assert_eq!(read.rescrub(&posix).as_str(), "bad <path> for <user>");
}

#[test]
fn path_ref_never_keeps_the_path() {
    let schema = serde_json::to_value(schemars::schema_for!(PathRef)).unwrap();
    let fields: BTreeSet<&str> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(fields, BTreeSet::from(["root", "ext", "hash"]));
    assert_eq!(schema["properties"]["ext"]["$ref"], json!("#/$defs/Name"));
    let p = PathRef::new(PathRoot::Project, Some("MOV"), h('2'));
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        json!({ "root": "project", "ext": "mov", "hash": h('2') })
    );
    let q = PathRef::new(PathRoot::Other, Some("secret-name"), h('2'));
    let text = serde_json::to_string(&q).unwrap();
    assert!(!text.contains("secret"), "{text}");
    assert!(Check::of(&schema).issues.is_empty());
}

#[test]
fn events_module_is_pure() {
    for (path, text) in sources(&events_dir()) {
        // `intent.rs`'s size counter implements `io::Write` and does no I/O.
        let text = if path.ends_with("events/intent.rs") {
            text.replace("std::io::Write", "")
                .replace("std::io::Result", "")
        } else {
            text
        };
        for banned in [
            "std::time",
            "SystemTime",
            "Instant",
            "std::env",
            "env::var",
            "std::fs",
            "std::io",
            "std::net",
            "std::process",
            "std::thread",
            "getrandom",
            "rand::",
        ] {
            assert!(!text.contains(banned), "{}: {banned}", path.display());
        }
    }
}

#[test]
fn strip_content_matches_redact() {
    for p in samples() {
        let spec = p.kind();
        let mut want = serde_json::to_value(&p).unwrap();
        redact(spec, &mut want, Purposes::DIAGNOSTICS);
        let mut typed = p.clone();
        typed.strip_content();
        assert_eq!(serde_json::to_value(&typed).unwrap(), want, "{}", spec.name);
        assert_eq!(typed.blobs().count(), 0, "{}", spec.name);
    }
}

#[test]
fn envelope_ref_serialises_as_envelope() {
    let mut payload = AnyPayload::from(log_commit());
    let (eid, session) = (ulid("01JC0000000000000000000000"), ulid(SESSION));
    let ctx = Ctx {
        project: ProjectId::parse("01jb0000000000000000000000"),
        tx_id: Some(id(TX)),
        ..Ctx::default()
    };
    let consent = ConsentStamp {
        rev: 1,
        purposes: Purposes::DIAGNOSTICS,
        basis: Basis::Default,
    };
    payload.strip_content();
    let borrowed = EnvelopeRef::new(
        &eid,
        7,
        &session,
        Source::FinNode,
        consent,
        Some(&ctx),
        &payload,
    );
    let owned = Envelope {
        v: ENVELOPE_VERSION,
        id: eid.clone(),
        ts: 7,
        session: session.clone(),
        source: Source::FinNode,
        kind: KindName::of(payload.kind()),
        kv: payload.kind().version,
        consent,
        ctx: Some(ctx.clone()),
        payload: serde_json::to_value(&payload).unwrap(),
    };
    let text = serde_json::to_string(&borrowed).unwrap();
    assert_eq!(
        serde_json::to_value(borrowed).unwrap(),
        serde_json::to_value(&owned).unwrap()
    );
    assert_eq!(serde_json::from_str::<Envelope>(&text).unwrap(), owned);
    assert!(!text.contains("\"intent\""));
    let none = EnvelopeRef {
        ctx: None,
        ..borrowed
    };
    assert!(!serde_json::to_string(&none).unwrap().contains("\"ctx\""));
}

#[test]
fn envelope_strings_are_classified() {
    let schema = serde_json::to_value(schemars::schema_for!(Envelope)).unwrap();
    let c = Check::of(&schema);
    assert!(c.issues.is_empty(), "{:?}", c.issues);
    for (field, def) in [("id", "Ulid"), ("session", "Ulid"), ("kind", "KindName")] {
        assert_eq!(
            schema["properties"][field]["$ref"],
            json!(format!("#/$defs/{def}")),
            "{field}"
        );
    }
    assert_eq!(
        schema["properties"]["payload"]["$ref"],
        json!("#/$defs/AnyPayload")
    );
    for bad in ["log", "Log.commit", "log.commit.x", "log.co mmit", ""] {
        assert!(KindName::parse(bad).is_none(), "{bad}");
    }
    for k in kinds() {
        assert_eq!(KindName::of(k).as_str(), k.name);
    }
    for bad in [
        "01jc0000000000000000000000",
        "81JC0000000000000000000000",
        "x",
    ] {
        assert!(Ulid::parse(bad).is_none(), "{bad}");
    }
}

#[test]
fn ulid_from_u128_is_canonical() {
    assert_eq!(Ulid::from_u128(0).as_str(), "00000000000000000000000000");
    assert_eq!(
        Ulid::from_u128(u128::MAX).as_str(),
        "7ZZZZZZZZZZZZZZZZZZZZZZZZZ"
    );
    let u = Ulid::from_u128((1_794_571_200_123u128 << 80) | 0xdead_beef);
    assert_eq!(Ulid::parse(u.as_str()), Some(u.clone()));
    assert!(!IdRef::from(u).is_hashed());
}

#[test]
fn meta_string_without_pattern_is_flagged() {
    let c = Check::of(&json!({
        "type": "object",
        "properties": {
            "loose": { "type": "string", "x-fin-class": "meta" },
            "fine": { "type": "string", "x-fin-class": "meta", "pattern": "^a$" },
        },
    }));
    assert_eq!(c.issues, ["loose: meta string without a pattern"]);
}

#[test]
#[should_panic(expected = "unresolved $ref")]
fn test_walk_panics_on_unresolved_ref() {
    Check::of(&json!({ "properties": { "x": { "$ref": "#/$defs/Missing" } } }));
}

#[test]
#[should_panic(expected = "unresolved $ref")]
fn redact_walk_panics_on_unresolved_ref() {
    let fake = KindSpec {
        name: "made.up",
        version: 1,
        reserved: false,
        training_only: false,
        sources: &[Source::Fin],
        schema: || json!({ "properties": { "x": { "$ref": "#/$defs/Missing" } } }),
    };
    content_paths(&fake);
}

#[test]
fn intent_content_holds_free_text_only_in_ops() {
    let c = check("log.commit", 1);
    assert!(c.issues.is_empty(), "{:?}", c.issues);
    assert!(!c.raw_in_content.is_empty());
    for raw in &c.raw_in_content {
        assert!(
            raw.starts_with("intent[].ops[]"),
            "{raw}: free text outside the ops"
        );
    }
    let schema = (kind("log.commit", 1).unwrap().schema)();
    let tx = &schema["$defs"]["IntentTx"]["properties"];
    for (field, def) in [
        ("txId", "IdRef"),
        ("author", "Hashed"),
        ("branch", "BranchId"),
        ("argsHash", "Hashed"),
    ] {
        assert_eq!(
            tx[field]["$ref"],
            json!(format!("#/$defs/{def}")),
            "{field}"
        );
    }
    // Every other kind's content is classed or opaque JSON: no free text field.
    for k in kinds().iter().filter(|k| k.name != "log.commit") {
        let c = Check::of(&(k.schema)());
        assert!(
            c.raw_in_content.is_empty(),
            "{}: {:?}",
            k.name,
            c.raw_in_content
        );
    }
}

#[test]
fn job_kinds_registered() {
    // `job.done` is `kv` 2 (the optional `media` object), `worker.exit` still `kv` 1.
    for (name, kv) in [("job.done", 2), ("worker.exit", 1)] {
        let k = kind(name, kv).unwrap_or_else(|| panic!("{name} at v{kv}"));
        assert!(!k.reserved && !k.training_only, "{name}");
        assert_eq!(k.sources, [Source::Finisherd], "{name}");
    }
}

#[test]
fn names_jobs_tables_valid() {
    use fin_core::events::names_jobs::{JOB_KINDS, SIGNALS, WORKER_FAMILIES};
    for t in [&JOB_KINDS, &WORKER_FAMILIES, &SIGNALS] {
        assert!(
            TABLES.iter().any(|x| std::ptr::eq(*x, t)),
            "{} in TABLES",
            t.table
        );
        let unique: BTreeSet<&str> = t.entries().iter().copied().collect();
        assert_eq!(unique.len(), t.entries().len(), "{} repeats", t.table);
    }
    // `test.*`, 5D's `library.*` and sub-project 6's `media.*`; 8C adds its own.
    assert!(
        JOB_KINDS.entries().iter().all(|k| k.starts_with("test.")
            || k.starts_with("library.")
            || k.starts_with("media."))
    );
    assert!(JOB_KINDS.get("media.look").is_some());
    for f in ["test", "library", "media", "analyze", "plugin"] {
        assert!(WORKER_FAMILIES.get(f).is_some(), "{f}");
    }
    assert!(SIGNALS.entries().iter().all(|s| !s.starts_with("sig")));
    // A value outside a table reads as `other`.
    let p: WorkerExit = serde_json::from_value(json!({
        "role": "worker", "family": "nope", "reason": "done", "signal": "usr1",
        "jobsServed": 1, "lifetimeS": 2, "sandboxed": false,
    }))
    .unwrap();
    assert_eq!(p.family, Name::OTHER);
    assert_eq!(p.signal, Some(Name::OTHER));
}

#[test]
fn worker_exit_reasons_closed() {
    let names: Vec<String> = ExitReason::ALL
        .iter()
        .map(|r| {
            serde_json::to_value(r)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        names,
        [
            "done",
            "idleExpired",
            "recycled",
            "cancelled",
            "killed",
            "crashed",
            "hung",
            "stalled",
            "abandoned",
            "spawnFailed",
            "oom",
            "protocolError"
        ]
    );
    let schema = (kind("worker.exit", 1).unwrap().schema)();
    let reason = &schema["$defs"]["ExitReason"]["enum"];
    assert_eq!(reason.as_array().map(Vec::len), Some(12), "{schema}");
}

#[test]
fn library_scan_payload_is_closed() {
    let k = kind("library.scan", 1).expect("library.scan at v1");
    assert!(!k.reserved && !k.training_only);
    assert_eq!(k.sources, [Source::Finisherd]);
    let schema = (k.schema)();
    let props: BTreeSet<&str> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    // No field names a library, a root or a path (Decision 50).
    let want: BTreeSet<&str> = [
        "storage",
        "full",
        "trigger",
        "outcome",
        "code",
        "files",
        "dirs",
        "added",
        "changed",
        "missing",
        "moved",
        "placeholders",
        "micros",
    ]
    .into_iter()
    .collect();
    assert_eq!(props, want, "{schema}");
    let trigger = &schema["$defs"]["ScanTrigger"]["enum"];
    assert_eq!(
        trigger,
        &json!(["add", "schedule", "demand", "watch", "mount"]),
        "{schema}"
    );
    // An unknown field or trigger is refused.
    let mut v = json!({
        "storage": "local", "full": true, "trigger": "add", "outcome": "succeeded", "files": 3,
        "dirs": 1, "added": 4, "changed": 0, "missing": 0, "moved": 0, "placeholders": 0,
        "micros": 10,
    });
    assert!(decode_payload(k, &v).is_ok());
    v["trigger"] = json!("cron");
    assert!(decode_payload(k, &v).is_err());
}

#[test]
fn sandbox_unconfined_payload_is_closed() {
    let k = kind("sandbox.unconfined", 1).expect("sandbox.unconfined at v1");
    assert!(!k.reserved && !k.training_only);
    assert_eq!(k.sources, [Source::Finisherd]);
    let schema = (k.schema)();
    let props: BTreeSet<&str> = schema["properties"]
        .as_object()
        .expect("properties")
        .keys()
        .map(String::as_str)
        .collect();
    // Metadata only: no path, host or process.
    assert_eq!(
        props,
        BTreeSet::from(["profile", "os", "reason"]),
        "{schema}"
    );
    let ok = json!({"profile": "worker", "os": "linux", "reason": "landlockDisabled"});
    assert!(decode_payload(k, &ok).is_ok());
    // The names are closed; there is no opt-out reason.
    for bad in [
        json!({"profile": "shell", "os": "linux", "reason": "landlockDisabled"}),
        json!({"profile": "worker", "os": "linux", "reason": "userOptOut"}),
    ] {
        assert!(decode_payload(k, &bad).is_err(), "{bad}");
    }
}

/// `agent.turn.purpose` (7B Decision 24) is additive in v1: a turn without it serialises as
/// before; a compaction turn carries `purpose: compaction` through the wire form; any other
/// value is refused.
#[test]
fn agent_turn_purpose_is_additive() {
    let key = |data: &[u8]| -> [u8; 16] { blake3::hash(data).as_bytes()[..16].try_into().unwrap() };
    let b3 = "blake3:0000000000000000000000000000000000000000000000000000000000000001";
    let wire = json!({
        "turnId": "01JC0000000000000000000005", "runId": "01JC0000000000000000000007", "seq": 2,
        "parentTurnId": "01JC0000000000000000000004", "harness": "builtin",
        "provider": "openrouter", "model": "acme/x", "weights": "open", "inputTokens": 700,
        "outputTokens": 90, "cacheReadTokens": 600, "cacheWriteTokens": 0, "reasoningTokens": 0,
        "latencyMs": 5, "stopReason": "endTurn", "costBasis": "reported", "capture": "exact",
        "systemHash": b3, "toolsHash": b3, "bodyHash": b3,
    });
    let spec = kind("agent.turn", 1).unwrap();
    let received = |v: &Value| validate_payload(spec, v, &key, &ScrubCtx::default());
    let plain = serde_json::to_value(received(&wire).unwrap()).unwrap();
    assert!(plain.get("purpose").is_none(), "the v1 shape is unchanged");
    let mut compaction = wire.clone();
    compaction["purpose"] = json!("compaction");
    let payload = received(&compaction).unwrap();
    let AnyPayload::AgentTurn(t) = &payload else {
        panic!("kind")
    };
    assert_eq!(t.purpose, Some(TurnPurpose::Compaction));
    check_golden(
        "agent_turn_compaction.json",
        &serde_json::to_string_pretty(&payload).unwrap(),
    );
    let mut back = serde_json::to_value(&payload).unwrap();
    let purpose = back.as_object_mut().unwrap().remove("purpose");
    assert_eq!((purpose, back), (Some(json!("compaction")), plain));
    for bad in [json!("summary"), json!(""), json!(1)] {
        let mut v = wire.clone();
        v["purpose"] = bad.clone();
        assert!(received(&v).is_err(), "{bad}");
    }
}
