//! Local-folder registries for tests (feature `test-hooks`, never shipped): built and signed by
//! `fin-registry`'s editor, with packages published under delegated publishers. A registry can
//! be published to again (`publish_to_registry_for_test`): the publishers' keys are kept in the
//! keys folder (`publisher-<name>.pem`), outside the registry.

#![cfg_attr(test, allow(dead_code))]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use fin_core::CoreError;
use fin_registry::advisory::Advisory;
use fin_registry::editor::{RepoBuilder, SigningKey};
use fin_registry::tuf::{FixedClock, sha256_hex};
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{encode, reject};

/// A package to publish: `"@publisher/name@version"` (a content pack with one caption style)
/// or the object form.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum PackageSpec {
    Short(String),
    Full(FullPackage),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FullPackage {
    id: String,
    version: String,
    /// `content` (default) or `code` (a `node` runtime on every platform).
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    permissions: Vec<Value>,
    #[serde(default)]
    license: Option<String>,
    /// Font catalog entry ids (each a copy of the built-in catalog's first entry): a pack with
    /// a font catalog instead of a caption style.
    #[serde(default)]
    fonts: Vec<String>,
    /// Caption style names: each `styles/<name>.json` is the built-in `clean` style named
    /// `<name> <version>`, so versions differ in bytes. Replaces the default `bold` style.
    #[serde(default)]
    styles: Vec<String>,
    /// Files written as given (a path in the package → its text); with `contributes`, a pack
    /// whose content the test states itself.
    #[serde(default)]
    files: BTreeMap<String, String>,
    /// The manifest's `contributes`, instead of the generated one.
    #[serde(default)]
    contributes: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Spec {
    #[serde(default)]
    packages: Vec<PackageSpec>,
    /// Advisories, each signed by its plugin's publisher.
    #[serde(default)]
    advisories: Vec<Advisory>,
    /// `@publisher/name@version` to mark yanked.
    #[serde(default)]
    yanks: Vec<String>,
    /// `custom.finisher.maxStaleDays` in the signed root (new registries only).
    #[serde(default)]
    max_stale_days: Option<u16>,
    /// Sign everything as of this many days ago (so the metadata is that stale now).
    #[serde(default)]
    age_days: u32,
}

fn fail(what: &str, e: impl std::fmt::Display) -> Error {
    reject(CoreError::new("INVALID_OP", format!("{what}: {e}")))
}

fn publisher_of(id: &str) -> Result<String> {
    id.strip_prefix('@')
        .and_then(|r| r.split('/').next())
        .map(str::to_string)
        .ok_or_else(|| fail("a package id", id))
}

fn split_at_version(p: &str) -> Result<(&str, &str)> {
    p.rsplit_once('@').ok_or_else(|| fail("a package", p))
}

fn full(p: &PackageSpec) -> Result<FullPackage> {
    Ok(match p {
        PackageSpec::Short(s) => {
            let (id, version) = split_at_version(s)?;
            FullPackage {
                id: id.into(),
                version: version.into(),
                kind: None,
                permissions: Vec::new(),
                license: None,
                fonts: Vec::new(),
                styles: Vec::new(),
                files: BTreeMap::new(),
                contributes: None,
            }
        }
        PackageSpec::Full(f) => FullPackage {
            id: f.id.clone(),
            version: f.version.clone(),
            kind: f.kind.clone(),
            permissions: f.permissions.clone(),
            license: f.license.clone(),
            fonts: f.fonts.clone(),
            styles: f.styles.clone(),
            files: f.files.clone(),
            contributes: f.contributes.clone(),
        },
    })
}

/// A font catalog of copies of the built-in catalog's first entry, one per id.
fn font_catalog(ids: &[String]) -> Result<Vec<u8>> {
    let builtin = fin_fonts::FontCatalog::builtin();
    let first = builtin
        .entries
        .first()
        .ok_or_else(|| fail("the built-in catalog", "empty"))?;
    let entries: Vec<_> = ids
        .iter()
        .map(|id| {
            let mut e = first.clone();
            e.id = id.clone();
            e
        })
        .collect();
    let catalog = fin_fonts::FontCatalog {
        catalog_version: builtin.catalog_version.clone(),
        fallback: Vec::new(),
        entries,
    };
    serde_json::to_vec(&catalog).map_err(|e| fail("a font catalog", e))
}

/// A caption style file: the built-in `clean` style named after the style and the version.
fn style_file(name: &str, version: &str) -> Result<Vec<u8>> {
    let clean = fin_core::captions::preset("preset:clean")
        .ok_or_else(|| fail("the clean preset", "missing"))?;
    let mut v = serde_json::to_value(clean).map_err(|e| fail("a caption style", e))?;
    if let Some(o) = v.as_object_mut() {
        o.remove("id");
        o.insert("name".into(), json!(format!("{name} {version}")));
    }
    serde_json::to_vec(&v).map_err(|e| fail("a caption style", e))
}

/// Writes `p`'s unpacked tree under `scratch` and packs it; returns the archive.
fn pack(p: &FullPackage, scratch: &Path) -> Result<std::path::PathBuf> {
    let stem = format!("{}-{}", p.id.replace(['@', '/'], "_"), p.version);
    let src = scratch.join(&stem);
    let _ = std::fs::remove_dir_all(&src);
    let write = |rel: &str, bytes: &[u8]| -> Result<()> {
        let f = src.join(rel);
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d).map_err(|e| fail("a package", e))?;
        }
        std::fs::write(&f, bytes).map_err(|e| fail("a package", e))
    };
    let mut manifest = json!({
        "manifestVersion": 1, "id": p.id, "version": p.version, "kind": "content",
        "hostApi": "^1.0", "license": p.license.as_deref().unwrap_or("MIT"),
        "publisher": { "name": "Test" },
    });
    if !p.permissions.is_empty() {
        manifest["permissions"] = Value::Array(p.permissions.clone());
    }
    if p.kind.as_deref() == Some("code") {
        manifest["kind"] = json!("code");
        manifest["platforms"] = json!(fin_plugin_api::Platform::ALL);
        manifest["runtime"] =
            json!({ "type": "node", "main": "dist/plugin.mjs", "node": ">=24.0" });
        write("dist/plugin.mjs", b"export {};\n")?;
    } else if let Some(contributes) = &p.contributes {
        manifest["contributes"] = contributes.clone();
    } else {
        let mut contributes = serde_json::Map::new();
        if !p.fonts.is_empty() {
            contributes.insert(
                "fontCatalogs".into(),
                json!([{ "file": "fonts/catalog.json" }]),
            );
            write("fonts/catalog.json", &font_catalog(&p.fonts)?)?;
        }
        let styles: Vec<String> = match (p.styles.is_empty(), p.fonts.is_empty()) {
            (false, _) => p.styles.clone(),
            (true, true) => vec!["bold".into()],
            (true, false) => Vec::new(),
        };
        if !styles.is_empty() {
            let mut list = Vec::new();
            for name in &styles {
                let file = format!("styles/{name}.json");
                write(&file, &style_file(name, &p.version)?)?;
                list.push(json!({ "name": name, "file": file }));
            }
            contributes.insert("captionStyles".into(), Value::Array(list));
        }
        manifest["contributes"] = Value::Object(contributes);
    }
    for (path, text) in &p.files {
        write(path, text.as_bytes())?;
    }
    write(
        fin_plugin_api::MANIFEST_FILE,
        manifest.to_string().as_bytes(),
    )?;
    let archive = scratch.join(format!("{stem}.finpkg"));
    let executables = serde_json::from_value::<fin_plugin_api::Manifest>(manifest)
        .map(|m| m.executables())
        .unwrap_or_default();
    fin_registry::package::pack(&src, &executables, &archive).map_err(|e| fail("a package", e))?;
    Ok(archive)
}

