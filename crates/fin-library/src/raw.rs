//! Raw paths below a library root (Decision 10): the component chain every open uses, WTF-8
//! for Windows names, and the wire forms of raw bytes (base64) and nanosecond times (decimal
//! strings, since they pass 2^53).
//!
//! A component is the name exactly as the OS returned it: Unix bytes, or on Windows the UTF-16
//! name as WTF-8 ([`from_wide`]; lone surrogates survive). It is never normalised, case-folded
//! or decoded; [`crate::names`] derives the display name and the lookup key from it.

use std::borrow::Cow;
use std::fmt;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The raw components below the root, outermost first; empty for the root itself.
/// Serde: `{"components": [<base64 of each component>]}` (the generated `RawRel`); a component
/// that is empty, `.`, `..`, or holds `/` or NUL (and on Windows `\`) is refused when read.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct RawRel(pub Vec<Vec<u8>>);

/// Why a [`RawRel`] component is not a single name below the root.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RawRelError {
    #[error("component {0} is empty")]
    Empty(usize),
    #[error("component {0} is `.` or `..`")]
    Dots(usize),
    #[error("component {0} holds a path separator or NUL")]
    Separator(usize),
}

impl RawRel {
    /// The root.
    pub fn root() -> Self {
        Self(Vec::new())
    }

    /// From components, checked as [`RawRel::validate`] does.
    pub fn new(components: Vec<Vec<u8>>) -> Result<Self, RawRelError> {
        let rel = Self(components);
        rel.validate()?;
        Ok(rel)
    }

    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    pub fn components(&self) -> &[Vec<u8>] {
        &self.0
    }

    /// Number of components (the root is 0).
    pub fn depth(&self) -> usize {
        self.0.len()
    }

    /// The last component (`None` for the root).
    pub fn name(&self) -> Option<&[u8]> {
        self.0.last().map(Vec::as_slice)
    }

    /// The containing directory (`None` for the root).
    pub fn parent(&self) -> Option<RawRel> {
        (!self.is_root()).then(|| Self(self.0[..self.0.len() - 1].to_vec()))
    }

    /// This path with one more component.
    pub fn join(&self, name: &[u8]) -> RawRel {
        let mut c = self.0.clone();
        c.push(name.to_vec());
        Self(c)
    }

    /// Whether `self` is `other` or below it.
    pub fn starts_with(&self, other: &RawRel) -> bool {
        self.0.starts_with(&other.0)
    }

    /// Every component is one name: non-empty, not `.` or `..`, without `/` or NUL, and on
    /// Windows without `\` (a crafted wire path must not name anything outside its chain).
    pub fn validate(&self) -> Result<(), RawRelError> {
        for (i, c) in self.0.iter().enumerate() {
            if c.is_empty() {
                return Err(RawRelError::Empty(i));
            }
            if c == b"." || c == b".." {
                return Err(RawRelError::Dots(i));
            }
            if c.iter()
                .any(|&b| b == b'/' || b == 0 || (cfg!(windows) && b == b'\\'))
            {
                return Err(RawRelError::Separator(i));
            }
        }
        Ok(())
    }

    /// A display string: each component's NFC name ([`crate::names::nfc`], lossy names with
    /// U+FFFD) joined by `/`. Never used to open anything.
    pub fn display(&self) -> String {
        let parts: Vec<String> = self.0.iter().map(|c| crate::names::nfc(c).nfc).collect();
        parts.join("/")
    }
}

impl fmt::Display for RawRel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRelRepr {
    components: Vec<String>,
}

impl Serialize for RawRel {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        RawRelRepr {
            components: self.0.iter().map(|c| STANDARD.encode(c)).collect(),
        }
        .serialize(s)
    }
}

impl<'de> Deserialize<'de> for RawRel {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let repr = RawRelRepr::deserialize(d)?;
        let components = repr
            .components
            .iter()
            .map(|c| STANDARD.decode(c).map_err(D::Error::custom))
            .collect::<Result<Vec<_>, _>>()?;
        RawRel::new(components).map_err(D::Error::custom)
    }
}

