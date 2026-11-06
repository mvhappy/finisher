//! The font catalog (4C Decisions 30 and 31): the built-in catalog, the licence gate's
//! shared cases, and face and fallback lookup.

use fin_fonts::{FontCatalog, FontFormat, MAX_FILE_SIZE, RULES_JSON};
use serde_json::Value;

const CASES: &str = include_str!("catalog_gate_cases.json");

fn set(doc: &mut Value, pointer: &str, value: Value) {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let target = doc
        .pointer_mut(parent)
        .unwrap_or_else(|| panic!("{parent}"));
    match target {
        Value::Object(map) if value.is_null() => {
            map.remove(key);
        }
        Value::Object(map) => {
            map.insert(key.to_string(), value);
        }
        Value::Array(items) => items[key.parse::<usize>().unwrap()] = value,
        _ => panic!("{pointer}: not a container"),
    }
}

#[test]
fn builtin_catalog_validates() {
    let c = FontCatalog::builtin();
    c.validate().unwrap();
    assert_eq!(c.catalog_version, "2026.11.1");
    assert_eq!(
        c.fallback,
        [
            "noto-sans",
            "noto-sans-arabic",
            "noto-sans-hebrew",
            "noto-sans-cjk",
            "noto-color-emoji"
        ]
    );
    let ids: Vec<&str> = c.entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "inter",
            "noto-sans",
            "noto-sans-arabic",
            "noto-sans-hebrew",
            "noto-sans-cjk",
            "noto-color-emoji"
        ]
    );
    for (e, s) in c.styles() {
        assert_eq!(e.licence.spdx, "OFL-1.1", "{}", e.id);
        assert!(s.file.size <= MAX_FILE_SIZE);
        assert!(!s.sources.is_empty());
    }
    // Inter comes from the release zip, as archive members with the zip's own hash and size.
    let inter = c.entry("inter").unwrap();
    for s in &inter.styles {
        let src = &s.sources[0];
        assert!(src.url.contains("/releases/download/v4.1/"));
        assert!(src.archive.is_some() && src.sha256.is_some() && src.size.is_some());
    }
    let cjk = c.entry("noto-sans-cjk").unwrap();
    for s in &cjk.styles {
        assert_eq!(s.file.format, FontFormat::Ttc);
        assert_eq!(s.face().family, "Noto Sans CJK SC");
    }
}

#[test]
fn catalog_gate_refuses_nc_licence() {
    let doc: Value = serde_json::from_str(CASES).unwrap();
    FontCatalog::from_json(&doc["base"].to_string()).expect("the base catalog passes");
    let rules: Value = serde_json::from_str(RULES_JSON).unwrap();
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let rule = case["rule"].as_str().unwrap();
        assert!(
            rules["rules"].get(rule).is_some(),
            "{name}: {rule} is not a rule"
        );
        let mut cat = doc["base"].clone();
        for (pointer, value) in case["set"].as_object().unwrap() {
            set(&mut cat, pointer, value.clone());
        }
        let err = FontCatalog::from_json(&cat.to_string())
            .err()
            .unwrap_or_else(|| panic!("{name}: accepted"));
        assert_eq!(err.code, "FONT_CATALOG_INVALID", "{name}");
        assert_eq!(err.details["rule"], rule, "{name}: {}", err.message);
    }
}

#[test]
fn find_face_and_fallback_for_script() {
    let c = FontCatalog::builtin();
    let (e, s, f) = c.find_face("Noto Sans CJK SC", 400, false).unwrap();
    assert_eq!(
        (e.id.as_str(), s.id.as_str(), f.index),
        ("noto-sans-cjk", "regular", 2)
    );
    let (e, s, f) = c.find_face("Inter", 700, false).unwrap();
    assert_eq!(
        (e.id.as_str(), s.id.as_str(), f.index),
        ("inter", "bold", 0)
    );
    assert!(c.find_face("Inter", 700, true).is_none());
    assert!(c.find_face("Comic Sans", 400, false).is_none());

    let fallback = |script: &str| {
        c.fallback_for(script)
            .map(|(e, s)| (e.id.clone(), s.id.clone(), s.face().family.clone()))
    };
    let want = |e: &str, f: &str| Some((e.to_string(), "regular".to_string(), f.to_string()));
    assert_eq!(fallback("Latn"), want("noto-sans", "Noto Sans"));
    assert_eq!(fallback("Cyrl"), want("noto-sans", "Noto Sans"));
    assert_eq!(
        fallback("Arab"),
        want("noto-sans-arabic", "Noto Sans Arabic")
    );
    assert_eq!(
        fallback("Hebr"),
        want("noto-sans-hebrew", "Noto Sans Hebrew")
    );
    for s in ["Hani", "Hira", "Kana", "Hang"] {
        assert_eq!(
            fallback(s),
            want("noto-sans-cjk", "Noto Sans CJK SC"),
            "{s}"
        );
    }
    assert_eq!(
        fallback("Zsye"),
        want("noto-color-emoji", "Noto Color Emoji")
    );
    assert_eq!(fallback("Thai"), None);

    let (_, bold) = c
        .by_sha("87cb2d84472a7d66da659ee47b6cdb9552326e8c128245231f191b6ac72529d9")
        .unwrap();
    assert_eq!((bold.id.as_str(), bold.weight), ("bold", 700));
    assert!(c.by_sha(&"0".repeat(64)).is_none());
}

#[test]
fn catalog_round_trips_and_has_a_schema() {
    let c = FontCatalog::builtin();
    let again = FontCatalog::from_json(&serde_json::to_string(c).unwrap()).unwrap();
    assert_eq!(&again, c);
    let schema = schemars::schema_for!(FontCatalog);
    assert!(
        schema
            .as_value()
            .pointer("/properties/catalogVersion")
            .is_some()
    );
}
