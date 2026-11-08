//! What plans and validation need from this binary: content checks over `fin-fonts`, the
//! catalog ids already taken (the built-in catalog and installed packs), and the running
//! host's API, platform and Node version.

// Reads an installed plugin's manifest from the plugin store; never a library path.
#![allow(clippy::disallowed_methods)]

use std::collections::BTreeSet;
use std::path::Path;

use fin_fonts::FontCatalog;
use fin_plugin_api::{
    ContentChecks, Manifest, Platform, PluginId, PluginKind, SourceClass, ValidateCtx, host_api,
};
use fin_registry::Store;
use fin_registry::plan::{DirFiles, PlanHost};

/// Font catalogs are parsed and validated as `fin-fonts` reads them; a caption style is a
/// JSON object until the 8A Task 12a consumer fixes its shape.
pub(crate) struct Checks;

impl ContentChecks for Checks {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String> {
        let text = std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
        FontCatalog::from_json(text)
            .map(|_| ())
            .map_err(|e| e.message)
    }

    fn caption_style(&self, bytes: &[u8]) -> Result<(), String> {
        fin_plugin_api::StructuralChecks.caption_style(bytes)
    }
}

fn catalog_ids(bytes: &[u8]) -> Vec<String> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|t| FontCatalog::from_json(t).ok())
        .map(|c| c.entries.into_iter().map(|e| e.id).collect())
        .unwrap_or_default()
}

/// The [`PlanHost`] of a server's store.
pub(crate) struct Host<'a> {
    pub(crate) store: &'a Store,
}

impl PlanHost for Host<'_> {
    fn font_catalog_ids(&self, bytes: &[u8]) -> Vec<String> {
        catalog_ids(bytes)
    }

    fn taken_font_ids(&self, except: &PluginId) -> BTreeSet<String> {
        let mut taken: BTreeSet<String> = FontCatalog::builtin()
            .entries
            .iter()
            .map(|e| e.id.clone())
            .collect();
        for (dir, m) in self.store.installed_manifests() {
            if m.id == *except || m.kind != PluginKind::Content {
                continue;
            }
            taken.extend(pack_catalog_ids(&dir, &m));
        }
        taken
    }
}

fn pack_catalog_ids(dir: &Path, m: &Manifest) -> Vec<String> {
    use fin_plugin_api::PackageFiles;
    let files = DirFiles(dir);
    m.contributes
        .font_catalogs
        .iter()
        .filter_map(|c| files.read(&c.file))
        .flat_map(|b| catalog_ids(&b))
        .collect()
}

/// The platform this binary runs on, if it is one plugins can target.
pub(crate) fn platform() -> Option<Platform> {
    let s = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x64",
        ("linux", "x86_64") => "linux-x64",
        ("linux", "aarch64") => "linux-arm64",
        ("windows", "x86_64") => "win32-x64",
        ("windows", "aarch64") => "win32-arm64",
        _ => return None,
    };
    serde_json::from_value(serde_json::Value::String(s.into())).ok()
}

pub(crate) fn ctx(source: SourceClass, node: Option<semver::Version>) -> ValidateCtx<'static> {
    ValidateCtx {
        host_api: host_api(),
        platform: platform(),
        source,
        node,
        content_checks: &Checks,
    }
}

/// The entry ids the staged plan `plan_id`'s font catalogs share with the built-in catalog and
/// the packs in `installed` (package directories relative to `base`, as `control.db` records
/// them), other versions of the plan's own plugin aside: the commit's re-check of the
/// plan-time `catalogIdTaken` rule (8A Task 9b, review M2).
pub(crate) fn staged_catalog_clash(
    store: &Store,
    plan_id: &str,
    installed: &[String],
) -> Vec<String> {
    use fin_plugin_api::PackageFiles;
    let pkg = store.staging_dir().join(plan_id).join("pkg");
    let Some(m) = read_manifest(&pkg) else {
        return Vec::new();
    };
    let files = DirFiles(&pkg);
    let mine: BTreeSet<String> = m
        .contributes
        .font_catalogs
        .iter()
        .filter_map(|c| files.read(&c.file))
        .flat_map(|b| catalog_ids(&b))
        .collect();
    if mine.is_empty() {
        return Vec::new();
    }
    let mut taken: BTreeSet<String> = FontCatalog::builtin()
        .entries
        .iter()
        .map(|e| e.id.clone())
        .collect();
    for rel in installed {
        let Some(dir) = package_dir(store.base(), rel) else {
            continue;
        };
        match read_manifest(&dir) {
            Some(other) if other.id != m.id && other.kind == PluginKind::Content => {
                taken.extend(pack_catalog_ids(&dir, &other));
            }
            _ => {}
        }
    }
    mine.intersection(&taken).cloned().collect()
}

fn read_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(dir.join(fin_plugin_api::MANIFEST_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `base/<rel>` for a `store/…` package directory, `/`-separated with no `.` or `..` part.
fn package_dir(base: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let mut parts = rel.split('/');
    if parts.next() != Some("store") {
        return None;
    }
    let mut p = base.join("store");
    for part in parts {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
            return None;
        }
        p.push(part);
    }
    Some(p)
}
