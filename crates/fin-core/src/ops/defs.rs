//! The `Op` enum and its payload and helper types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::model::{
    AssetKind, AudioInfo, Curve, FontFile, ImageInfo, Item, ItemBody, LottieInfo, LutInfo,
    TransitionKind, VideoInfo,
};
use crate::ops::asset_move::AssetMove;
use crate::ops::bin_add::BinAdd;
use crate::ops::bin_move::BinMove;
use crate::ops::bin_remove::BinRemove;
use crate::ops::marker_add::MarkerAdd;
use crate::ops::marker_remove::MarkerRemove;
use crate::ops::marker_set::MarkerSet;
use crate::ops::timeline_add::TimelineAdd;
use crate::ops::timeline_remove::TimelineRemove;
use crate::ops::timeline_restore::TimelineRestore;
use crate::ops::timeline_set::TimelineSet;
use crate::ops::track_add::TrackAdd;
use crate::ops::track_move::TrackMove;
use crate::ops::track_remove::TrackRemove;
use crate::ops::track_restore::TrackRestore;
use crate::ops::track_set::TrackSet;
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
    /// Moves an item's source under it (`sourceIn`), keeping its timeline place.
    #[serde(rename = "item.slip")]
    ItemSlip(ItemSlip),
    /// Cuts an item in two at `offset`; the right part gets a new id.
    #[serde(rename = "item.split")]
    ItemSplit(ItemSplit),
    /// Moves an item (and its link group) to a track and position.
    #[serde(rename = "item.move")]
    ItemMove(ItemMove),
    /// Moves an item between its neighbours, trimming them.
    #[serde(rename = "item.slide")]
    ItemSlide(ItemSlide),
    /// Makes items one link group.
    #[serde(rename = "item.link")]
    ItemLink(ItemLink),
    /// Dissolves a link group.
    #[serde(rename = "item.unlink")]
    ItemUnlink(ItemUnlink),
    /// Creates or updates the transition at a cut.
    #[serde(rename = "transition.set")]
    TransitionSet(TransitionSet),
    /// Takes a transition off its track.
    #[serde(rename = "transition.remove")]
    TransitionRemove(TransitionRemove),
    /// Adds an empty track to a timeline.
    #[serde(rename = "track.add")]
    TrackAdd(TrackAdd),
    /// Takes a track off its timeline (with its items when `withItems`).
    #[serde(rename = "track.remove")]
    TrackRemove(TrackRemove),
    /// Moves a track to another place in its timeline's compositing order.
    #[serde(rename = "track.move")]
    TrackMove(TrackMove),
    /// Sets (or, with no `value`, resets) one setting of a track.
    #[serde(rename = "track.set")]
    TrackSet(TrackSet),
    /// Adds an empty timeline.
    #[serde(rename = "timeline.add")]
    TimelineAdd(TimelineAdd),
    /// Sets (or, with no `value`, resets) one setting of a timeline.
    #[serde(rename = "timeline.set")]
    TimelineSet(TimelineSet),
    /// Takes a timeline off the project with everything on it.
    #[serde(rename = "timeline.remove")]
    TimelineRemove(TimelineRemove),
    /// Adds a marker to a timeline.
    #[serde(rename = "marker.add")]
    MarkerAdd(MarkerAdd),
    /// Sets (or, with no `value`, clears) one field of a marker.
    #[serde(rename = "marker.set")]
    MarkerSet(MarkerSet),
    /// Takes a marker off its timeline.
    #[serde(rename = "marker.remove")]
    MarkerRemove(MarkerRemove),
    /// Adds a bin.
    #[serde(rename = "bin.add")]
    BinAdd(BinAdd),
    /// Puts a bin inside another one, or at the top.
    #[serde(rename = "bin.move")]
    BinMove(BinMove),
    /// Takes an empty bin off the project.
    #[serde(rename = "bin.remove")]
    BinRemove(BinRemove),
    /// Puts an asset in a bin, or at the top.
    #[serde(rename = "asset.move")]
    AssetMove(AssetMove),
    /// Internal (inverse of `track.remove`): puts a whole track back.
    #[serde(rename = "track.restore")]
    TrackRestore(Box<TrackRestore>),
    /// Internal (inverse of `timeline.remove`): puts a whole timeline back.
    #[serde(rename = "timeline.restore")]
    TimelineRestore(Box<TimelineRestore>),
    /// Removes an unused asset (also the inverse of `asset.add`).
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
/// track by the item's duration; `overwrite` takes the range over, removing the items inside it
/// and trimming the ones overlapping its ends (an item strictly containing it is
/// `OVERWRITE_SPLITS_ITEM`: split it first, `compose::overwrite`), and moves nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum InsertMode {
    Ripple,
    Overwrite,
}

/// How a delete leaves its range: `ripple` closes the gap, `lift` leaves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum DeleteMode {
    Ripple,
    Lift,
}

