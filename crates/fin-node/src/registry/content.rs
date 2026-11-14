//! What an installed content pack contributes, read from its store folder (8A Task 12a):
//! the font catalogs' text for the font manager and the caption styles for
//! `plugin.content.get`. Both are validated again here as the install validated them (a
//! store folder is read-only after install, but nothing here trusts that): a pack that no
//! longer validates serves nothing.

use fin_core::CoreError;
use fin_plugin_api::{Manifest, PackageFiles, PluginKind};
use fin_registry::plan::DirFiles;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use serde::Serialize;
use serde_json::{Value, json};

use super::host::{package_dir, read_manifest};
use super::{Registry, bad_request, on_worker};
use crate::{encode, reject};

/// A caption style of a pack: its name, its title (the `en` message, else the key) and the
/// `CaptionStyle` without its `id`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StyleEntry {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    style: Value,
}

/// An installed content pack's contributions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackContent {
    id: String,
    version: String,
    /// The text of each font catalog file, in manifest order.
    font_catalogs: Vec<String>,
    caption_styles: Vec<StyleEntry>,
}

fn damaged(file: &str, why: impl std::fmt::Display) -> CoreError {
    CoreError::new("PLUGIN_MANIFEST_INVALID", format!("{file}: {why}"))
        .with_details(json!({ "reason": "contentInvalid", "file": file }))
        .with_fix("remove the plugin and install it again")
}

/// The `en` message for `key`, if the pack has a catalogue with it.
fn title(m: &Manifest, files: &DirFiles<'_>, key: &str) -> String {
    let found = m.messages.as_ref().and_then(|dir| {
        let bytes = files.read(&format!("{}/en.json", dir.trim_end_matches('/')))?;
        let catalogue: Value = serde_json::from_slice(&bytes).ok()?;
        catalogue.get(key)?.as_str().map(str::to_string)
    });
    found.unwrap_or_else(|| key.to_string())
}

/// The manifest of the installed content pack at store folder `rel`, and the folder.
fn open_pack(
    base: &std::path::Path,
    rel: &str,
) -> std::result::Result<(Manifest, std::path::PathBuf), CoreError> {
    let dir = package_dir(base, rel).ok_or_else(|| {
        CoreError::new("PLUGIN_NOT_INSTALLED", "not a store folder")
            .with_details(json!({ "reason": "dir" }))
    })?;
    let m =
        read_manifest(&dir).ok_or_else(|| damaged(fin_plugin_api::MANIFEST_FILE, "unreadable"))?;
    if m.kind != PluginKind::Content {
        return Err(damaged(fin_plugin_api::MANIFEST_FILE, "not a content pack"));
    }
    Ok((m, dir))
}

/// One caption style of the pack, its file parsed and validated as at install.
fn style_entry(
    m: &Manifest,
    files: &DirFiles<'_>,
    s: &fin_plugin_api::manifest::CaptionStyleContribution,
) -> std::result::Result<StyleEntry, CoreError> {
    let bytes = files
        .read(&s.file)
        .ok_or_else(|| damaged(&s.file, "missing"))?;
    let style = fin_core::captions::parse_style_file(&bytes).map_err(|e| damaged(&s.file, e))?;
    let mut style = serde_json::to_value(&style).map_err(|e| damaged(&s.file, e))?;
    if let Some(o) = style.as_object_mut() {
        o.remove("id");
    }
    Ok(StyleEntry {
        name: s.name.clone(),
        title: s.title.as_deref().map(|k| title(m, files, k)),
        style,
    })
}

/// One caption style of a pack (`plugin.content.get`): only that style's file is parsed, so
/// serving one style does not re-read the pack's font catalogs or its other styles (review L2).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PackStyle {
    id: String,
    version: String,
    /// Every caption style name of the pack, for the answer to an unknown name.
    available: Vec<String>,
    /// The style, when the pack has one of that name.
    #[serde(skip_serializing_if = "Option::is_none")]
    found: Option<StyleEntry>,
}

fn read_style(
    base: &std::path::Path,
    rel: &str,
    name: &str,
) -> std::result::Result<PackStyle, CoreError> {
    let (m, dir) = open_pack(base, rel)?;
    let files = DirFiles(&dir);
    let found = match m.contributes.caption_styles.iter().find(|s| s.name == name) {
        Some(s) => Some(style_entry(&m, &files, s)?),
        None => None,
    };
    Ok(PackStyle {
        id: m.id.to_string(),
        version: m.version.to_string(),
        available: m
            .contributes
            .caption_styles
            .iter()
            .map(|s| s.name.clone())
            .collect(),
        found,
    })
}

fn read(base: &std::path::Path, rel: &str) -> std::result::Result<PackContent, CoreError> {
    let (m, dir) = open_pack(base, rel)?;
    let files = DirFiles(&dir);
    let mut font_catalogs = Vec::new();
    for c in &m.contributes.font_catalogs {
        let bytes = files
            .read(&c.file)
            .ok_or_else(|| damaged(&c.file, "missing"))?;
        let text = String::from_utf8(bytes).map_err(|_| damaged(&c.file, "not UTF-8"))?;
        fin_fonts::FontCatalog::from_json(&text).map_err(|e| damaged(&c.file, e.message))?;
        font_catalogs.push(text);
    }
    let mut caption_styles = Vec::new();
    for s in &m.contributes.caption_styles {
        caption_styles.push(style_entry(&m, &files, s)?);
    }
    Ok(PackContent {
        id: m.id.to_string(),
        version: m.version.to_string(),
        font_catalogs,
        caption_styles,
    })
}

#[napi]
impl Registry {
    /// The contributions of the installed content pack in `dir` (the folder as `control.db`
    /// records it, `store/@pub/name/<version>-<hash>`): `{id, version, fontCatalogs: [text],
    /// captionStyles: [{name, title?, style}]}` JSON, every file validated as at install.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn pack_content<'e>(&self, env: &'e Env, dir: String) -> Result<Object<'e>> {
        if dir.is_empty() {
            return Err(bad_request("the pack folder", "empty"));
        }
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            encode(&read(&inner.base, &dir).map_err(reject)?)
        })
    }

    /// One caption style of the installed content pack in `dir`: `{id, version, available:
    /// [name], found?: {name, title?, style}}` JSON, only that style's file read and validated.
    #[napi(catch_unwind, ts_return_type = "Promise<string>")]
    pub fn pack_style<'e>(&self, env: &'e Env, dir: String, name: String) -> Result<Object<'e>> {
        if dir.is_empty() {
            return Err(bad_request("the pack folder", "empty"));
        }
        let inner = self.inner.clone();
        on_worker(env, &self.worker, move || {
            encode(&read_style(&inner.base, &dir, &name).map_err(reject)?)
        })
    }
}