/// The key of `publisher`, kept in `keys_dir`; a new publisher is generated and delegated.
fn publisher_key(
    b: &mut RepoBuilder,
    keys_dir: &Path,
    publisher: &str,
    keys: &mut BTreeMap<String, SigningKey>,
) -> Result<SigningKey> {
    if let Some(k) = keys.get(publisher) {
        return Ok(k.clone());
    }
    let path = keys_dir.join(format!("publisher-{publisher}.pem"));
    let k = if path.exists() {
        SigningKey::read(&path).map_err(|e| fail("a publisher key", e))?
    } else {
        let k = SigningKey::generate();
        b.delegate(publisher, &k.public())
            .map_err(|e| fail("a publisher", e))?;
        k.write(&path).map_err(|e| fail("a publisher key", e))?;
        k
    };
    keys.insert(publisher.into(), k.clone());
    Ok(k)
}

fn apply(b: &mut RepoBuilder, keys_dir: &Path, spec: &Spec) -> Result<()> {
    let scratch = keys_dir.join("pkgs");
    let mut keys = BTreeMap::new();
    for p in &spec.packages {
        let p = full(p)?;
        let k = publisher_key(b, keys_dir, &publisher_of(&p.id)?, &mut keys)?;
        let archive = pack(&p, &scratch)?;
        b.publish(&archive, &k).map_err(|e| fail("a package", e))?;
    }
    for y in &spec.yanks {
        let (id, version) = split_at_version(y)?;
        let v = semver::Version::parse(version).map_err(|e| fail("a version", e))?;
        let k = publisher_key(b, keys_dir, &publisher_of(id)?, &mut keys)?;
        b.yank(id, &v, &k).map_err(|e| fail("a yank", e))?;
    }
    for a in &spec.advisories {
        let k = publisher_key(b, keys_dir, &publisher_of(&a.plugin)?, &mut keys)?;
        b.advise(a.clone(), &k)
            .map_err(|e| fail("an advisory", e))?;
    }
    Ok(())
}

