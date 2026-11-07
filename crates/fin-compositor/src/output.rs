//! What a render returns.

use fin_core::eval::ItemKind;
use fin_media::Size;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A layer (or an effect or mask on a drawn layer) the compositor left out (spec §2.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skipped {
    pub item: String,
    pub kind: ItemKind,
    pub code: String,
    pub details: Value,
}

/// An 8-bit RGBA frame (alpha 255) and what was left out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderOutput {
    pub size: Size,
    pub rgba8: Vec<u8>,
    pub skipped: Vec<Skipped>,
}
