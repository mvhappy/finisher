//! The text-measurement contract (4C Decisions 36 and 38). `fin-core` never shapes text and
//! never reads font bytes: a [`TextMeasurer`] (implemented by `fin-text`) is injected by the
//! crates above it. A [`TextRequest`] names exact font files by SHA-256, so every machine that
//! holds those bytes measures the same layout.
//!
//! Units: every input length is in pixels at the timeline resolution (`f64`); every output
//! length is an integer in 1/64 px.

use std::collections::BTreeSet;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;
use unicode_script::{Script, UnicodeScript};

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

/// A text to lay out. `words` are joined by single spaces into one paragraph; lines break
/// only between words, never inside one (a word wider than `max_width` gets its own line and
/// overflows). Words hold no control, line or paragraph separator characters
/// ([`check_words`]). `faces` is the primary face first, then the fallbacks in the project's
/// order; it is the only fallback list (the platform's is never used) and is never empty.
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

/// The pseudo-script (ISO 15924 `Zsye`) of emoji, which Unicode gives the `Common` script.
pub const EMOJI_SCRIPT: &str = "Zsye";

/// Whether `c` selects an emoji face: the pictographic blocks (U+1F000–U+1FAFF, which hold
/// the regional indicators too), Miscellaneous Symbols and Dingbats (U+2600–U+27BF), the
/// emoji in Miscellaneous Technical and Arrows (U+2300–U+23FF, U+2B00–U+2BFF), the CJK emoji
/// U+3030, U+303D, U+3297, U+3299, and VS16 (U+FE0F). Text-default symbols such as © are not.
pub fn is_emoji(c: char) -> bool {
    matches!(u32::from(c),
        0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2300..=0x23FF | 0x2B00..=0x2BFF
        | 0x3030 | 0x303D | 0x3297 | 0x3299 | 0xFE0F)
}

/// The scripts a text needs faces for (4C Decision 35): ISO 15924 codes from `unicode-script`,
/// [`EMOJI_SCRIPT`] for emoji; `Common`, `Inherited` and `Unknown` are ignored. The one rule
/// for every caller (pin planning, face resolution, measurement tests).
pub fn scripts_of_text(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for c in text.chars() {
        if is_emoji(c) {
            out.insert(EMOJI_SCRIPT.to_string());
            continue;
        }
        let script = c.script();
        if !matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
            out.insert(script.short_name().to_string());
        }
    }
    out
}

/// Characters a word may not hold: controls (`Cc`, which includes tab, LF, CR, VT, FF, NEL and
/// the bidi paragraph separators U+001C–U+001E), U+2028 LINE SEPARATOR and U+2029 PARAGRAPH
/// SEPARATOR. A caption breaks into lines only between words.
pub fn is_forbidden_in_word(c: char) -> bool {
    c.is_control() || c == '\u{2028}' || c == '\u{2029}'
}

/// `INVALID_OP` naming the first word and character [`is_forbidden_in_word`] refuses.
pub fn check_words(words: &[String]) -> Result<(), CoreError> {
    for (i, w) in words.iter().enumerate() {
        if let Some(c) = w.chars().find(|&c| is_forbidden_in_word(c)) {
            let cp = format!("U+{:04X}", u32::from(c));
            return Err(CoreError::new(
                "INVALID_OP",
                format!("word {i} holds the control character {cp}"),
            )
            .with_details(json!({ "path": "words", "word": i, "codePoint": cp }))
            .with_fix(
                "remove control, tab and line-break characters; lines break only between words",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_emoji() {
        let s = scripts_of_text("Hi Привет 中文 かな 😀 1, ©");
        let s: Vec<&str> = s.iter().map(String::as_str).collect();
        assert_eq!(s, ["Cyrl", "Hani", "Hira", "Latn", "Zsye"]);
        assert!(scripts_of_text("12 ,.!").is_empty());
        assert!(is_emoji('\u{2764}') && is_emoji('\u{1F1FA}') && !is_emoji('©'));
    }

    #[test]
    fn words_refuse_controls_and_separators() {
        for bad in [
            "a\nb",
            "a\rb",
            "a\tb",
            "a\u{b}b",
            "a\u{c}b",
            "a\u{85}b",
            "a\u{2028}b",
            "a\u{2029}b",
            "a\u{1d}b",
        ] {
            let err = check_words(&["ok".into(), bad.into()]).unwrap_err();
            assert_eq!(
                (err.code.as_str(), err.details["word"].as_u64()),
                ("INVALID_OP", Some(1)),
                "{bad:?}"
            );
            assert!(err.fix.is_some());
        }
        assert!(
            check_words(&[
                "a-b".into(),
                "a b".into(),
                "\u{200F}x".into(),
                String::new()
            ])
            .is_ok()
        );
    }

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