/// How a trim treats the rest of the track: `ripple` moves every later item by the change;
/// `roll` moves the cut between the item and the neighbour touching that edge (both change,
/// nothing else moves; `NOT_ADJACENT` without one); `overwrite` moves the edge itself, carving
/// whatever it extends over and leaving a gap when it shrinks; `strict` (internal: the inverse
/// of carving edits) moves the edge only, and an overlap it would make is `CLIP_OVERLAP` in
/// every apply mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum TrimMode {
    Ripple,
    Roll,
    Overwrite,
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
    /// Internal (every trim's inverse): this item alone, not its link group (plan 4C
    /// Decision 19). A `strict` trim is always alone.
    #[serde(default, skip_serializing_if = "is_false")]
    pub alone: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// `item.move`: moves `item` to `track` at `at` (a position in the frame before the op, plan
/// 4C Decision 15), and every member of its link group by the same time on its own track
/// (only the named item changes track). `ripple` closes the gap it leaves and opens room at
/// the target on each track; `overwrite` lifts it and carves the target range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemMove {
    pub item: String,
    pub track: String,
    pub at: Rational,
    pub mode: InsertMode,
}

/// `item.slide`: moves `item` (alone) by `delta` seconds on its track, trimming the
/// neighbour touching each side so nothing else moves: the one before it at its end, the one
/// after it at its start. A side with no touching neighbour needs room in the gap
/// (`SLIDE_BLOCKED`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemSlide {
    pub item: String,
    pub delta: Rational,
    // Internal (a slide's inverse): the neighbours to trim, named instead of found by touch,
    // so an undo trims exactly what the slide trimmed. Not a doc comment: a `$ref` with a
    // sibling description would be inlined in the generated TypeScript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "SlideSides")]
    pub sides: Option<SlideSides>,
}

/// The neighbours an internal `item.slide` trims; a side without one trims nothing there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SlideSides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub before: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub after: Option<String>,
}

/// `item.link`: makes `items` (at least two, on one timeline, none linked yet) one link group
/// named `id` (a `$name` placeholder). Internal applies (inverses) may name a literal id: a new
/// group (the inverse of `item.unlink`, one member included), or an existing one the items
/// join (the undo of a removal that left a member alone, 4C Tasks 9–10 review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemLink {
    pub items: Vec<String>,
    pub id: String,
}

/// `transition.set`: the transition at the cut between `between[0]` (outgoing) and
/// `between[1]` (incoming), touching items of `track`, centred on the cut. `id` is a `$name`
/// placeholder to create one, or the literal id of the one to update (a literal id naming no
/// transition restores it: internal only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransitionSet {
    pub id: String,
    pub track: String,
    pub between: [String; 2],
    pub kind: TransitionKind,
    // Seconds, centred on the cut.
    pub duration: Rational,
    #[serde(default, skip_serializing_if = "is_linear")]
    pub curve: Curve,
}

fn is_linear(c: &Curve) -> bool {
    *c == Curve::Linear
}

/// `transition.remove`: takes transition `transition` off its track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransitionRemove {
    pub transition: String,
}

/// `item.unlink`: dissolves link group `group` (internal, with `items`: only those members
/// leave it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemUnlink {
    pub group: String,
    // Internal (the inverse of an `item.link` that joined items to an existing group): these
    // members only leave it. Not a doc comment: a `$ref`-free sibling keeps the TS plain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub items: Option<Vec<String>>,
}

/// `item.slip`: moves the source of `item` (a clip or nested item) under it by `delta`
/// source seconds (`sourceIn += delta`); its place on the timeline and its item-relative times
/// stay. The source range is policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemSlip {
    pub item: String,
    pub delta: Rational,
}

/// `item.split`: cuts `item` at `offset` (item-relative seconds, strictly inside it). The left
/// part keeps the id and ends at `offset`; the right part is a copy (effects keep their ids,
/// item-relative times move by `−offset`, no link group) named by `id`, a `$name` placeholder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ItemSplit {
    pub item: String,
    pub offset: Rational,
    pub id: String,
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

impl From<ItemSlip> for Op {
    fn from(op: ItemSlip) -> Op {
        Op::ItemSlip(op)
    }
}

impl From<ItemSplit> for Op {
    fn from(op: ItemSplit) -> Op {
        Op::ItemSplit(op)
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

impl From<ItemMove> for Op {
    fn from(op: ItemMove) -> Op {
        Op::ItemMove(op)
    }
}

impl From<ItemSlide> for Op {
    fn from(op: ItemSlide) -> Op {
        Op::ItemSlide(op)
    }
}

impl From<ItemLink> for Op {
    fn from(op: ItemLink) -> Op {
        Op::ItemLink(op)
    }
}

impl From<ItemUnlink> for Op {
    fn from(op: ItemUnlink) -> Op {
        Op::ItemUnlink(op)
    }
}

impl From<TransitionSet> for Op {
    fn from(op: TransitionSet) -> Op {
        Op::TransitionSet(op)
    }
}

impl From<TransitionRemove> for Op {
    fn from(op: TransitionRemove) -> Op {
        Op::TransitionRemove(op)
    }
}
