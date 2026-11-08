//! The content checks a publisher's `validate`, `pack` and `registry publish` run (8A Task
//! 12c): the host's own. A caption style is `fin_core::captions::parse_style_file`'s
//! `CaptionStyle` without `id`; a font catalog is `FontCatalog::from_json`'s (licence allow
//! list, https and immutable source URLs, source hosts, size and SHA-256 pins); and the pack's
//! catalogs must merge, one after the other, into the built-in catalog, as the server merges
//! them (no entry id it has, no file or face listed twice). Another installed pack is the
//! server's to check when the pack is planned.

use fin_fonts::FontCatalog;
use fin_plugin_api::{ContentChecks, Invalid, Manifest, PackageFiles};
use serde_json::{Map, json};

pub struct HostChecks;

fn parse_catalog(bytes: &[u8]) -> Result<FontCatalog, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("not UTF-8: {e}"))?;
    FontCatalog::from_json(text).map_err(|e| e.message)
}

impl ContentChecks for HostChecks {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String> {
        parse_catalog(bytes).map(|_| ())
    }

    fn caption_style(&self, bytes: &[u8]) -> Result<(), String> {
        fin_core::captions::parse_style_file(bytes).map(|_| ())
    }
}

/// `PLUGIN_MANIFEST_INVALID {path, reason: catalogIdTaken|catalogMerge, rule, entry?, file}`
/// for the first of the pack's (valid) font catalogs that does not merge.
pub fn catalog_merge(m: &Manifest, files: &dyn PackageFiles) -> Option<Invalid> {
    let parsed: Vec<(usize, &str, FontCatalog)> = m
        .contributes
        .font_catalogs
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let catalog = parse_catalog(&files.read(&c.file)?).ok()?;
            Some((i, c.file.as_str(), catalog))
        })
        .collect();
    let catalogs: Vec<FontCatalog> = parsed.iter().map(|(_, _, c)| c.clone()).collect();
    let (k, e) = FontCatalog::builtin().first_unmergeable(&catalogs)?;
    let (index, file, _) = &parsed[k];
    let rule = e.details.get("rule").cloned().unwrap_or(json!("invalid"));
    let entry = e.details.get("id").cloned();
    let reason = if rule == "catalogIdTaken" {
        "catalogIdTaken"
    } else {
        "catalogMerge"
    };
    let mut details = Map::new();
    details.insert(
        "path".into(),
        json!(format!("/contributes/fontCatalogs/{index}/file")),
    );
    details.insert("reason".into(), json!(reason));
    details.insert("rule".into(), rule);
    if let Some(entry) = entry {
        details.insert("entry".into(), entry);
    }
    details.insert("file".into(), json!(file));
    Some(Invalid {
        code: "PLUGIN_MANIFEST_INVALID".into(),
        message: format!(
            "{file}: does not merge into the font catalog: {}",
            e.message
        ),
        details,
    })
}
