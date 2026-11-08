//! A local-folder registry for tests (feature `test-hooks`, never shipped): built and signed
//! by `fin-registry`'s editor, with content packs published under delegated publishers.

#![cfg_attr(test, allow(dead_code))]

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fin_core::CoreError;
use fin_registry::editor::{RepoBuilder, SigningKey};
use fin_registry::tuf::{FixedClock, sha256_hex};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::json;

use crate::{encode, reject};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Spec {
    /// `@publisher/name@version` content packs to publish.
    #[serde(default)]
    packages: Vec<String>,
    /// `custom.finisher.maxStaleDays` in the signed root.
    #[serde(default)]
    max_stale_days: Option<u16>,
    /// Sign everything as of this many days ago (so the metadata is that stale now).
    #[serde(default)]
    age_days: u32,
}

fn fail(what: &str, e: impl std::fmt::Display) -> Error {
    reject(CoreError::new("INVALID_OP", format!("{what}: {e}")))
}

/// Builds a registry in `dir` (keys in `keys_dir`, outside it) from the `Spec` JSON; resolves
/// to `{rootSha256, rootFile}` of its `1.root.json`.
#[napi(catch_unwind)]
pub fn build_registry_for_test(dir: String, keys_dir: String, spec_json: String) -> Result<String> {
    let spec: Spec = serde_json::from_str(&spec_json).map_err(|e| fail("the spec", e))?;
    let at = SystemTime::now() - Duration::from_secs(u64::from(spec.age_days) * 86_400);
    let root_key = SigningKey::generate();
    let mut b = RepoBuilder::init_with(
        Path::new(&dir),
        Path::new(&keys_dir),
        std::slice::from_ref(&root_key),
        1,
        spec.max_stale_days,
        Arc::new(FixedClock::new(at)),
    )
    .map_err(|e| fail("the registry", e))?;
    let scratch = Path::new(&keys_dir).join("pkgs");
    let mut publishers: std::collections::BTreeMap<String, SigningKey> = Default::default();
    for p in &spec.packages {
        let (id, version) = p.rsplit_once('@').ok_or_else(|| fail("a package", p))?;
        let publisher = id
            .strip_prefix('@')
            .and_then(|r| r.split('/').next())
            .ok_or_else(|| fail("a package id", id))?
            .to_string();
        if !publishers.contains_key(&publisher) {
            let k = SigningKey::generate();
            b.delegate(&publisher, &k.public())
                .map_err(|e| fail("a publisher", e))?;
            publishers.insert(publisher.clone(), k);
        }
        let src = scratch.join(format!("{}-{version}", id.replace(['@', '/'], "_")));
        std::fs::create_dir_all(src.join("styles")).map_err(|e| fail("a package", e))?;
        let manifest = json!({
            "manifestVersion": 1, "id": id, "version": version, "kind": "content",
            "hostApi": "^1.0", "license": "MIT", "publisher": { "name": "Test" },
            "contributes": { "captionStyles": [{ "name": "bold", "file": "styles/bold.json" }] },
        });
        std::fs::write(src.join("finisher-plugin.json"), manifest.to_string())
            .and_then(|()| {
                std::fs::write(
                    src.join("styles/bold.json"),
                    format!("{{\"v\": \"{version}\"}}"),
                )
            })
            .map_err(|e| fail("a package", e))?;
        let archive = scratch.join(format!("{}-{version}.finpkg", id.replace(['@', '/'], "_")));
        fin_registry::package::pack(&src, &[], &archive).map_err(|e| fail("a package", e))?;
        b.publish(&archive, &publishers[&publisher])
            .map_err(|e| fail("a package", e))?;
    }
    let root = b.first_root_bytes().map_err(|e| fail("the root", e))?;
    encode(&json!({
        "rootSha256": sha256_hex(&root),
        "rootFile": Path::new(&dir).join("metadata/1.root.json").to_string_lossy(),
    }))
}
