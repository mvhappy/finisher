//! What plans and validation need from this binary: content checks over `fin-fonts`, whether a
//! pack's font catalogs merge with the built-in catalog and the installed packs' (as content
//! sync merges them), and the running host's API, platform and Node version.

// Reads an installed plugin's manifest from the plugin store; never a library path.
#![allow(clippy::disallowed_methods)]

use std::path::Path;

use fin_fonts::FontCatalog;
use fin_plugin_api::{
    ContentChecks, Manifest, Platform, PluginId, PluginKind, SourceClass, ValidateCtx, host_api,
};
use fin_registry::Store;
use fin_registry::plan::{CatalogClash, DirFiles, PlanHost};

/// Font catalogs are parsed and validated as `fin-fonts` reads them (the built-in catalog's
/// rules: licences, https and immutable source URLs, source hosts, SHA-256 pins); a caption
/// style is a `CaptionStyle` without its `id`, checked by `fin-core` as `style.add` checks
/// the same values.
pub(crate) struct Checks;

impl ContentChecks for Checks {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String> {
        let text = std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
        FontCatalog::from_json(text)
            .map(|_| ())
            .map_err(|e| e.message)
    }

    fn caption_style(&self, bytes: &[u8]) -> Result<(), String> {
        fin_core::captions::parse_style_file(bytes)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn parse_catalog(bytes: &[u8]) -> Option<FontCatalog> {
    let text = std::str::from_utf8(bytes).ok()?;
    FontCatalog::from_json(text).ok()
}

/// The font catalogs of the installed pack `m` in `dir`, in manifest order.
fn pack_catalogs(dir: &Path, m: &Manifest) -> Vec<FontCatalog> {
    use fin_plugin_api::PackageFiles;
    let files = DirFiles(dir);
    m.contributes
        .font_catalogs
        .iter()
        .filter_map(|c| files.read(&c.file))
        .filter_map(|b| parse_catalog(&b))
        .collect()
}

/// The built-in catalog with `packs` merged in id order, a pack that does not merge left out
/// whole, as `plugins/content.ts` hands them to the font manager.
fn installed_catalog(mut packs: Vec<(PluginId, Vec<FontCatalog>)>) -> FontCatalog {
    packs.sort_by_key(|p| p.0.to_string());
    let mut catalog = FontCatalog::builtin().clone();
    for (_, catalogs) in packs.iter().filter(|(_, c)| !c.is_empty()) {
        if let Ok(merged) = catalog.merged(catalogs) {
            catalog = merged;
        }
    }
    catalog
}

/// The first of `mine` that does not merge into `base` after the ones before it.
pub(crate) fn clash(base: &FontCatalog, mine: &[Vec<u8>]) -> Option<CatalogClash> {
    // Validated one by one already (`Checks::font_catalog`); a file that is not is the
    // manifest check's to report, so it is skipped here (and indexes follow the manifest).
    let parsed: Vec<(usize, FontCatalog)> = mine
        .iter()
        .enumerate()
        .filter_map(|(i, b)| parse_catalog(b).map(|c| (i, c)))
        .collect();
    let catalogs: Vec<FontCatalog> = parsed.iter().map(|(_, c)| c.clone()).collect();
    let (k, e) = base.first_unmergeable(&catalogs)?;
    let detail = |key: &str| {
        e.details
            .get(key)
            .and_then(|v| v.as_str())
            .map(String::from)
    };
    Some(CatalogClash {
        index: parsed[k].0,
        rule: detail("rule").unwrap_or_else(|| "invalid".into()),
        entry: detail("id"),
        message: e.message,
    })
}

/// The [`PlanHost`] of a server's store.
pub(crate) struct Host<'a> {
    pub(crate) store: &'a Store,
}

impl PlanHost for Host<'_> {
    fn catalog_clash(&self, catalogs: &[Vec<u8>], except: &PluginId) -> Option<CatalogClash> {
        let others = self
            .store
            .installed_manifests()
            .into_iter()
            .filter(|(_, m)| m.id != *except && m.kind == PluginKind::Content)
            .map(|(dir, m)| (m.id.clone(), pack_catalogs(&dir, &m)))
            .collect();
        clash(&installed_catalog(others), catalogs)
    }
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

/// The first of the staged plan `plan_id`'s font catalogs that no longer merges with the
/// built-in catalog and the packs in `installed` (package directories relative to `base`, as
/// `control.db` records them), other versions of the plan's own plugin aside: the commit's
/// re-check of the plan-time rule (8A Task 9b, review M2; Task 12c).
pub(crate) fn staged_catalog_clash(
    store: &Store,
    plan_id: &str,
    installed: &[String],
) -> Option<CatalogClash> {
    use fin_plugin_api::PackageFiles;
    let pkg = store.staging_dir().join(plan_id).join("pkg");
    let m = read_manifest(&pkg)?;
    let files = DirFiles(&pkg);
    let mine: Vec<Vec<u8>> = m
        .contributes
        .font_catalogs
        .iter()
        .filter_map(|c| files.read(&c.file))
        .collect();
    if mine.is_empty() {
        return None;
    }
    let others = installed
        .iter()
        .filter_map(|rel| package_dir(store.base(), rel))
        .filter_map(|dir| read_manifest(&dir).map(|other| (dir, other)))
        .filter(|(_, other)| other.id != m.id && other.kind == PluginKind::Content)
        .map(|(dir, other)| (other.id.clone(), pack_catalogs(&dir, &other)))
        .collect();
    clash(&installed_catalog(others), &mine)
}

pub(super) fn read_manifest(dir: &Path) -> Option<Manifest> {
    let bytes = std::fs::read(dir.join(fin_plugin_api::MANIFEST_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// `base/<rel>` for a `store/…` package directory, `/`-separated with no `.` or `..` part.
pub(super) fn package_dir(base: &Path, rel: &str) -> Option<std::path::PathBuf> {
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
