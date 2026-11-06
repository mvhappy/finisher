//! The `Op` enum and its helper types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{AudioInfo, Clip, VideoInfo};
use crate::time::Rational;

/// A primitive edit operation, tagged by `op`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all_fields = "camelCase")]
pub enum Op {
    /// Adds a media asset. `id` is a `$name` placeholder bound to a new `a<n>` id.
    #[serde(rename = "asset.add")]
    AssetAdd {
        id: String,
        path: String,
        // Asset length in seconds; must be positive.
        duration: Rational,
        video: Option<VideoInfo>,
        audio: Option<AudioInfo>,
        fingerprint: String,
    },
    /// Inserts a clip at `at` on `track`, rippling later clips by `duration`. `id` is a `$name`
    /// placeholder; `asset` is an asset id or a placeholder bound earlier in the transaction.
    #[serde(rename = "clip.insert")]
    ClipInsert {
        id: String,
        track: String,
        // Timeline position in seconds: a clip boundary, a gap, or at/after the track end.
        at: Rational,
        asset: String,
        // Offset into the asset, in seconds.
        source_in: Rational,
        // Clip length in seconds; at least one frame.
        duration: Rational,
    },
    /// Moves one edge of a clip by `delta` seconds, rippling later clips on the track.
    #[serde(rename = "clip.trim")]
    ClipTrim {
        clip: String,
        edge: Edge,
        // Seconds; positive moves the edge later.
        delta: Rational,
    },
    /// Removes a clip and closes the gap (ripple).
    #[serde(rename = "clip.delete")]
    ClipDelete { clip: String },
    /// Internal (inverse of `asset.add`): removes an unused asset.
    #[serde(rename = "asset.remove")]
    AssetRemove { id: String },
    /// Internal (inverse of `clip.insert`): removes a clip, rippling when `ripple`.
    #[serde(rename = "clip.remove")]
    ClipRemove { id: String, ripple: bool },
    /// Internal (inverse of `clip.delete`/`clip.remove`): puts `clip` back at its `start`,
    /// rippling later clips by its duration when `ripple`.
    #[serde(rename = "clip.restore")]
    ClipRestore {
        // Boxed: a clip with its common fields is several times the size of the other ops.
        clip: Box<Clip>,
        track: String,
        #[serde(default = "yes")]
        ripple: bool,
    },
}

fn yes() -> bool {
    true
}

/// Which edge of a clip a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Start,
    End,
}