fn clock(age_days: u32) -> Arc<FixedClock> {
    let at = SystemTime::now() - Duration::from_secs(u64::from(age_days) * 86_400);
    Arc::new(FixedClock::new(at))
}

/// Builds a registry in `dir` (keys in `keys_dir`, outside it) from the `Spec` JSON; resolves
/// to `{rootSha256, rootFile}` of its `1.root.json`.
#[napi(catch_unwind)]
pub fn build_registry_for_test(dir: String, keys_dir: String, spec_json: String) -> Result<String> {
    let spec: Spec = serde_json::from_str(&spec_json).map_err(|e| fail("the spec", e))?;
    let root_key = SigningKey::generate();
    let mut b = RepoBuilder::init_with(
        Path::new(&dir),
        Path::new(&keys_dir),
        std::slice::from_ref(&root_key),
        1,
        spec.max_stale_days,
        clock(spec.age_days),
    )
    .map_err(|e| fail("the registry", e))?;
    apply(&mut b, Path::new(&keys_dir), &spec)?;
    let root = b.first_root_bytes().map_err(|e| fail("the root", e))?;
    encode(&json!({
        "rootSha256": sha256_hex(&root),
        "rootFile": Path::new(&dir).join("metadata/1.root.json").to_string_lossy(),
    }))
}

/// Publishes the `Spec`'s packages, yanks and advisories to the registry `build_registry_for_test`
/// built in `dir` with `keys_dir` (`maxStaleDays` is ignored).
#[napi(catch_unwind)]
pub fn publish_to_registry_for_test(
    dir: String,
    keys_dir: String,
    spec_json: String,
) -> Result<()> {
    let spec: Spec = serde_json::from_str(&spec_json).map_err(|e| fail("the spec", e))?;
    let mut b = RepoBuilder::open(Path::new(&dir), Path::new(&keys_dir))
        .map_err(|e| fail("the registry", e))?
        .with_clock(clock(spec.age_days));
    apply(&mut b, Path::new(&keys_dir), &spec)
}

/// The tree hash of the files under `dir` (`fin_registry::package::tree_hash`), the function
/// `build.rs` makes `BUNDLED_PLUGINS` with: for tests that stand in their own bundled list.
#[napi(catch_unwind)]
pub fn tree_hash_for_test(dir: String) -> Result<String> {
    fin_registry::package::tree_hash(Path::new(&dir)).map_err(|e| fail("the tree", e))
}
