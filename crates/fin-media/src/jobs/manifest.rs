//! The job inputs `fin-node` writes (spec §4.2): `in/snapshot.json` and `in/manifest.json`.

use fin_core::{AssetKind, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `in/snapshot.json` (plan 6A Decision 2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobSnapshot {
    pub branch: String,
    pub rev: u64,
    pub project: Project,
}

/// The render manifest's version.
pub const MANIFEST_VERSION: u32 = 1;

/// `in/manifest.json` (spec §4.2): built by `fin-node` from the snapshot's asset table,
/// never by the worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RenderManifest {
    /// [`MANIFEST_VERSION`].
    pub version: u32,
    /// Every asset the snapshot references, once.
    pub assets: Vec<ManifestAsset>,
    #[serde(default)]
    pub fonts: ManifestFonts,
}

impl RenderManifest {
    pub fn asset(&self, id: &str) -> Option<&ManifestAsset> {
        self.assets.iter().find(|a| a.asset == id)
    }
}

/// One asset of the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestAsset {
    pub asset: String,
    pub kind: AssetKind,
    pub fingerprint: String,
    /// The file on this machine (plan 6A Decision 5: the model's stored path), `null` when it
    /// is missing.
    pub original: Option<OriginalFile>,
}

/// A file a worker opens, with what it looked like when the manifest was written.
///
/// `path` may be absolute anywhere on the machine (the model stores the path the user gave:
/// plan 6A Decision 5) and the directories above the file may be links: **by design** only the
/// file itself is opened no-follow and checked against these facts. The manifest is the host's,
/// written from the project's own model, never from anything a worker or a client sent in the
/// job; `--replay` of a directory that is not the caller's own is the exception, and confines
/// originals to the directory (`--originals-anywhere` lifts that). The worker
/// opens it through `fin-library`'s no-follow opener, which refuses it once its size, mtime or
/// file id changed (`ORIGINALS_MISSING {reason: changed}`); `mtimeNs` and `fileId` are
/// encoded as `fin-library`'s `Expect` encodes them (a decimal string; standard base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct OriginalFile {
    /// Absolute.
    pub path: String,
    pub size: u64,
    pub mtime_ns: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub file_id: Option<String>,
}

/// Font bytes for captions (plan 6A Decision 3): the server's font cache, read-only, then the
/// project's font asset files by SHA-256.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestFonts {
    /// `<server-data>/fonts`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cache: Option<String>,
    #[serde(default)]
    pub files: Vec<FontFileRef>,
}

/// A project font asset's file and the SHA-256 its asset pins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FontFileRef {
    pub sha256: String,
    pub original: OriginalFile,
}
