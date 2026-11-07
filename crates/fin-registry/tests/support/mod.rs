//! Shared by the fin-registry tests.
#![allow(dead_code)]

pub mod hostile;
pub mod tuf;

use std::path::Path;

use serde_json::json;

/// A code plugin manifest whose native executables are `exec`.
pub fn manifest_with_exec(exec: &[(&str, &str)]) -> Vec<u8> {
    let platforms: Vec<&str> = exec.iter().map(|(p, _)| *p).collect();
    let exec: serde_json::Map<String, serde_json::Value> = exec
        .iter()
        .map(|(p, f)| (p.to_string(), json!(f)))
        .collect();
    serde_json::to_vec(&json!({
        "manifestVersion": 1,
        "id": "@acme/tools",
        "version": "1.0.0",
        "kind": "code",
        "hostApi": "^1.0",
        "license": "MIT",
        "publisher": { "name": "Acme" },
        "platforms": platforms,
        "runtime": { "type": "native", "exec": exec },
    }))
    .unwrap()
}

/// A content pack manifest.
pub fn content_manifest() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "manifestVersion": 1,
        "id": "@acme/pack",
        "version": "1.0.0",
        "kind": "content",
        "hostApi": "^1.0",
        "license": "MIT",
        "publisher": { "name": "Acme" },
        "contributes": { "captionStyles": [{ "name": "bold", "file": "styles/bold.json" }] },
    }))
    .unwrap()
}

/// Writes `files` (path → bytes) under `root`, creating directories.
pub fn write_tree(root: &Path, files: &[(&str, &[u8])]) {
    for (path, bytes) in files {
        let p = root.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }
}

/// Names of the entries directly under `dir`, sorted.
pub fn listing(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
