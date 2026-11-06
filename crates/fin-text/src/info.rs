//! `font_info`: the one source of a font file's faces, family names, weights, italics and
//! script coverage (4C Decisions 37 and 38), used by `fin-probe` and catalog verification.
//!
//! **Coverage rule.** A script (ISO 15924) is covered when the face maps every exemplar code
//! point listed for it below; a script without exemplars is covered when the face maps at
//! least [`MIN_OTHER`] of its code points. `Zsye` (emoji) uses exemplars, though Unicode gives
//! emoji the `Common` script. `Common`, `Inherited` and `Unknown` are never listed. Changing
//! this rule changes the `scripts` of new pins, so treat it as a format.

use std::collections::BTreeMap;

use cosmic_text::skrifa::MetadataProvider;
use cosmic_text::skrifa::raw::types::NameId;
use cosmic_text::skrifa::raw::{FileRef, FontRef, TableProvider};
use fin_core::CoreError;
use serde_json::json;
use unicode_script::{Script, UnicodeScript};

/// One face of a font file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceInfo {
    /// The face index inside a collection (0 for a single font).
    pub index: u32,
    /// The typographic family (name ID 16), else the family (name ID 1).
    pub family: String,
    /// OS/2 `usWeightClass` (400 without an OS/2 table).
    pub weight: u16,
    /// OS/2 italic or oblique, or the `head` italic bit.
    pub italic: bool,
    /// ISO 15924 codes, sorted.
    pub scripts: Vec<String>,
}

/// Scripts without exemplars need this many mapped code points.
pub const MIN_OTHER: usize = 16;

type Ranges = &'static [(u32, u32)];

/// Exemplars per script: inclusive code point ranges the face must map in full.
const EXEMPLARS: &[(&str, Ranges)] = &[
    ("Latn", &[(0x41, 0x5A), (0x61, 0x7A)]),
    ("Cyrl", &[(0x410, 0x44F)]),
    ("Grek", &[(0x391, 0x3A1), (0x3A3, 0x3A9), (0x3B1, 0x3C9)]),
    ("Arab", &[(0x627, 0x63A), (0x641, 0x64A)]),
    ("Hebr", &[(0x5D0, 0x5EA)]),
    (
        "Hani",
        &[
            (0x4E00, 0x4E00),
            (0x4EBA, 0x4EBA),
            (0x4E2D, 0x4E2D),
            (0x65E5, 0x65E5),
            (0x5927, 0x5927),
            (0x5B50, 0x5B50),
        ],
    ),
    (
        "Hira",
        &[
            (0x3042, 0x3042),
            (0x304B, 0x304B),
            (0x3055, 0x3055),
            (0x305F, 0x305F),
            (0x306A, 0x306A),
            (0x3093, 0x3093),
        ],
    ),
    (
        "Kana",
        &[
            (0x30A2, 0x30A2),
            (0x30AB, 0x30AB),
            (0x30B5, 0x30B5),
            (0x30BF, 0x30BF),
            (0x30CA, 0x30CA),
            (0x30F3, 0x30F3),
        ],
    ),
    (
        "Hang",
        &[
            (0xAC00, 0xAC00),
            (0xB098, 0xB098),
            (0xB2E4, 0xB2E4),
            (0xD55C, 0xD55C),
            (0xAE00, 0xAE00),
        ],
    ),
    ("Thai", &[(0xE01, 0xE2E)]),
    ("Deva", &[(0x905, 0x914), (0x915, 0x939)]),
    (
        "Zsye",
        &[
            (0x1F600, 0x1F600),
            (0x1F44D, 0x1F44D),
            (0x1F525, 0x1F525),
            (0x1F389, 0x1F389),
        ],
    ),
];

fn invalid(why: impl Into<String>) -> CoreError {
    CoreError::new("FONT_INVALID", why)
        .with_fix("use a TrueType or OpenType font file (.ttf, .otf, .ttc)")
}

/// Every face of `bytes` (a TTF, OTF or TTC), in face order. `FONT_INVALID` when the file or
/// a face does not parse, or a face has no family name.
pub fn font_info(bytes: &[u8]) -> Result<Vec<FaceInfo>, CoreError> {
    let file = FileRef::new(bytes).map_err(|e| invalid(format!("not a font file: {e}")))?;
    let fonts: Vec<FontRef<'_>> = match file {
        FileRef::Font(f) => vec![f],
        FileRef::Collection(c) => c
            .iter()
            .collect::<Result<_, _>>()
            .map_err(|e| invalid(format!("a collection face does not parse: {e}")))?,
    };
    if fonts.is_empty() {
        return Err(invalid("the collection has no faces"));
    }
    fonts
        .iter()
        .enumerate()
        .map(|(i, f)| face_info(i as u32, f))
        .collect()
}

fn name(font: &FontRef<'_>, id: NameId) -> Option<String> {
    let s = font.localized_strings(id).english_or_first()?.to_string();
    (!s.trim().is_empty()).then_some(s)
}

fn face_info(index: u32, font: &FontRef<'_>) -> Result<FaceInfo, CoreError> {
    let family = name(font, NameId::TYPOGRAPHIC_FAMILY_NAME)
        .or_else(|| name(font, NameId::FAMILY_NAME))
        .ok_or_else(|| {
            invalid(format!("face {index} has no family name"))
                .with_details(json!({ "face": index }))
        })?;
    let os2 = font.os2().ok();
    let weight = os2.as_ref().map_or(400, |t| t.us_weight_class());
    // fsSelection bit 0 (ITALIC) or 9 (OBLIQUE); head macStyle bit 1 (italic).
    let italic = os2
        .as_ref()
        .is_some_and(|t| t.fs_selection().bits() & 0x0201 != 0)
        || font
            .head()
            .is_ok_and(|h| h.mac_style().bits() & 0x0002 != 0);
    Ok(FaceInfo {
        index,
        family,
        weight,
        italic,
        scripts: scripts(font),
    })
}

fn scripts(font: &FontRef<'_>) -> Vec<String> {
    let charmap = font.charmap();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (cp, _) in charmap.mappings() {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let script = c.script();
        if matches!(script, Script::Common | Script::Inherited | Script::Unknown) {
            continue;
        }
        *counts.entry(script.short_name()).or_default() += 1;
    }
    let covers = |ranges: Ranges| {
        ranges.iter().all(|&(lo, hi)| {
            (lo..=hi).all(|cp| char::from_u32(cp).is_some_and(|c| charmap.map(c).is_some()))
        })
    };
    let mut out: Vec<String> = Vec::new();
    for (code, ranges) in EXEMPLARS {
        if covers(ranges) {
            out.push((*code).to_string());
        }
    }
    for (code, n) in counts {
        if n >= MIN_OTHER && !EXEMPLARS.iter().any(|(c, _)| *c == code) {
            out.push(code.to_string());
        }
    }
    out.sort();
    out
}
