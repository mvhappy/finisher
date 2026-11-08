//! Shared by the fin-plugin-api tests: fixtures, an in-memory package and contexts.
#![allow(dead_code)]

use std::collections::BTreeMap;

use fin_plugin_api::{
    ContentChecks, Invalid, Manifest, PackageFiles, SourceClass, StructuralChecks, ValidateCtx,
    host_api, validate,
};
use serde_json::{Value, json};

pub fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A package's files in memory.
#[derive(Clone, Default)]
pub struct MemFiles(pub BTreeMap<String, Vec<u8>>);

impl MemFiles {
    /// Every file any fixture names.
    pub fn full() -> MemFiles {
        let en = json!({ "style.boldPop": "Bold pop", "model.tiny": "Finds things" });
        let de = json!({ "style.boldPop": "Fett", "model.tiny": "Findet Dinge" });
        let mut m = BTreeMap::new();
        for (path, bytes) in [
            ("fonts/catalog.json", br#"{"entries": []}"#.to_vec()),
            ("styles/bold-pop.json", br#"{"text": {}}"#.to_vec()),
            ("i18n/en.json", en.to_string().into_bytes()),
            ("i18n/de.json", de.to_string().into_bytes()),
            ("bin/darwin-arm64/tools", b"\x7fELF".to_vec()),
            ("bin/linux-x64/tools", b"\x7fELF".to_vec()),
            ("dist/plugin.mjs", b"export {}".to_vec()),
        ] {
            m.insert(path.to_string(), bytes);
        }
        MemFiles(m)
    }

    pub fn without(mut self, path: &str) -> MemFiles {
        self.0.remove(path);
        self
    }

    pub fn with(mut self, path: &str, bytes: &[u8]) -> MemFiles {
        self.0.insert(path.to_string(), bytes.to_vec());
        self
    }
}

impl PackageFiles for MemFiles {
    fn exists(&self, path: &str) -> bool {
        self.0.contains_key(path)
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

pub fn ctx(source: SourceClass) -> ValidateCtx<'static> {
    ctx_with(source, &StructuralChecks)
}

pub fn ctx_with(source: SourceClass, checks: &dyn ContentChecks) -> ValidateCtx<'_> {
    ValidateCtx {
        host_api: host_api(),
        platform: None,
        source,
        node: None,
        content_checks: checks,
    }
}

pub fn check_with(
    v: &Value,
    files: &MemFiles,
    ctx: &ValidateCtx,
) -> Result<Manifest, Vec<Invalid>> {
    validate(&serde_json::to_vec(v).unwrap(), files, ctx)
}

/// Validates `v` from a registry against every fixture file.
pub fn check(v: &Value) -> Result<Manifest, Vec<Invalid>> {
    check_with(v, &MemFiles::full(), &ctx(SourceClass::Registry))
}

/// The errors' `details.reason` (or `code` when there is none).
pub fn reasons(r: Result<Manifest, Vec<Invalid>>) -> Vec<String> {
    match r {
        Ok(_) => Vec::new(),
        Err(errs) => errs
            .iter()
            .map(|e| e.reason().map_or_else(|| e.code.clone(), str::to_string))
            .collect(),
    }
}

/// `v` with `pointer` replaced by `value` (the parent must exist).
pub fn set(mut v: Value, pointer: &str, value: Value) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let p = if parent.is_empty() {
        &mut v
    } else {
        v.pointer_mut(parent).unwrap()
    };
    match p {
        Value::Object(o) => {
            o.insert(key.to_string(), value);
        }
        Value::Array(a) => a[key.parse::<usize>().unwrap()] = value,
        _ => panic!("{pointer}: parent is not a container"),
    }
    v
}

/// `v` without `pointer`.
pub fn remove(mut v: Value, pointer: &str) -> Value {
    let (parent, key) = pointer.rsplit_once('/').unwrap();
    let p = if parent.is_empty() {
        &mut v
    } else {
        v.pointer_mut(parent).unwrap()
    };
    p.as_object_mut().unwrap().remove(key);
    v
}
