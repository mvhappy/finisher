//! Manifest validation in Rust (spec §4.1), one module per rule family:
//! - [`kind`]: id, version, publisher, per-kind fields, runtimes, and compatibility with this
//!   host (`PLUGIN_HOST_API`, `PLUGIN_PLATFORM`, `PLUGIN_NODE_VERSION`);
//! - [`permissions`]: grammar, duplicates, `hooks:modify` within `hooks:observe`;
//! - [`files`]: every referenced file is a safe package path and present;
//! - [`messages`]: catalogues and message keys in the `en` catalogue;
//! - [`content`]: contributions: unconsumable kinds refused, consumable ones checked through
//!   the injected [`ContentChecks`], commands' names, timeouts and argument schemas;
//! - licences (here): `nonCommercial` refused; `unknown` refused for bundled plugins.
//!
//! The parse stops at the first structural error (serde); the rule families then report
//! every violation they find.

mod content;
mod files;
mod kind;
mod messages;
mod permissions;

use schemars::JsonSchema;
use semver::Version;
use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::licence::{LicenceClass, classify};
use crate::manifest::{Manifest, Platform};

pub use files::is_package_path;
pub(crate) use messages::key_ok as is_message_key;

/// The largest manifest accepted (8A Decision 1).
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// A package's files, by package path (`/`-separated, relative to the root).
pub trait PackageFiles {
    fn exists(&self, path: &str) -> bool;
    fn read(&self, path: &str) -> Option<Vec<u8>>;
}

/// Checks of contributed content that need crates above this one; `fin-node` implements them
/// over `fin-fonts` (`FontCatalog::from_json` + `validate`) and `fin-core` (`CaptionStyle`).
/// An `Err` is a human-readable reason.
pub trait ContentChecks {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String>;
    fn caption_style(&self, bytes: &[u8]) -> Result<(), String>;
}

/// Structural checks only (the file is a JSON object): for callers that link neither
/// `fin-fonts` nor `fin-core`'s styles.
pub struct StructuralChecks;

fn json_object(bytes: &[u8]) -> Result<(), String> {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(_)) => Ok(()),
        Ok(_) => Err("not a JSON object".into()),
        Err(e) => Err(format!("not JSON: {e}")),
    }
}

impl ContentChecks for StructuralChecks {
    fn font_catalog(&self, bytes: &[u8]) -> Result<(), String> {
        json_object(bytes)
    }

    fn caption_style(&self, bytes: &[u8]) -> Result<(), String> {
        json_object(bytes)
    }
}

/// Where a package comes from, which decides reserved publishers and unknown licences.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceClass {
    Bundled,
    Registry,
    /// `finisherd --plugin-dir` (developer mode, spec §5.5).
    Dev,
}

pub struct ValidateCtx<'a> {
    /// The running host's API version.
    pub host_api: Version,
    /// The running platform, when compatibility should be checked.
    pub platform: Option<Platform>,
    pub source: SourceClass,
    /// The running Node version, when a `node` runtime's range should be checked.
    pub node: Option<Version>,
    pub content_checks: &'a dyn ContentChecks,
}

/// One violation, shaped as an error `{code, message, details}`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct Invalid {
    pub code: String,
    pub message: String,
    pub details: Map<String, Value>,
}

impl Invalid {
    fn new(code: &str, message: impl Into<String>, details: Value) -> Invalid {
        let Value::Object(details) = details else {
            unreachable!("details are an object")
        };
        Invalid {
            code: code.into(),
            message: message.into(),
            details,
        }
    }

    /// `PLUGIN_MANIFEST_INVALID {path, reason}`; `path` is a JSON pointer into the manifest.
    pub(crate) fn manifest(path: &str, reason: &str, message: impl Into<String>) -> Invalid {
        Invalid::new(
            "PLUGIN_MANIFEST_INVALID",
            message,
            json!({ "path": path, "reason": reason }),
        )
    }

    pub(crate) fn with(mut self, key: &str, value: impl Into<Value>) -> Invalid {
        self.details.insert(key.into(), value.into());
        self
    }

    /// `details.reason`, when the code has one.
    pub fn reason(&self) -> Option<&str> {
        self.details.get("reason").and_then(Value::as_str)
    }

    /// `details.path`, when the code has one.
    pub fn path(&self) -> Option<&str> {
        self.details.get("path").and_then(Value::as_str)
    }
}

/// Validates `manifest_json` (the bytes of `finisher-plugin.json`) against the package's
/// files and the running host.
pub fn validate(
    manifest_json: &[u8],
    files: &dyn PackageFiles,
    ctx: &ValidateCtx,
) -> Result<Manifest, Vec<Invalid>> {
    if manifest_json.len() > MAX_MANIFEST_BYTES {
        return Err(vec![Invalid::new(
            "MANIFEST_TOO_LARGE",
            format!(
                "the manifest is {} bytes; at most {MAX_MANIFEST_BYTES}",
                manifest_json.len()
            ),
            json!({ "bytes": manifest_json.len(), "max": MAX_MANIFEST_BYTES }),
        )]);
    }
    let text = std::str::from_utf8(manifest_json).map_err(|e| {
        vec![Invalid::manifest(
            "",
            "utf8",
            format!("the manifest is not UTF-8: {e}"),
        )]
    })?;
    if text.starts_with('\u{feff}') {
        return Err(vec![Invalid::manifest(
            "",
            "utf8",
            "the manifest starts with a byte-order mark",
        )]);
    }
    let manifest: Manifest = serde_json::from_str(text)
        .map_err(|e| vec![Invalid::manifest("", "schema", e.to_string())])?;
    if manifest.manifest_version != 1 {
        return Err(vec![Invalid::manifest(
            "/manifestVersion",
            "manifestVersion",
            format!(
                "manifestVersion {} is not supported (1)",
                manifest.manifest_version
            ),
        )]);
    }
    let mut out = Vec::new();
    kind::check(&manifest, ctx, &mut out);
    permissions::check(&manifest, &mut out);
    licence(&manifest, ctx, &mut out);
    files::check(&manifest, files, &mut out);
    messages::check(&manifest, files, &mut out);
    content::check(&manifest, files, ctx, &mut out);
    if out.is_empty() {
        Ok(manifest)
    } else {
        Err(out)
    }
}

fn licence(m: &Manifest, ctx: &ValidateCtx, out: &mut Vec<Invalid>) {
    let class = classify(&m.license);
    let refuse = match class {
        LicenceClass::NonCommercial => Some("licenceNonCommercial"),
        LicenceClass::Unknown if ctx.source == SourceClass::Bundled => Some("licenceUnknown"),
        _ => None,
    };
    if let Some(reason) = refuse {
        out.push(
            Invalid::manifest(
                "/license",
                reason,
                format!("licence {:?} is not accepted here ({class:?})", m.license),
            )
            .with("license", m.license.clone()),
        );
    }
    for (i, model) in m.models.iter().enumerate() {
        if classify(&model.license) == LicenceClass::NonCommercial {
            out.push(Invalid::manifest(
                &format!("/models/{i}/license"),
                "licenceNonCommercial",
                format!("model {:?} has a non-commercial licence", model.id),
            ));
        }
    }
}
