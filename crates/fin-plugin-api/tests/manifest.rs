//! Manifest grammar and shape (spec §4.1, 8A Decisions 1–3).

mod support;

use fin_plugin_api::{
    MAX_MANIFEST_BYTES, Manifest, PluginId, SourceClass, manifest::RESERVED_PUBLISHERS,
};
use serde_json::{Value, json};
use support::{MemFiles, check, check_with, ctx, fixture, reasons, set};

#[test]
fn ids_follow_the_grammar() {
    let long_pub = "a".repeat(39);
    let long_name = "b".repeat(64);
    for ok in [
        "@acme/tools".to_string(),
        "@a1/b2".into(),
        "@ab/x-y-z".into(),
        "@0x/9lives".into(),
        "@acme/com0".into(),
        "@acme/console".into(),
        "@acme/lpt10".into(),
        format!("@{long_pub}/{long_name}"),
    ] {
        let id = PluginId::parse(&ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        assert_eq!(id.to_string(), ok);
    }
    for (bad, reason) in [
        ("acme/tools".to_string(), "idShape"),
        ("@acme".into(), "idShape"),
        ("@Acme/tools".into(), "idPublisher"),
        ("@a/tools".into(), "idPublisher"),
        ("@-acme/tools".into(), "idPublisher"),
        ("@acme-/tools".into(), "idPublisher"),
        ("@ac--me/tools".into(), "idPublisher"),
        (format!("@{}a/tools", "a".repeat(39)), "idPublisher"),
        ("@acme/t".into(), "idName"),
        ("@acme/tools-".into(), "idName"),
        ("@acme/to--ols".into(), "idName"),
        ("@acme/to_ols".into(), "idName"),
        ("@acme/tools/x".into(), "idName"),
        (format!("@acme/{}b", "b".repeat(64)), "idName"),
        // Windows device names, as a publisher or a name (ids are lower case already).
        ("@con/tools".into(), "idPublisher"),
        ("@nul/tools".into(), "idPublisher"),
        ("@lpt9/tools".into(), "idPublisher"),
        ("@acme/aux".into(), "idName"),
        ("@acme/prn".into(), "idName"),
        ("@acme/com1".into(), "idName"),
    ] {
        assert_eq!(
            PluginId::parse(&bad).map_err(|e| e.reason()),
            Err(reason),
            "{bad}"
        );
    }
    // Through a manifest, a bad id fails the parse with the id in the message.
    let errs = check(&set(fixture("content-pack"), "/id", json!("@Acme/x1"))).unwrap_err();
    assert_eq!(errs[0].reason(), Some("schema"));
    assert!(errs[0].message.contains("@Acme/x1"), "{}", errs[0].message);
}

#[test]
fn reserved_publishers_refused_outside_bundled() {
    for p in RESERVED_PUBLISHERS {
        let v = set(fixture("content-pack"), "/id", json!(format!("@{p}/pack")));
        assert_eq!(
            reasons(check(&v)),
            ["reservedPublisher"],
            "@{p} from a registry"
        );
        let dev = check_with(&v, &MemFiles::full(), &ctx(SourceClass::Dev));
        assert_eq!(
            reasons(dev),
            ["reservedPublisher"],
            "@{p} in developer mode"
        );
        let bundled = check_with(&v, &MemFiles::full(), &ctx(SourceClass::Bundled));
        assert!(bundled.is_ok(), "@{p} bundled: {bundled:?}");
    }
    let dev = set(fixture("content-pack"), "/id", json!("@dev/pack"));
    assert_eq!(reasons(check(&dev)), ["devPublisher"]);
    assert_eq!(
        reasons(check_with(
            &dev,
            &MemFiles::full(),
            &ctx(SourceClass::Bundled)
        )),
        ["devPublisher"]
    );
    assert!(check_with(&dev, &MemFiles::full(), &ctx(SourceClass::Dev)).is_ok());
}

#[test]
fn build_metadata_refused() {
    let v = set(fixture("content-pack"), "/version", json!("1.2.0+build.7"));
    assert_eq!(reasons(check(&v)), ["buildMetadata"]);
    let pre = set(fixture("content-pack"), "/version", json!("1.2.0-rc.1"));
    assert!(check(&pre).is_ok());
    for bad in ["1.2", "v1.2.0", "01.2.0"] {
        let v = set(fixture("content-pack"), "/version", json!(bad));
        assert_eq!(reasons(check(&v)), ["schema"], "{bad}");
    }
}

#[test]
fn unknown_fields_refused() {
    for pointer in [
        "/extra",
        "/publisher/extra",
        "/contributes/captionStyles/0/extra",
        "/contributes/fontCatalogs/0/extra",
    ] {
        let v = set(fixture("content-pack"), pointer, json!(1));
        assert_eq!(reasons(check(&v)), ["schema"], "{pointer}");
    }
    for pointer in [
        "/runtime/extra",
        "/permissions/0/extra",
        "/permissions/1/extra",
        "/permissions/2/extra",
        "/models/0/extra",
    ] {
        let v = set(fixture("code-native"), pointer, json!(1));
        assert_eq!(reasons(check(&v)), ["schema"], "{pointer}");
    }
    // A misspelt permission key or perm never parses into a weaker grant.
    let v = set(
        fixture("code-native"),
        "/permissions/1",
        json!({ "perm": "net", "origin": ["https://api.example.org"] }),
    );
    assert_eq!(reasons(check(&v)), ["schema"]);
    let v = set(
        fixture("code-native"),
        "/permissions/0",
        json!({ "perm": "project:reads" }),
    );
    assert_eq!(reasons(check(&v)), ["schema"]);
    let v = set(fixture("code-native"), "/runtime/type", json!("python"));
    assert_eq!(reasons(check(&v)), ["schema"]);
}

#[test]
fn manifest_schema_round_trips() {
    for name in ["content-pack", "code-native", "code-node"] {
        let v = fixture(name);
        let m: Manifest = check(&v).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert_eq!(serde_json::to_value(&m).unwrap(), v, "{name}");
        let again: Manifest = serde_json::from_value(serde_json::to_value(&m).unwrap()).unwrap();
        assert_eq!(again, m, "{name}");
    }
    let schema = serde_json::to_value(schemars::schema_for!(Manifest)).unwrap();
    let required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for field in [
        "manifestVersion",
        "id",
        "version",
        "kind",
        "hostApi",
        "license",
        "publisher",
    ] {
        assert!(
            required.contains(&field),
            "{field} is required: {required:?}"
        );
    }
    assert_eq!(schema["additionalProperties"], json!(false));
    let contributes = &schema["$defs"]["Contributes"]["properties"];
    let keys: Vec<&String> = contributes.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["captionStyles", "fontCatalogs"]);
}

#[test]
fn manifest_size_and_encoding() {
    let mut v = fixture("content-pack");
    v["publisher"]["name"] = Value::String("x".repeat(MAX_MANIFEST_BYTES));
    let errs = check(&v).unwrap_err();
    assert_eq!(errs[0].code, "MANIFEST_TOO_LARGE");
    let files = MemFiles::full();
    let c = ctx(SourceClass::Registry);
    let bytes = serde_json::to_vec(&fixture("content-pack")).unwrap();
    let mut bom = b"\xef\xbb\xbf".to_vec();
    bom.extend_from_slice(&bytes);
    let r = fin_plugin_api::validate(&bom, &files, &c);
    assert_eq!(reasons(r), ["utf8"]);
    let r = fin_plugin_api::validate(b"{\"id\": \"\xff\"}", &files, &c);
    assert_eq!(reasons(r), ["utf8"]);
    let v = set(fixture("content-pack"), "/manifestVersion", json!(2));
    assert_eq!(reasons(check(&v)), ["manifestVersion"]);
}
