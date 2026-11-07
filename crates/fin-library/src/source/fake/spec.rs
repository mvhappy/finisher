//! The fake's JSON spec (see [`super`] for the format) and the tree it builds.

use std::io;
use std::time::Duration;

use serde::Deserialize;

use crate::raw::{RawRel, from_wide};
use crate::source::{Attrs, RawEntry};
use crate::types::EntryType;

/// A spec that does not describe a tree.
#[derive(Debug, thiserror::Error)]
pub enum FakeError {
    #[error("fake spec: {0}")]
    Json(#[from] serde_json::Error),
    #[error("fake spec: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VolumeSpec {
    pub(super) key: String,
    #[serde(default = "default_kind")]
    pub(super) kind: String,
    #[serde(default = "default_dev")]
    pub(super) dev_id: String,
    #[serde(default)]
    pub(super) parent_dev_id: Option<String>,
    #[serde(default)]
    pub(super) mount_root_attr: Option<bool>,
    #[serde(default = "yes")]
    pub(super) case_sensitive: bool,
    #[serde(default)]
    pub(super) remote: bool,
    #[serde(default)]
    pub(super) root_file_id: Option<String>,
}

fn default_kind() -> String {
    "fake".into()
}
fn default_dev() -> String {
    "1".into()
}
fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RootSpec {
    pub(super) volume: VolumeSpec,
    /// The root directory's own mtime ([`crate::source::Source::dir_mtime`]).
    #[serde(default)]
    pub(super) mtime_ns: i64,
    #[serde(default)]
    pub(super) stat_delay_ms: u64,
    #[serde(default)]
    pub(super) stat_error: Option<String>,
    #[serde(default)]
    pub(super) read_delay_ms: u64,
    #[serde(default)]
    pub(super) error: Option<String>,
    #[serde(default)]
    pub(super) children: Vec<NodeSpec>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NodeSpec {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    name_hex: Option<String>,
    #[serde(default)]
    name_wide: Option<Vec<u16>>,
    #[serde(default, rename = "type")]
    kind: Option<EntryType>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    mtime_ns: i64,
    #[serde(default)]
    file_id: Option<String>,
    #[serde(default)]
    dev: Option<String>,
    #[serde(default)]
    macos_flags: Option<u32>,
    #[serde(default)]
    windows_attributes: Option<u32>,
    #[serde(default)]
    reparse_tag: Option<String>,
    #[serde(default)]
    mount_root: bool,
    #[serde(default)]
    automount: bool,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    open_error: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    read_delay_ms: u64,
    #[serde(default)]
    children: Option<Vec<NodeSpec>>,
}

#[derive(Debug, Clone)]
pub(super) struct Node {
    pub(super) entry: RawEntry,
    pub(super) content: Option<Vec<u8>>,
    pub(super) open_error: Option<String>,
    pub(super) error: Option<String>,
    pub(super) read_delay: Duration,
    pub(super) children: Vec<Node>,
}

#[derive(Debug, Clone)]
pub(super) struct Tree {
    pub(super) root: RootSpec,
    pub(super) root_file_id: Option<Vec<u8>>,
    pub(super) children: Vec<Node>,
}

fn hex(s: &str) -> Result<Vec<u8>, FakeError> {
    let bad = || FakeError::Invalid(format!("not hex: {s:?}"));
    if !s.len().is_multiple_of(2) {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2).ok_or_else(bad)?, 16).map_err(|_| bad()))
        .collect()
}

pub(crate) fn error(name: &str) -> io::Error {
    let kind = match name {
        "notFound" => io::ErrorKind::NotFound,
        "permissionDenied" => io::ErrorKind::PermissionDenied,
        "timedOut" => io::ErrorKind::TimedOut,
        "unsupported" => io::ErrorKind::Unsupported,
        "invalidParameter" => io::ErrorKind::InvalidInput,
        _ => io::ErrorKind::Other,
    };
    io::Error::new(kind, format!("fake {name}"))
}

pub(crate) fn check_error(name: &Option<String>) -> Result<(), FakeError> {
    match name.as_deref() {
        None
        | Some(
            "notFound" | "permissionDenied" | "timedOut" | "io" | "unsupported"
            | "invalidParameter",
        ) => Ok(()),
        Some(other) => Err(FakeError::Invalid(format!("unknown error {other:?}"))),
    }
}

fn build(spec: &NodeSpec, parent_dev: &str) -> Result<Node, FakeError> {
    let name = match (&spec.name, &spec.name_hex, &spec.name_wide) {
        (Some(n), None, None) => n.as_bytes().to_vec(),
        (None, Some(h), None) => hex(h)?,
        (None, None, Some(w)) => from_wide(w),
        _ => {
            return Err(FakeError::Invalid(
                "a node needs exactly one of name, nameHex, nameWide".into(),
            ));
        }
    };
    if RawRel::new(vec![name.clone()]).is_err() {
        return Err(FakeError::Invalid(format!("not a single name: {name:?}")));
    }
    let entry_type = spec.kind.unwrap_or(if spec.children.is_some() {
        EntryType::Dir
    } else {
        EntryType::File
    });
    let attrs = match (spec.macos_flags, spec.windows_attributes) {
        (None, None) => Attrs::None,
        (Some(flags), None) => Attrs::Macos { flags },
        (None, Some(attributes)) => Attrs::Windows { attributes },
        _ => {
            return Err(FakeError::Invalid(
                "macosFlags and windowsAttributes are exclusive".into(),
            ));
        }
    };
    let reparse_tag = spec
        .reparse_tag
        .as_deref()
        .map(|t| u32::from_str_radix(t.trim_start_matches("0x"), 16))
        .transpose()
        .map_err(|e| FakeError::Invalid(format!("reparseTag: {e}")))?;
    check_error(&spec.open_error)?;
    check_error(&spec.error)?;
    let dev = spec.dev.clone().unwrap_or_else(|| parent_dev.to_owned());
    let content = spec.content.as_ref().map(|c| c.as_bytes().to_vec());
    let size = spec
        .size
        .unwrap_or_else(|| content.as_ref().map_or(0, |c| c.len() as u64));
    let children = spec
        .children
        .iter()
        .flatten()
        .map(|c| build(c, &dev))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Node {
        entry: RawEntry {
            name,
            entry_type,
            size,
            mtime_ns: spec.mtime_ns,
            file_id: spec.file_id.as_deref().map(hex).transpose()?,
            dev: Some(dev),
            attrs,
            reparse_tag,
            mount_root: spec.mount_root,
            automount: spec.automount,
        },
        content,
        open_error: spec.open_error.clone(),
        error: spec.error.clone(),
        read_delay: Duration::from_millis(spec.read_delay_ms),
        children,
    })
}

pub(super) fn parse(spec: &str) -> Result<Tree, FakeError> {
    let root: RootSpec = serde_json::from_str(spec)?;
    check_error(&root.stat_error)?;
    check_error(&root.error)?;
    let children = root
        .children
        .iter()
        .map(|c| build(c, &root.volume.dev_id))
        .collect::<Result<Vec<_>, _>>()?;
    let root_file_id = root.volume.root_file_id.as_deref().map(hex).transpose()?;
    Ok(Tree {
        root,
        root_file_id,
        children,
    })
}
