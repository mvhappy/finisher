//! The job inputs `fin-node` writes (spec §4.2): `in/snapshot.json` and `in/manifest.json`.

use fin_core::{AssetKind, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::proxy::{ProxyCodec, ProxyTier};

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
    /// The proxies the media cache holds for this asset's fingerprint when the manifest was
    /// written (spec §4.2 "from 6B"; plan 6B-1 Task 16), pinned for the job's life by the host.
    /// A layer reads the `1080` one when `media.look` does not ask for originals
    /// ([`ManifestAsset::proxy`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proxies: Vec<ManifestProxy>,
}

impl ManifestAsset {
    /// The proxy of `tier`, if the manifest lists one.
    pub fn proxy(&self, tier: ProxyTier) -> Option<&ManifestProxy> {
        self.proxies.iter().find(|p| p.tier == tier)
    }
}

/// One proxy entry of the media cache (`cache/media/<key[0..2]>/<key>/`): `proxy.mp4`,
/// `table.bin` (the original's sample table) and `meta.json` (plan 6B-1 Decision 7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ManifestProxy {
    pub tier: ProxyTier,
    /// The entry's cache key.
    pub key: String,
    /// The entry's directory on this machine, absolute.
    pub dir: String,
    pub codec: ProxyCodec,
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
