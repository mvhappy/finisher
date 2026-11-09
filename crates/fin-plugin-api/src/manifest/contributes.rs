//! `contributes` (spec §4.2). Only the kinds this crate types are parsed; any other key is
//! kept by name so validation can refuse it as `PLUGIN_CONTRIBUTION_UNSUPPORTED` (8A
//! Decision 4) instead of failing the parse, and no unconsumed kind is half-validated.

use std::borrow::Cow;
use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::de::Error as _;
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::contribution::ContributionKind;

/// A font catalog file (`fin_fonts::FontCatalog` JSON) whose entries join the server's
/// catalog by entry id (8A Decision 22).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FontCatalogContribution {
    pub file: String,
}

/// A caption style (`fin_core::CaptionStyle` JSON) that `fin style add --from
/// @pub/pack:<name>` copies into a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CaptionStyleContribution {
    /// `[a-z0-9][a-z0-9-]{0,63}`, unique in the pack.
    pub name: String,
    pub file: String,
    /// A message key in the `en` catalogue for the style's display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// The typed contributions, plus the names of keys this crate does not type.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contributes {
    pub font_catalogs: Vec<FontCatalogContribution>,
    pub caption_styles: Vec<CaptionStyleContribution>,
    unsupported: Vec<String>,
}

impl Contributes {
    /// Keys present in the manifest that this crate has no type for, sorted.
    pub fn unsupported(&self) -> &[String] {
        &self.unsupported
    }

    /// The typed kinds with at least one entry.
    pub fn kinds(&self) -> Vec<ContributionKind> {
        let mut v = Vec::new();
        if !self.font_catalogs.is_empty() {
            v.push(ContributionKind::FontCatalogs);
        }
        if !self.caption_styles.is_empty() {
            v.push(ContributionKind::CaptionStyles);
        }
        v
    }

    pub fn is_empty(&self) -> bool {
        self.kinds().is_empty() && self.unsupported.is_empty()
    }
}

impl Serialize for Contributes {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(None)?;
        if !self.font_catalogs.is_empty() {
            m.serialize_entry(ContributionKind::FontCatalogs.key(), &self.font_catalogs)?;
        }
        if !self.caption_styles.is_empty() {
            m.serialize_entry(ContributionKind::CaptionStyles.key(), &self.caption_styles)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for Contributes {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map = BTreeMap::<String, Value>::deserialize(d)?;
        let mut c = Contributes::default();
        for (key, value) in map {
            let typed = |what: &str| format!("contributes.{what}: ");
            match key.as_str() {
                "fontCatalogs" => {
                    c.font_catalogs = serde_json::from_value(value)
                        .map_err(|e| D::Error::custom(format!("{}{e}", typed(&key))))?;
                }
                "captionStyles" => {
                    c.caption_styles = serde_json::from_value(value)
                        .map_err(|e| D::Error::custom(format!("{}{e}", typed(&key))))?;
                }
                _ => c.unsupported.push(key),
            }
        }
        Ok(c)
    }
}

/// Contributions by kind (spec §4.2): the kinds this host consumes. Validation refuses any
/// other key as `PLUGIN_CONTRIBUTION_UNSUPPORTED`.
// The schema of `Contributes`, which serialises by hand.
#[derive(JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[allow(dead_code)]
struct ContributesSchema {
    #[serde(default)]
    font_catalogs: Vec<FontCatalogContribution>,
    #[serde(default)]
    caption_styles: Vec<CaptionStyleContribution>,
}

impl JsonSchema for Contributes {
    fn schema_name() -> Cow<'static, str> {
        "Contributes".into()
    }

    fn json_schema(g: &mut SchemaGenerator) -> Schema {
        ContributesSchema::json_schema(g)
    }
}
