//! Plugin ids (8A Decision 2): `@<publisher>/<name>`, publisher `[a-z0-9][a-z0-9-]{1,38}`,
//! name `[a-z0-9][a-z0-9-]{1,63}`, no `--`, no trailing `-` and no Windows device name
//! (`con`, `prn`, `aux`, `nul`, `com1`–`com9`, `lpt1`–`lpt9`).

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Publishers no registry plugin may use (spec §4.1); bundled plugins may.
pub const RESERVED_PUBLISHERS: &[&str] = &["finisher", "fin", "official", "system", "local"];
/// The developer-mode scope (spec §5.5): only for plugins loaded with `--plugin-dir`.
pub const DEV_PUBLISHER: &str = "dev";

/// Why a string is not a plugin id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// Not `@<publisher>/<name>`.
    Shape,
    Publisher,
    Name,
}

impl IdError {
    pub fn reason(self) -> &'static str {
        match self {
            IdError::Shape => "idShape",
            IdError::Publisher => "idPublisher",
            IdError::Name => "idName",
        }
    }
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            IdError::Shape => "a plugin id is @<publisher>/<name>",
            IdError::Publisher => {
                "a publisher is 2 to 39 of [a-z0-9-], starting with a letter or digit, with no `--`, no trailing `-` and no Windows device name"
            }
            IdError::Name => {
                "a plugin name is 2 to 64 of [a-z0-9-], starting with a letter or digit, with no `--`, no trailing `-` and no Windows device name"
            }
        })
    }
}

impl std::error::Error for IdError {}

/// `@<publisher>/<name>`, validated on construction.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PluginId {
    publisher: String,
    name: String,
}

/// `[a-z0-9]` then `max_rest` or fewer of `[a-z0-9-]` (at least one), no `--`, no trailing `-`.
/// Windows device names: a publisher or name directory called this cannot be created on
/// Windows (`store/@<publisher>/<name>/`, `data/…`), in any case.
pub const RESERVED_DEVICE_NAMES: &[&str] = &[
    "con", "prn", "aux", "nul", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

fn is_device_name(s: &str) -> bool {
    RESERVED_DEVICE_NAMES
        .iter()
        .any(|d| d.eq_ignore_ascii_case(s))
}

fn segment_ok(s: &str, max_rest: usize) -> bool {
    let b = s.as_bytes();
    let ok_char = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    b.len() >= 2
        && b.len() <= max_rest + 1
        && ok_char(b[0])
        && b.iter().all(|&c| ok_char(c) || c == b'-')
        && !s.contains("--")
        && !s.ends_with('-')
        && !is_device_name(s)
}

impl PluginId {
    pub fn parse(s: &str) -> Result<PluginId, IdError> {
        let rest = s.strip_prefix('@').ok_or(IdError::Shape)?;
        let (publisher, name) = rest.split_once('/').ok_or(IdError::Shape)?;
        if !segment_ok(publisher, 38) {
            return Err(IdError::Publisher);
        }
        if !segment_ok(name, 63) {
            return Err(IdError::Name);
        }
        Ok(PluginId {
            publisher: publisher.to_string(),
            name: name.to_string(),
        })
    }

    pub fn publisher(&self) -> &str {
        &self.publisher
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn is_reserved(&self) -> bool {
        RESERVED_PUBLISHERS.contains(&self.publisher.as_str())
    }

    pub fn is_dev(&self) -> bool {
        self.publisher == DEV_PUBLISHER
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}/{}", self.publisher, self.name)
    }
}

impl Serialize for PluginId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for PluginId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        PluginId::parse(&s).map_err(|e| serde::de::Error::custom(format!("id {s:?}: {e}")))
    }
}

impl JsonSchema for PluginId {
    fn schema_name() -> Cow<'static, str> {
        "PluginId".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "`@<publisher>/<name>` (8A Decision 2).",
            "type": "string",
            "pattern": "^@[a-z0-9][a-z0-9-]{1,38}/[a-z0-9][a-z0-9-]{1,63}$",
        })
    }
}
