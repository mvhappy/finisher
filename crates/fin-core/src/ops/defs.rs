//! The `Op` enum and its payload and helper types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{
    AssetKind, AudioInfo, FontFile, ImageInfo, Item, ItemBody, LottieInfo, LutInfo, VideoInfo,
};
use crate::time::Rational;

/// A primitive edit operation, tagged by `op`.
///
/// `clip.insert`, `clip.trim` and `clip.delete` are client aliases of the `item.*` ops (plan 4C
/// Decision 6): [`crate::normalize`] rewrites them before a commit, so a stored transaction
/// never holds one, and internal and replay applies refuse them (`INVALID_OP`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "op", rename_all_fields = "camelCase")]
pub enum Op {
    /// Adds an asset. `id` is a `$name` placeholder bound to a new `a<n>` id.
    #[serde(rename = "asset.add")]
    AssetAdd(Box<AssetAdd>),
    /// Places an item on a track at `at`.
    #[serde(rename = "item.insert")]
    ItemInsert(Box<ItemInsert>),
    /// Moves one edge of an item by `delta` seconds.
    #[serde(rename = "item.trim")]
    ItemTrim(ItemTrim),
    /// Takes an item off its track.
    #[serde(rename = "item.delete")]
    ItemDelete(ItemDelete),
    /// Internal (inverse of `asset.add`): removes an unused asset.
    #[serde(rename = "asset.remove")]
    AssetRemove { id: String },
    /// Internal (inverse of `item.insert`): removes an item.
    #[serde(rename = "item.remove")]
    ItemRemove(ItemRemove),
    /// Internal (inverse of `item.delete`/`item.remove`): puts an item back at its `start`.
    #[serde(rename = "item.restore")]
    ItemRestore(Box<ItemRestore>),
    /// Client alias of `item.insert` of a clip in ripple mode.
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
    /// Client alias of `item.trim` in ripple mode.
    #[serde(rename = "clip.trim")]
    ClipTrim {
        clip: String,
        edge: Edge,
        // Seconds; positive moves the edge later.
        delta: Rational,
    },
    /// Client alias of `item.delete` in ripple mode.
    #[serde(rename = "clip.delete")]
    ClipDelete { clip: String },
}

/// Which edge of an item a trim moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Edge {
    Start,
    End,
}

/// How an insert makes room (plan 4C Decision 18): `ripple` moves every later item on the
/// track by the item's duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InsertMode {
    Ripple,
}

/// How a delete leaves its range: `ripple` closes the gap, `lift` leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    Ripple,
    Lift,
}

/// How a trim treats the rest of the track: `ripple` moves every later item by the change;
/// `strict` (internal: the inverse of carving edits) moves nothing, and an overlap it would
/// make is `CLIP_OVERLAP` in every apply mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrimMode {
    Ripple,
    Strict,
}

/// `asset.add`: what probing found, by kind (plan 4C Task 7). Each kind carries its own
/// fields and none of the others': `media` a positive `duration` and its `video`/`audio`
/// streams; `image` and `svg` an `image`; `lottieTemplate` a positive `duration` and `lottie`;
/// `font` a `font` (with its whole-file SHA-256); `lut` a `lut`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssetAdd {
    pub id: String,
    /// `media` when absent: a sub-project 4B (schema 1) `asset.add` names no kind.
    #[serde(default = "media")]
    pub kind: AssetKind,
    pub path: String,
    pub fingerprint: String,
    // Length in seconds (media, animated templates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Rational")]
    pub duration: Option<Rational>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "VideoInfo")]
    pub video: Option<VideoInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AudioInfo")]
    pub audio: Option<AudioInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ImageInfo")]
    pub image: Option<ImageInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "LottieInfo")]
    pub lottie: Option<LottieInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "FontFile")]
    pub font: Option<FontFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "LutInfo")]
    pub lut: Option<LutInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub bin: Option<String>,
}

fn media() -> AssetKind {
    AssetKind::Media
}

impl AssetAdd {
    /// A media asset with the given streams and nothing else.
    pub fn media(
        id: impl Into<String>,
        path: impl Into<String>,
        duration: Rational,
        video: Option<VideoInfo>,
        audio: Option<AudioInfo>,
        fingerprint: impl Into<String>,
    ) -> AssetAdd {
        AssetAdd {
            id: id.into(),
            kind: AssetKind::Media,
            path: path.into(),
            fingerprint: fingerprint.into(),
            duration: Some(duration),
            video,
            audio,
            image: None,
            lottie: None,
            font: None,
            lut: None,
            bin: None,
        }
    }
}

/// `item.insert`: an item of any kind (the flattened `body`, tagged by `kind`) at `at` on
/// `track`, `duration` long, with default common fields. `id` is a `$name` placeholder bound
/// to a new id of the item's kind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemInsert {
    pub id: String,
    pub track: String,
    // Timeline position in seconds: an item boundary, a gap, or at/after the track end.
    pub at: Rational,
    // Length on the timeline in seconds; at least one frame.
    pub duration: Rational,
    pub mode: InsertMode,
    #[serde(flatten)]
    pub body: ItemBody,
}

/// `item.delete`: takes `item` off its track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemDelete {
    pub item: String,
    pub mode: DeleteMode,
}

/// `item.trim`: moves one edge of `item` by `delta` seconds (positive moves it later).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemTrim {
    pub item: String,
    pub edge: Edge,
    pub delta: Rational,
    pub mode: TrimMode,
}

/// `item.remove` (internal): takes item `id` off its track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemRemove {
    pub id: String,
    pub mode: DeleteMode,
}

/// `item.restore` (internal): puts `item` back on `track` at its `start`. Its id may be a
/// `$placeholder` (spec §4.3): the item then gets a newly allocated id.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemRestore {
    pub item: Item,
    pub track: String,
    pub mode: DeleteMode,
}

impl From<AssetAdd> for Op {
    fn from(op: AssetAdd) -> Op {
        Op::AssetAdd(Box::new(op))
    }
}

impl From<ItemInsert> for Op {
    fn from(op: ItemInsert) -> Op {
        Op::ItemInsert(Box::new(op))
    }
}

impl From<ItemTrim> for Op {
    fn from(op: ItemTrim) -> Op {
        Op::ItemTrim(op)
    }
}

impl From<ItemDelete> for Op {
    fn from(op: ItemDelete) -> Op {
        Op::ItemDelete(op)
    }
}

impl From<ItemRemove> for Op {
    fn from(op: ItemRemove) -> Op {
        Op::ItemRemove(op)
    }
}

impl From<ItemRestore> for Op {
    fn from(op: ItemRestore) -> Op {
        Op::ItemRestore(Box::new(op))
    }
}
