//! Field classes (plan D, Decision 4): every payload field is `meta`, `scrubbed` or
//! `content`, by its type, and its JSON Schema says which (`x-fin-class`).
//!
//! - [`Content`] wraps anything a user or a model wrote, or anything that names user data
//!   (`x-fin-class: content`). The field names its kind with
//!   `#[schemars(extend("x-fin-content" = "<kind>"))]` (one of [`ContentKind`]). Content is
//!   removed from an entry unless training is on ([`super::redact`]).
//! - [`Scrubbed`] is Finisher-authored free text with paths, the user and the host replaced
//!   ([`super::scrub`]). It appears only inside [`Content`] in this phase.
//! - [`Name`] holds a `&'static str` from code or a static table ([`super::names`]); nothing a
//!   user typed can become one. The pattern-checked meta strings ([`Hashed`], [`IdRef`],
//!   [`Code`], [`SrcLoc`], ...) live in `refs.rs` and are re-exported here.
//!
//! Numbers, booleans and closed enums are `meta` implicitly.

use std::borrow::Cow;
use std::fmt;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use super::refs::{
    Blake3Hex, BlobHash, BlobRef, BranchId, Code, Hashed, IdRef, KindName, MAX_RAW_ID, NeedsHash,
    PathRef, PathRoot, ProjectId, RawId, SrcLoc, Ulid, Version,
};
use super::scrub::scrub;

/// The `x-fin-class` values.
pub const META: &str = "meta";
pub const SCRUBBED: &str = "scrubbed";
pub const CONTENT: &str = "content";

/// User or model content of kind named by the field (`x-fin-content`). Serialised as `T`
/// itself. Removed unless training is on, both when recorded and when written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Content<T>(pub T);

impl<T: JsonSchema> JsonSchema for Content<T> {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        format!("Content_{}", T::schema_name()).into()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let mut schema = generator.subschema_for::<T>();
        schema.insert("x-fin-class".into(), CONTENT.into());
        schema
    }
}

/// What a content field holds (`x-fin-content`). `frame` and `audio` always hold a
/// [`BlobRef`] (Decision 20).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "camelCase")]
pub enum ContentKind {
    UserText,
    Caption,
    FileName,
    Prompt,
    ModelInput,
    ModelOutput,
    ToolArgs,
    ToolResult,
    MediaHash,
    TxIntent,
    ErrorMessage,
    Frame,
    Audio,
}

impl ContentKind {
    pub const ALL: [ContentKind; 13] = [
        ContentKind::UserText,
        ContentKind::Caption,
        ContentKind::FileName,
        ContentKind::Prompt,
        ContentKind::ModelInput,
        ContentKind::ModelOutput,
        ContentKind::ToolArgs,
        ContentKind::ToolResult,
        ContentKind::MediaHash,
        ContentKind::TxIntent,
        ContentKind::ErrorMessage,
        ContentKind::Frame,
        ContentKind::Audio,
    ];

    /// The `x-fin-content` value (`"txIntent"`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::UserText => "userText",
            ContentKind::Caption => "caption",
            ContentKind::FileName => "fileName",
            ContentKind::Prompt => "prompt",
            ContentKind::ModelInput => "modelInput",
            ContentKind::ModelOutput => "modelOutput",
            ContentKind::ToolArgs => "toolArgs",
            ContentKind::ToolResult => "toolResult",
            ContentKind::MediaHash => "mediaHash",
            ContentKind::TxIntent => "txIntent",
            ContentKind::ErrorMessage => "errorMessage",
            ContentKind::Frame => "frame",
            ContentKind::Audio => "audio",
        }
    }

    pub fn parse(s: &str) -> Option<ContentKind> {
        ContentKind::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

/// What [`Scrubbed::new`] replaces, each with its placeholder: the home directory (`<home>`),
/// the server data directory (`<data>`), the temp directory (`<tmp>`), the user name
/// (`<user>`) and the host name (`<host>`). Any other absolute path becomes `<path>`. The
/// emitter fills it once from its environment; `fin-core` reads none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScrubCtx {
    pub home: Option<String>,
    pub server_data: Option<String>,
    pub tmp: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
}

