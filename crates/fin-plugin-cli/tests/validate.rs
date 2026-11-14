//! `fin-plugin validate` holds a pack's caption styles and font catalogs to the host's own
//! checks (8A Task 12c), offline: a publisher finds a bad style or catalog, or a catalog that
//! would not merge into the built-in one, before publishing rather than at install.

use std::path::Path;
use std::process::Command;

use serde_json::{Value, json};

const BUILTIN: &str = include_str!("../../fin-fonts/catalog/catalog.json");

/// Runs `fin-plugin validate <dir>`: the exit code and the document (stdout, else stderr).
fn validate(dir: &Path) -> (i32, Value) {
    let o = Command::new(env!("CARGO_BIN_EXE_fin-plugin"))
        .arg("validate")
        .arg(dir)
        // Offline: nothing it does may need a network or a server.
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("FIN_SERVER_DATA", dir.join("no-server"))
        .output()
        .unwrap();
    let code = o.status.code().unwrap();
    let text = if code == 0 { o.stdout } else { o.stderr };
    (code, serde_json::from_slice(&text).unwrap())
}

fn scaffold(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    let o = Command::new(env!("CARGO_BIN_EXE_fin-plugin"))
        .args(["new", "content"])
        .arg(&dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    dir
}

fn manifest(dir: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("finisher-plugin.json")).unwrap()).unwrap()
}

fn write_json(dir: &Path, rel: &str, v: &Value) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, serde_json::to_vec_pretty(v).unwrap()).unwrap();
}

/// The first problem's details (the error lists every problem; these have one each).
fn problem(doc: &Value) -> &Value {
    assert_eq!(doc["code"], "PLUGIN_MANIFEST_INVALID", "{doc}");
    &doc["details"]["problems"][0]["details"]
}

