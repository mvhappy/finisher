//! Display names and lookup keys from raw names (Decision 10).
//!
//! - **`nameNfc`** ([`nfc`]): the raw bytes decoded as UTF-8 (WTF-8 for Windows names), each
//!   invalid sequence (a lone surrogate among them) replaced by U+FFFD with `lossy: true`,
//!   then NFC. For display and search.
//! - **`relKey`** ([`rel_key`]): the NFC components joined by `/`, root-relative without a
//!   leading `/` (the root's key is empty), and on a case-insensitive root lower-cased with
//!   `str::to_lowercase` (then NFC again, so a key is always NFC). A lookup key only: two raw
//!   names may share one ([`crate::collide`]).

use unicode_normalization::UnicodeNormalization;

use crate::raw::RawRel;

/// A raw name's display form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    pub nfc: String,
    pub lossy: bool,
}

/// The raw bytes decoded (not yet normalised): UTF-8, with U+FFFD for each invalid sequence;
/// `true` when anything was replaced.
pub fn decode(raw: &[u8]) -> (String, bool) {
    match String::from_utf8_lossy(raw) {
        std::borrow::Cow::Borrowed(s) => (s.to_owned(), false),
        std::borrow::Cow::Owned(s) => (s, true),
    }
}

/// The NFC display name of a raw component.
pub fn nfc(raw: &[u8]) -> Name {
    let (s, lossy) = decode(raw);
    Name {
        nfc: s.nfc().collect(),
        lossy,
    }
}

/// One component's key: `nfc` itself on a case-sensitive root, else lower-cased and
/// re-normalised.
pub fn fold(nfc: &str, case_sensitive: bool) -> String {
    if case_sensitive {
        nfc.to_owned()
    } else {
        nfc.to_lowercase().nfc().collect()
    }
}

/// The key of `name_nfc` inside the directory whose key is `dir_key`.
pub fn rel_key(dir_key: &str, name_nfc: &str, case_sensitive: bool) -> String {
    let name = fold(name_nfc, case_sensitive);
    if dir_key.is_empty() {
        name
    } else {
        format!("{dir_key}/{name}")
    }
}

/// The key of a directory given by its raw chain.
pub fn dir_key(rel: &RawRel, case_sensitive: bool) -> String {
    let parts: Vec<String> = rel
        .components()
        .iter()
        .map(|c| fold(&nfc(c).nfc, case_sensitive))
        .collect();
    parts.join("/")
}
