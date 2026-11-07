//! Caption blocks and caption styles (plan 4C Task 18, Decision 28).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::layout::{Align, TextStyle};
use crate::model::item::ParamValue;
use crate::time::Rational;

/// A caption item's body: its words, its style (`preset:<name>` or a project style `st<n>`),
/// per-item style overrides by style path (`text.size`, `color`, ...), and the transcript it
/// came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionBlock {
    pub words: Vec<Word>,
    pub style: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, ParamValue>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "CaptionSource")]
    pub source: Option<CaptionSource>,
}

/// One word; `start` and `end` are relative to the item's start (plan 4C Decision 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Word {
    pub text: String,
    pub start: Rational,
    pub end: Rational,
}

/// The transcript artifact a caption came from, and its word range (`[first, end)`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionSource {
    pub artifact: String,
    pub word_range: [u32; 2],
}

/// A caption style: a built-in preset (`preset:<name>`, `fin_core::captions::presets`) or a
/// project style (`st<n>`, made by `style.add`). Lengths are px at the timeline resolution.
///
/// - `text` names the face (**family, weight, italic**: it resolves to a font pin, Decision
///   29) and how lines are set;
/// - `stroke`, `background` and `highlight` (the spoken word's colour) are optional;
/// - `position` places the block inside the safe zone `safeZone` (a name from
///   `fin_core::captions::safe_zones`);
/// - `maxChars` (per line), `maxLines`, `gap` (a pause at least this long starts a new block;
///   0 never does) and `maxDuration` are the block-splitting rule (`captions::blocks`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionStyle {
    pub id: String,
    pub name: String,
    pub text: TextStyle,
    pub color: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Stroke")]
    pub stroke: Option<Stroke>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "CaptionBox")]
    pub background: Option<CaptionBox>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub highlight: Option<String>,
    pub position: CaptionPosition,
    pub safe_zone: String,
    pub max_chars: u32,
    pub max_lines: u32,
    pub gap: Rational,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub max_duration: Option<Rational>,
}

impl Default for CaptionStyle {
    /// The base of a `style.add` without `from` (and what a reset returns to): Inter Bold
    /// 64 px, centred, white, at the bottom of the generic 9:16 safe zone, two lines of up to
    /// 32 characters, a new block after a pause of 1/2 s.
    fn default() -> Self {
        CaptionStyle {
            id: String::new(),
            name: String::new(),
            text: TextStyle {
                family: "Inter".into(),
                weight: 700,
                italic: false,
                size: 64.0,
                line_height: 1.2,
                letter_spacing: 0.0,
                align: Align::Center,
            },
            color: "#ffffff".into(),
            stroke: None,
            background: None,
            highlight: None,
            position: CaptionPosition::default(),
            safe_zone: "generic-9x16".into(),
            max_chars: 32,
            max_lines: 2,
            gap: Rational::new(1, 2).unwrap_or_default(),
            max_duration: None,
        }
    }
}

/// An outline around the glyphs. A stroke equal to the default (width 0) is no stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Stroke {
    pub color: String,
    pub width: f64,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke {
            color: "#000000".into(),
            width: 0.0,
        }
    }
}

/// A box behind the block. One equal to the default (transparent, no padding) is no box.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionBox {
    pub color: String,
    pub padding: f64,
    pub radius: f64,
}

impl Default for CaptionBox {
    fn default() -> Self {
        CaptionBox {
            color: "#00000000".into(),
            padding: 0.0,
            radius: 0.0,
        }
    }
}

/// Where the block sits in its safe zone: against the zone's top, middle or bottom, moved
/// towards the middle by `offset` (a fraction of the zone's height, 0 to 1).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionPosition {
    pub anchor: Anchor,
    pub offset: f64,
}

/// The safe-zone edge a caption block is placed against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Anchor {
    Top,
    Middle,
    #[default]
    Bottom,
}