/// Finisher-authored free text, scrubbed ([`super::scrub`]) before it exists. Its field is
/// private: [`Scrubbed::new`] is the only constructor. Deserialising scrubs again with an
/// empty [`ScrubCtx`] (absolute paths only), so a received value is never stored raw; a
/// receiver that knows its environment calls [`Scrubbed::rescrub`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scrubbed(String);

impl Scrubbed {
    pub fn new(text: &str, ctx: &ScrubCtx) -> Scrubbed {
        Scrubbed(scrub(text, ctx))
    }

    /// Scrubs the text again with `ctx` (for a value received from another process).
    pub fn rescrub(&self, ctx: &ScrubCtx) -> Scrubbed {
        Scrubbed::new(&self.0, ctx)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for Scrubbed {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Scrubbed {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Ok(Scrubbed::new(&text, &ScrubCtx::default()))
    }
}

impl JsonSchema for Scrubbed {
    fn schema_name() -> Cow<'static, str> {
        "Scrubbed".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "x-fin-class": SCRUBBED,
            "description": "Finisher-authored text with paths, the home, data and temp directories, the user name and the host name replaced by placeholders."
        })
    }
}

/// The `Name` pattern: `^[a-z][A-Za-z0-9_./-]{0,63}$`. Wider than the plan's first
/// `^[a-z][a-z0-9_. -]{0,63}$`, which the tables it names do not fit: RPC methods are camel
/// case (`history.cherryPick`) and media types hold a slash (`image/webp`). No space: every
/// table name is dotted (`item.trim`).
pub const NAME_PATTERN: &str = r"^[a-z][A-Za-z0-9_./-]{0,63}$";

/// Whether `s` matches [`NAME_PATTERN`].
pub const fn is_valid_name(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 64 || !b[0].is_ascii_lowercase() {
        return false;
    }
    let mut i = 1;
    while i < b.len() {
        let c = b[i];
        if !(c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'/' | b'-')) {
            return false;
        }
        i += 1;
    }
    true
}

/// A name from code or a static table: a command, a flag, an RPC method, a media extension or
/// type, an op kind, an error-detail key. Its field is private and holds a `&'static str`, so
/// nothing a user typed can become one. Outside `fin-core` a `Name` comes only from a table
/// ([`super::names::NameTable::get`], [`super::names::NameTable::get_or_other`]) or is
/// [`Name::OTHER`]; inside, [`Name::from_static`] (crate-private, a string literal, checked
/// against [`NAME_PATTERN`] in every build) is the one constructor. There is no
/// `From<String>`, `FromStr`, `Default` or `Deserialize`: a `Name` field deserialises through
/// its table (`#[serde(deserialize_with = "names::cli_command")]`), and a value not in the
/// table reads as `"other"`.
///
/// ```compile_fail
/// let n: fin_core::events::Name = serde_json::from_str("\"item.trim\"").unwrap();
/// ```
///
/// ```compile_fail
/// let n = fin_core::events::Name::from(String::from("item.trim"));
/// ```
///
/// ```compile_fail
/// let n: fin_core::events::Name = "item.trim".parse().unwrap();
/// ```
///
/// ```compile_fail
/// let leaked: &'static str = Box::leak(String::from("typed by a user").into_boxed_str());
/// let n = fin_core::events::Name::from_static(leaked);
/// ```
///
/// ```compile_fail
/// static T: fin_core::events::names::NameTable =
///     fin_core::events::names::NameTable::new("T", &["typed"]);
/// ```
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(&'static str);

impl Name {
    /// The fallback for a value not in a table.
    pub const OTHER: Name = Name("other");

    /// A name written in code or held by a static table. Checks [`NAME_PATTERN`] in every
    /// build: at compile time in a `const` or `static`, else when called.
    pub(crate) const fn from_static(s: &'static str) -> Name {
        assert!(is_valid_name(s), "a Name must match NAME_PATTERN");
        Name(s)
    }

    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.0)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

impl JsonSchema for Name {
    fn schema_name() -> Cow<'static, str> {
        "Name".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "pattern": NAME_PATTERN,
            "x-fin-class": META,
            "description": "A name from code or a static table (fin-core events::names); \"other\" for a value not in the table."
        })
    }
}
