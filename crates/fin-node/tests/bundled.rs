//! The compiled-in `BUNDLED_PLUGINS` list (8A plan Task 10, Decision 24): it is made from the
//! files under `resources/plugins/bundled/`, names only plugins the journal's table knows, and
//! `verify` refuses a tree that differs by a byte. The crate is a napi `cdylib` with nothing to
//! link natively, so this includes the napi-free `src/bundled.rs`.

#![allow(dead_code)]
// Reads the app's own resources/plugins/bundled folder and temporary trees by path.
#![allow(clippy::disallowed_methods)]

#[path = "../src/bundled.rs"]
mod bundled;

use std::fs;
use std::path::{Path, PathBuf};

use bundled::{BUNDLED_PLUGINS, compiled, verify};
use fin_core::events::names::bundled_plugin_name;
use fin_plugin_api::{Manifest, SourceClass, StructuralChecks, ValidateCtx, host_api, validate};
use fin_registry::package::tree_hash;
use fin_registry::plan::DirFiles;

fn resources() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/plugins/bundled")
}

fn folders() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(resources())
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    v.sort();
    v
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for e in fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let dest = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &dest);
        } else {
            fs::copy(e.path(), dest).unwrap();
        }
    }
}

#[test]
fn list_is_made_from_the_files() {
    let list = compiled();
    assert_eq!(list.len(), folders().len());
    for dir in folders() {
        let name = dir.file_name().unwrap().to_str().unwrap();
        let entry = list
            .iter()
            .find(|e| e.folder() == name)
            .unwrap_or_else(|| panic!("{name} is not in the compiled-in list"));
        assert_eq!(entry.id, format!("@finisher/{name}"));
        assert_eq!(entry.tree_hash, tree_hash(&dir).unwrap());
        let manifest: Manifest =
            serde_json::from_slice(&fs::read(dir.join("finisher-plugin.json")).unwrap()).unwrap();
        assert_eq!(manifest.id.to_string(), entry.id);
        assert_eq!(manifest.version.to_string(), entry.version);
    }
    assert_eq!(BUNDLED_PLUGINS.len(), list.len());
}

#[test]
fn every_bundled_plugin_has_a_journal_name() {
    for e in compiled() {
        assert!(
            bundled_plugin_name(&e.id).is_some(),
            "{} is missing from fin-core's BUNDLED_PLUGINS table, so the journal would hash it",
            e.id
        );
    }
}

#[test]
fn bundled_trees_validate_as_bundled_plugins() {
    for dir in folders() {
        let bytes = fs::read(dir.join("finisher-plugin.json")).unwrap();
        let ctx = ValidateCtx {
            host_api: host_api(),
            platform: None,
            source: SourceClass::Bundled,
            node: None,
            content_checks: &StructuralChecks,
        };
        validate(&bytes, &DirFiles(&dir), &ctx)
            .unwrap_or_else(|e| panic!("{} does not validate: {e:?}", dir.display()));
        // The reserved publisher is for bundled plugins only.
        let registry = ValidateCtx {
            source: SourceClass::Registry,
            ..ctx
        };
        assert!(validate(&bytes, &DirFiles(&dir), &registry).is_err());
    }
}

/// The three styles are the built-in presets (`preset:bold|clean|punch`) without their ids.
#[test]
fn captions_basic_styles_are_the_presets() {
    let dir = resources().join("captions-basic/styles");
    for name in ["bold", "clean", "punch"] {
        let file: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join(format!("{name}.json"))).unwrap()).unwrap();
        let preset = fin_core::captions::preset(&format!("preset:{name}")).unwrap();
        let mut want = serde_json::to_value(preset).unwrap();
        want.as_object_mut().unwrap().remove("id");
        assert_eq!(file, want, "{name}.json drifted from preset:{name}");
    }
}

#[test]
fn the_shipped_resources_verify() {
    let out = verify(&resources(), &compiled());
    assert!(!out.is_empty());
    assert!(out.iter().all(|v| v.ok && v.code.is_none()), "{out:?}");
}

#[test]
fn a_changed_byte_is_a_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&resources(), tmp.path());
    assert!(verify(tmp.path(), &compiled()).iter().all(|v| v.ok));
    let style = tmp.path().join("captions-basic/styles/bold.json");
    let mut bytes = fs::read(&style).unwrap();
    bytes.push(b' ');
    fs::write(&style, bytes).unwrap();
    let out = verify(tmp.path(), &compiled());
    assert_eq!(out.len(), 1);
    assert!(!out[0].ok);
    assert_eq!(out[0].code, Some("PLUGIN_BUNDLED_MISMATCH"));
    assert_eq!(out[0].reason, Some("treeHash"));
}

#[test]
fn an_added_missing_or_renamed_file_is_a_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&resources(), tmp.path());
    let root = tmp.path().join("captions-basic");
    fs::write(root.join("styles/extra.json"), b"{}").unwrap();
    assert_eq!(verify(tmp.path(), &compiled())[0].reason, Some("treeHash"));
    fs::remove_file(root.join("styles/extra.json")).unwrap();
    fs::rename(
        root.join("styles/clean.json"),
        root.join("styles/neat.json"),
    )
    .unwrap();
    assert_eq!(verify(tmp.path(), &compiled())[0].reason, Some("treeHash"));
    fs::remove_dir_all(&root).unwrap();
    let out = verify(tmp.path(), &compiled());
    assert_eq!(out[0].reason, Some("missing"));
    assert_eq!(out[0].code, Some("PLUGIN_BUNDLED_MISMATCH"));
}

#[cfg(unix)]
#[test]
fn a_symlink_in_the_tree_is_unreadable_not_ok() {
    let tmp = tempfile::tempdir().unwrap();
    copy_tree(&resources(), tmp.path());
    std::os::unix::fs::symlink(
        "/etc/passwd",
        tmp.path().join("captions-basic/styles/link.json"),
    )
    .unwrap();
    let out = verify(tmp.path(), &compiled());
    assert!(!out[0].ok);
    assert_eq!(out[0].reason, Some("unreadable"));
}