/// A catalog of one entry: the built-in `noto-sans` (a static font, not a variable one) as
/// `id`; with `fresh`, under its own family and a SHA-256 of its own.
fn catalog(id: &str, fresh: Option<u8>) -> Value {
    let builtin: Value = serde_json::from_str(BUILTIN).unwrap();
    let mut entry = builtin["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["id"] == "noto-sans")
        .unwrap()
        .clone();
    entry["id"] = json!(id);
    if let Some(seed) = fresh {
        let family = format!("Acme {id}");
        entry["family"] = json!(family);
        for (k, style) in entry["styles"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .enumerate()
        {
            let file = &mut style["file"];
            file["sha256"] = json!(format!("{seed:02x}{k:02x}").repeat(16));
            file["faces"][0]["family"] = json!(family);
        }
    }
    json!({ "catalogVersion": builtin["catalogVersion"], "fallback": [], "entries": [entry] })
}

/// A pack contributing these catalogs (`fonts/<i>.json`) beside its scaffolded style.
fn with_catalogs(dir: &Path, catalogs: &[Value]) {
    let mut m = manifest(dir);
    let list: Vec<Value> = (0..catalogs.len())
        .map(|i| json!({ "file": format!("fonts/{i}.json") }))
        .collect();
    m["contributes"]["fontCatalogs"] = json!(list);
    write_json(dir, "finisher-plugin.json", &m);
    for (i, c) in catalogs.iter().enumerate() {
        write_json(dir, &format!("fonts/{i}.json"), c);
    }
}

#[test]
fn fin_plugin_validate_offline() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = scaffold(tmp.path(), "fine");
    with_catalogs(
        &dir,
        &[
            catalog("acme-sans", Some(1)),
            catalog("acme-serif", Some(2)),
        ],
    );
    let (code, doc) = validate(&dir);
    assert_eq!(code, 0, "{doc}");
    assert_eq!(doc["ok"], true);
}

#[test]
fn caption_styles_get_the_hosts_check() {
    let tmp = tempfile::tempdir().unwrap();
    let style: Value = serde_json::from_slice(
        &std::fs::read(scaffold(tmp.path(), "base").join("styles/default.json")).unwrap(),
    )
    .unwrap();
    for (name, change) in [
        ("unknown-key", json!({ "shadow": true })),
        ("bad-colour", json!({ "color": "white" })),
        ("an-id", json!({ "id": "st1" })),
        ("too-many-lines", json!({ "maxLines": 99 })),
    ] {
        let dir = scaffold(tmp.path(), name);
        let mut bad = style.clone();
        for (k, v) in change.as_object().unwrap() {
            bad[k] = v.clone();
        }
        write_json(&dir, "styles/default.json", &bad);
        let (code, doc) = validate(&dir);
        assert_eq!(code, 3, "{name}: {doc}");
        let d = problem(&doc);
        assert_eq!(d["reason"], "captionStyle", "{name}: {doc}");
        assert_eq!(d["path"], "/contributes/captionStyles/0/file", "{name}");
    }
}

#[test]
fn font_catalogs_get_the_hosts_rules() {
    let tmp = tempfile::tempdir().unwrap();
    let mut nc = catalog("acme-nc", Some(3));
    nc["entries"][0]["licence"]["spdx"] = json!("CC-BY-NC-4.0");
    let mut moving = catalog("acme-moving", Some(4));
    moving["entries"][0]["styles"][0]["sources"][0]["url"] =
        json!("https://example.com/latest/a.ttf");
    for (name, c) in [
        ("nc", nc),
        ("moving", moving),
        ("not-a-catalog", json!({ "entries": 1 })),
    ] {
        let dir = scaffold(tmp.path(), name);
        with_catalogs(&dir, &[c]);
        let (code, doc) = validate(&dir);
        assert_eq!(code, 3, "{name}: {doc}");
        assert_eq!(problem(&doc)["reason"], "fontCatalog", "{name}: {doc}");
    }
}

#[test]
fn font_catalogs_must_merge_into_the_built_in_one() {
    let tmp = tempfile::tempdir().unwrap();
    // The built-in file under a new id and family (its own family would be taken first).
    let mut copy = catalog("acme-copy", None);
    copy["entries"][0]["family"] = json!("Acme Copy");
    for style in copy["entries"][0]["styles"].as_array_mut().unwrap() {
        style["file"]["faces"][0]["family"] = json!("Acme Copy");
    }
    // Review H1: a face (or an entry) named like a built-in family would win new pins for it.
    let mut shadow = catalog("acme-shadow", Some(9));
    shadow["entries"][0]["styles"][0]["file"]["faces"][0]["family"] = json!("Inter");
    let mut shadow_entry = catalog("acme-shadow-entry", Some(10));
    shadow_entry["entries"][0]["family"] = json!("noto sans");
    // A built-in id; the built-in file under a new id; the pack's own file listed twice.
    for (name, catalogs, index, reason, rule) in [
        (
            "taken-id",
            vec![catalog("inter", Some(5))],
            0,
            "catalogIdTaken",
            "catalogIdTaken",
        ),
        ("builtin-file", vec![copy], 0, "catalogMerge", "unique"),
        (
            "shadow-face-family",
            vec![shadow],
            0,
            "catalogIdTaken",
            "catalogIdTaken",
        ),
        (
            "shadow-entry-family",
            vec![shadow_entry],
            0,
            "catalogIdTaken",
            "catalogIdTaken",
        ),
        (
            "twice",
            vec![catalog("acme-a", Some(6)), catalog("acme-b", Some(6))],
            1,
            "catalogMerge",
            "unique",
        ),
        (
            "same-id",
            vec![catalog("acme-a", Some(7)), catalog("acme-a", Some(8))],
            1,
            "catalogIdTaken",
            "catalogIdTaken",
        ),
    ] {
        let dir = scaffold(tmp.path(), name);
        with_catalogs(&dir, &catalogs);
        let (code, doc) = validate(&dir);
        assert_eq!(code, 3, "{name}: {doc}");
        let d = problem(&doc);
        assert_eq!(d["reason"], reason, "{name}: {doc}");
        assert_eq!(d["rule"], rule, "{name}: {doc}");
        assert_eq!(
            d["path"],
            format!("/contributes/fontCatalogs/{index}/file"),
            "{name}"
        );
        assert_eq!(d["file"], format!("fonts/{index}.json"), "{name}");
    }
}
