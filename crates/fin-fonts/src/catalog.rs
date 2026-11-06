//! The font catalog (4C Decision 30): `catalog/catalog.json`, a versioned manifest of font
//! files pinned by SHA-256 at immutable upstream URLs. A newer `catalogVersion` adds files;
//! it never changes a project's pins (they name exact bytes).

use std::sync::OnceLock;

use fin_core::{CoreError, FontFormat};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The built-in catalog's source.
const BUILTIN: &str = include_str!("../catalog/catalog.json");

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FontCatalog {
    /// `YYYY.MM.N`, e.g. `2026.11.1`.
    pub catalog_version: String,
    /// Entry ids, in the order a text's uncovered scripts look for a fallback.
    pub fallback: Vec<String>,
    pub entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogEntry {
    /// `[a-z0-9-]`, unique in the catalog.
    pub id: String,
    pub family: String,
    pub licence: Licence,
    pub homepage: String,
    /// The ISO 15924 scripts the entry is offered for (a fallback is chosen by these); each
    /// style's default face covers them.
    pub scripts: Vec<String>,
    pub styles: Vec<CatalogStyle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Licence {
    /// An SPDX id from the permissive allow list (`catalog/rules.json`).
    pub spdx: String,
    /// The licence text at a pinned commit or tag.
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogStyle {
    /// `[a-z0-9-]`, unique in its entry (`regular`, `bold`).
    pub id: String,
    pub weight: u16,
    pub italic: bool,
    /// The face a fallback or an entry-level pin uses (a collection holds several).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub default_face: u32,
    pub file: CatalogFile,
    /// Tried in order: the upstream first, then mirrors serving the same bytes.
    pub sources: Vec<CatalogSource>,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogFile {
    /// Lowercase hex SHA-256 of the whole file: its cache name and its pin.
    pub sha256: String,
    /// Bytes; a download must match it exactly.
    pub size: u64,
    pub format: FontFormat,
    /// Every face, as `fin_text::font_info` reads it (`warm_cache` checks).
    pub faces: Vec<CatalogFace>,
    /// A variable font's `wght` axis. Only a variable file may serve several styles (one per
    /// weight in the range; the request's weight drives the axis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variable: Option<Variable>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Variable {
    /// The `fvar` `wght` axis' `[min, max]` (`warm_cache` checks).
    pub wght: [u16; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogFace {
    pub index: u32,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// ISO 15924 coverage by `fin_text::font_info`'s rule, sorted.
    pub scripts: Vec<String>,
}

/// Where a file can be fetched. A plain source serves the file itself; an archive source
/// serves an archive (its own `sha256` and `size`) holding the file as `archive.member`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CatalogSource {
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArchiveMember {
    pub format: ArchiveFormat,
    /// The member's path inside the archive.
    pub member: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveFormat {
    Zip,
}

impl FontCatalog {
    /// The catalog built into this binary (`catalog/catalog.json`).
    pub fn builtin() -> &'static FontCatalog {
        static CATALOG: OnceLock<FontCatalog> = OnceLock::new();
        CATALOG.get_or_init(|| {
            FontCatalog::from_json(BUILTIN).expect("the built-in font catalog validates")
        })
    }

    /// The catalog this server uses: the built-in one; in `test-hooks` builds the file
    /// `FIN_FONT_CATALOG` names instead, when set.
    pub fn active() -> Result<FontCatalog, CoreError> {
        #[cfg(feature = "test-hooks")]
        if let Some(path) = std::env::var_os("FIN_FONT_CATALOG") {
            let text = std::fs::read_to_string(&path).map_err(|e| {
                crate::validate::invalid("schema", "", format!("{}: {e}", path.to_string_lossy()))
            })?;
            return FontCatalog::from_json(&text);
        }
        Ok(FontCatalog::builtin().clone())
    }

    /// Parses and validates a catalog (Decision 31). `FONT_CATALOG_INVALID` with
    /// `details {rule, path}` names the first rule a catalog breaks.
    pub fn from_json(s: &str) -> Result<FontCatalog, CoreError> {
        let schema = |e: serde_json::Error| {
            crate::validate::invalid("schema", "", format!("not a font catalog: {e}"))
        };
        let value: serde_json::Value = serde_json::from_str(s).map_err(schema)?;
        crate::validate::check_formats(&value)?;
        let catalog: FontCatalog = serde_json::from_value(value).map_err(schema)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// The face for a family, weight and italic: a face of that family name with that
    /// weight (a variable face whose style has that weight counts), else the default face of
    /// the style with that weight and italic in the entry of that family (`"Inter"` finds
    /// the `Inter Variable` face; `"Noto Sans CJK"` its SC face).
    pub fn find_face(
        &self,
        family: &str,
        weight: u16,
        italic: bool,
    ) -> Option<(&CatalogEntry, &CatalogStyle, &CatalogFace)> {
        let by_face = self.styles().find_map(|(e, s)| {
            // A variable file serves one style per weight: the style's weight decides.
            let at = |f: &CatalogFace| match s.file.variable {
                Some(_) => s.weight == weight,
                None => f.weight == weight,
            };
            s.file
                .faces
                .iter()
                .find(|f| f.family == family && f.italic == italic && at(f))
                .map(|f| (e, s, f))
        });
        by_face.or_else(|| {
            self.styles()
                .find(|(e, s)| e.family == family && s.weight == weight && s.italic == italic)
                .map(|(e, s)| (e, s, s.face()))
        })
    }

    /// The style whose file has this SHA-256.
    pub fn by_sha(&self, sha256: &str) -> Option<(&CatalogEntry, &CatalogStyle)> {
        self.styles().find(|(_, s)| s.file.sha256 == sha256)
    }

    /// The first `fallback` entry offered for `script` (ISO 15924), with its regular style
    /// (weight 400, upright) or else its first.
    pub fn fallback_for(&self, script: &str) -> Option<(&CatalogEntry, &CatalogStyle)> {
        self.fallback
            .iter()
            .filter_map(|id| self.entry(id))
            .find(|e| e.scripts.iter().any(|s| s == script))
            .and_then(|e| e.regular().map(|s| (e, s)))
    }

    pub fn entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Every style with its entry, in catalog order.
    pub fn styles(&self) -> impl Iterator<Item = (&CatalogEntry, &CatalogStyle)> {
        self.entries
            .iter()
            .flat_map(|e| e.styles.iter().map(move |s| (e, s)))
    }

    /// The styles whose files an archive with this SHA-256 holds, with their members.
    pub(crate) fn archive_members(
        &self,
        archive_sha256: &str,
    ) -> Vec<(&CatalogStyle, &ArchiveMember)> {
        self.styles()
            .flat_map(|(_, s)| {
                s.sources.iter().filter_map(move |src| match &src.archive {
                    Some(m) if src.sha256.as_deref() == Some(archive_sha256) => Some((s, m)),
                    _ => None,
                })
            })
            .collect()
    }
}

impl CatalogEntry {
    /// Weight 400 upright, else the first style.
    pub fn regular(&self) -> Option<&CatalogStyle> {
        self.styles
            .iter()
            .find(|s| s.weight == 400 && !s.italic)
            .or_else(|| self.styles.first())
    }

    pub fn style(&self, id: &str) -> Option<&CatalogStyle> {
        self.styles.iter().find(|s| s.id == id)
    }
}

impl CatalogStyle {
    /// The face at `default_face` (validation guarantees it exists).
    pub fn face(&self) -> &CatalogFace {
        self.file
            .faces
            .iter()
            .find(|f| f.index == self.default_face)
            .expect("validated: default_face is one of the file's faces")
    }
}
