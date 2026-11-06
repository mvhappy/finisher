//! The text-measurement contract (4C Decisions 36 and 38). `fin-core` never shapes text and
//! never reads font bytes: a [`TextMeasurer`] (implemented by `fin-text`) is injected by the
//! crates above it. A [`TextRequest`] names exact font files by SHA-256, so every machine that
//! holds those bytes measures the same layout.
//!
//! Units: every input length is in pixels at the timeline resolution (`f64`); every output
//! length is an integer in 1/64 px.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// Horizontal alignment of each line inside the layout box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Align {
    Left,
    Center,
    Right,
}

/// The style a text is measured with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextStyle {
    pub family: String,
    pub weight: u16,
    pub italic: bool,
    /// Font size in px.
    pub size: f64,
    /// Line advance as a multiple of `size` (e.g. 1.2).
    pub line_height: f64,
    /// Extra advance after every glyph, in em (a multiple of `size`; may be negative).
    pub letter_spacing: f64,
    pub align: Align,
}

/// One face of one exact font file: `face` is the index inside a collection (0 for a TTF).
/// `family`, `weight` and `italic` describe the face; only `sha256` and `face` select it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FaceRef {
    pub sha256: String,
    pub face: u32,
    pub family: String,
    pub weight: u16,
    pub italic: bool,
}

/// A text to lay out. `words` are joined by single spaces into one paragraph and wrap only
/// between words. `faces` is the primary face first, then the fallbacks in the project's
/// order; it is the only fallback list (the platform's is never used).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextRequest {
    pub words: Vec<String>,
    pub style: TextStyle,
    pub faces: Vec<FaceRef>,
    /// The layout box width in px; `None` lays out on unbounded lines.
    #[serde(default)]
    pub max_width: Option<f64>,
    #[serde(default)]
    pub max_lines: Option<u32>,
}

/// One line. `x` is the left edge of its ink box after alignment, `top` its top; `baseline` is
/// measured from the layout's top. `words` is the half-open range of word indices on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LineBox {
    pub x: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub baseline: i32,
    pub rtl: bool,
    pub first_word: u32,
    pub end_word: u32,
}

/// One word's horizontal extent on its line (visual order is given by `x`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WordBox {
    pub line: u32,
    pub x: i32,
    pub width: i32,
}

/// A measured layout, all lengths in 1/64 px. `width` is the widest line, `height` the
/// lines' total advance. `overflow` is set when a line is wider than `max_width` or there are
/// more lines than `max_lines`. `missing` lists the code points no face maps (ascending);
/// `faces` the sha256s whose glyphs the layout uses, in request order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextLayout {
    pub lines: Vec<LineBox>,
    pub words: Vec<WordBox>,
    pub width: i32,
    pub height: i32,
    pub overflow: bool,
    pub missing: Vec<u32>,
    pub faces: Vec<String>,
}

/// The answer to a [`TextRequest`]: a layout, or the sha256s whose bytes are not available
/// (not fetched, offline, failed, mismatched or unusable). Unavailable never blocks an edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Measured {
    Layout(TextLayout),
    Unavailable { missing: Vec<String> },
}

/// Measures text from exact font bytes. One measurer serves every project and branch: the
/// request alone determines the answer.
pub trait TextMeasurer: Send + Sync {
    fn measure(&self, req: &TextRequest) -> Result<Measured, CoreError>;
    /// Names the shaping code (crate versions) whose layouts this measurer produces.
    fn fingerprint(&self) -> String;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_in_camel_case() {
        let req = TextRequest {
            words: vec!["Hi".into()],
            style: TextStyle {
                family: "Inter".into(),
                weight: 400,
                italic: false,
                size: 48.0,
                line_height: 1.2,
                letter_spacing: 0.0,
                align: Align::Center,
            },
            faces: vec![FaceRef {
                sha256: "ab".repeat(32),
                face: 0,
                family: "Inter".into(),
                weight: 400,
                italic: false,
            }],
            max_width: Some(900.0),
            max_lines: Some(2),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["style"]["lineHeight"], 1.2);
        assert_eq!(json["maxWidth"], 900.0);
        let back: TextRequest = serde_json::from_value(json).unwrap();
        assert_eq!(back, req);
        let un = Measured::Unavailable {
            missing: vec!["ab".into()],
        };
        assert_eq!(
            serde_json::to_value(&un).unwrap(),
            serde_json::json!({"kind": "unavailable", "missing": ["ab"]})
        );
    }
}
