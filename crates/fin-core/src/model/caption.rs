//! Caption blocks and caption styles.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::item::ParamValue;
use crate::time::Rational;

/// A caption item's body: its words, its style (`preset:<name>` or a project style `st<n>`),
/// per-item style overrides by registry path, and the transcript it came from.
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

/// A project caption style (`st<n>`). Its fields arrive with captions (plan 4C Task 18); until
/// then the project holds none.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionStyle {}