impl JsonSchema for RawRel {
    fn schema_name() -> Cow<'static, str> {
        "RawRel".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "Raw components below a library root, outermost first (empty for the root): base64 of each name's bytes (Unix bytes; Windows UTF-16 as WTF-8).",
            "type": "object",
            "properties": {
                "components": { "type": "array", "items": { "type": "string", "contentEncoding": "base64" } }
            },
            "required": ["components"],
            "additionalProperties": false
        })
    }
}

/// UTF-16 (a Windows name) as WTF-8: UTF-8 for every valid scalar value, and the generalised
/// 3-byte form for each unpaired surrogate, so any `[u16]` round-trips through [`to_wide`].
pub fn from_wide(wide: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(wide.len());
    for unit in char::decode_utf16(wide.iter().copied()) {
        match unit {
            Ok(c) => out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes()),
            Err(e) => {
                let u = u32::from(e.unpaired_surrogate());
                out.extend_from_slice(&[
                    0xE0 | (u >> 12) as u8,
                    0x80 | ((u >> 6) & 0x3F) as u8,
                    0x80 | (u & 0x3F) as u8,
                ]);
            }
        }
    }
    out
}

/// WTF-8 back to UTF-16; `None` for bytes that are not well-formed WTF-8 (invalid UTF-8
/// other than surrogates, or a surrogate pair written as two 3-byte sequences).
pub fn to_wide(wtf8: &[u8]) -> Option<Vec<u16>> {
    let mut out = Vec::with_capacity(wtf8.len());
    let mut i = 0;
    let mut prev_high = false;
    while i < wtf8.len() {
        let b = wtf8[i];
        let (len, min, init) = match b {
            0x00..=0x7F => (1, 0, u32::from(b)),
            0xC2..=0xDF => (2, 0x80, u32::from(b & 0x1F)),
            0xE0..=0xEF => (3, 0x800, u32::from(b & 0x0F)),
            0xF0..=0xF4 => (4, 0x1_0000, u32::from(b & 0x07)),
            _ => return None,
        };
        let tail = wtf8.get(i + 1..i + len)?;
        let mut cp = init;
        for &t in tail {
            if t & 0xC0 != 0x80 {
                return None;
            }
            cp = (cp << 6) | u32::from(t & 0x3F);
        }
        if cp < min || cp > 0x10_FFFF {
            return None;
        }
        let low = (0xDC00..=0xDFFF).contains(&cp);
        if low && prev_high {
            return None;
        }
        prev_high = (0xD800..=0xDBFF).contains(&cp);
        if cp >= 0x1_0000 {
            let v = cp - 0x1_0000;
            out.push(0xD800 | (v >> 10) as u16);
            out.push(0xDC00 | (v & 0x3FF) as u16);
        } else {
            out.push(cp as u16);
        }
        i += len;
    }
    Some(out)
}

/// `#[serde(with = "b64")]`: bytes as standard base64.
pub(crate) mod b64 {
    use super::*;

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        STANDARD.decode(s).map_err(D::Error::custom)
    }
}

/// `#[serde(with = "b64_opt")]` for `Option<Vec<u8>>` (with `default` and
/// `skip_serializing_if = "Option::is_none"`).
pub(crate) mod b64_opt {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&STANDARD.encode(v)),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| STANDARD.decode(s).map_err(D::Error::custom))
            .transpose()
    }
}

/// `#[serde(with = "ns")]`: an `i64` (nanoseconds since the Unix epoch) as a decimal string,
/// exact in JavaScript.
pub(crate) mod ns {
    use super::*;

    pub fn serialize<S: Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        parse(&String::deserialize(d)?).map_err(D::Error::custom)
    }

    pub(super) fn parse(s: &str) -> Result<i64, String> {
        let digits = s.strip_prefix('-').unwrap_or(s);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("not a decimal integer: {s:?}"));
        }
        s.parse().map_err(|e| format!("{e}: {s:?}"))
    }
}

/// `#[serde(with = "ns_opt")]` for `Option<i64>`.
pub(crate) mod ns_opt {
    use super::*;

    pub fn serialize<S: Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(v) => s.serialize_some(&v.to_string()),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| ns::parse(&s).map_err(D::Error::custom))
            .transpose()
    }
}
