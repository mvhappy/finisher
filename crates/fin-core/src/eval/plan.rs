//! The frame plan: what the compositor draws at one time (sub-project 4 spec §5.4, plan 4C
//! Decision 41). Every value is resolved at that time; times stay rational, parameters are
//! finite `f64`s.

use std::collections::BTreeMap;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use crate::layout::TextLayout;
use crate::model::{BlendMode, Crop, Curve, Resolution, Transform, TransitionKind};
use crate::time::Rational;

/// The layers of one timeline at `t`, bottom first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FramePlan {
    pub timeline: String,
    // The evaluated time on the timeline, in seconds.
    pub t: Rational,
    pub resolution: Resolution,
    /// Bottom first: tracks in compositing order, items in time order within a track (a
    /// transition's outgoing item before its incoming one).
    pub layers: Vec<Layer>,
    /// The text engine (`TextMeasurer::fingerprint`) that produced the plan's caption layouts;
    /// absent when no caption was measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub text_engine: Option<String>,
}

/// The item kinds, as they appear in plans.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ItemKind {
    Clip,
    Caption,
    Graphic,
    Image,
    Adjustment,
    Nested,
}

/// One item drawn at the plan's time. `local` is the time from the item's start: negative for
/// a transition's incoming item before its cut, past `duration` for the outgoing one after it.
/// An adjustment layer carries effects only (its transform and crop are the defaults); a
/// nested item is a group layer whose `children` are its timeline's layers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Layer {
    pub item: String,
    pub kind: ItemKind,
    pub track: String,
    // Seconds from the item's start.
    pub local: Rational,
    /// The asset and its time: a clip's source time (through speed), an image's 0, a
    /// graphic's template time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "SourceRef")]
    pub source: Option<SourceRef>,
    pub transform: Transform,
    pub crop: Crop,
    pub opacity: f64,
    pub blend: BlendMode,
    /// Enabled effects, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "MaskPlan")]
    pub mask: Option<MaskPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "TransitionState")]
    pub transition: Option<TransitionState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "CaptionPlan")]
    pub caption: Option<CaptionPlan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "GraphicPlan")]
    pub graphic: Option<GraphicPlan>,
    /// A nested item's timeline, time and frame size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "NestedPlan")]
    pub nested: Option<NestedPlan>,
    /// A nested item's layers, bottom first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<Layer>")]
    pub children: Option<Vec<Layer>>,
}

/// An asset and the time to read from it, in seconds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceRef {
    pub asset: String,
    pub time: Rational,
}

/// One enabled effect with its parameters resolved at the plan's time. `params` holds the
/// effect's stored and keyframed parameters (the registry's defaults fill the rest).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectPlan {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, PlanValue>,
}

/// A resolved value (plan 4C Decision 12): colours are sRGB `[r, g, b, a]` in 0…1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum PlanValue {
    Bool(bool),
    Number(f64),
    Vec2([f64; 2]),
    Color([f64; 4]),
    Text(String),
}

/// The mask with its values resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MaskPlan {
    pub matte: String,
    pub feather: f64,
    pub choke: f64,
    pub invert: bool,
}

/// Which side of a transition a layer is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// The outgoing item (A).
    Out,
    /// The incoming item (B).
    In,
}

/// A layer inside a transition window `[cut − d/2, cut + d/2)`: `progress` runs 0 → 1 over it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TransitionState {
    pub id: String,
    pub kind: TransitionKind,
    pub curve: Curve,
    pub progress: f64,
    pub role: Role,
}

/// A caption's words at the plan's time, and its layout when it was measured. `layout` is
/// `null` when it was not, and `unmeasured` says why (plan 4C Decision 36).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CaptionPlan {
    /// Every word, in order (indices match the layout's word boxes).
    pub words: Vec<WordPlan>,
    pub style: String,
    #[schemars(required, schema_with = "nullable_layout")]
    pub layout: Option<TextLayout>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Unmeasured")]
    pub unmeasured: Option<Unmeasured>,
}

/// `TextLayout | null`, required: the plan always says whether a caption was measured.
fn nullable_layout(generator: &mut SchemaGenerator) -> Schema {
    let layout = generator.subschema_for::<TextLayout>();
    json_schema!({ "anyOf": [layout, { "type": "null" }] })
}

/// One word: item-relative times clamped to the item, and whether it is spoken at the plan's
/// time (`start ≤ local < end`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WordPlan {
    pub text: String,
    pub start: Rational,
    pub end: Rational,
    pub active: bool,
}

/// Why a caption has no layout, and the font keys concerned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Unmeasured {
    pub reason: UnmeasuredReason,
    pub keys: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum UnmeasuredReason {
    FontMissing,
    FontFetchFailed,
    FontUnpinned,
    NoMeasurer,
}

/// A graphic's template, its time (from the item's start) and its slot values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GraphicPlan {
    pub asset: String,
    pub time: Rational,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub slots: BTreeMap<String, PlanValue>,
}

/// The nested timeline of a group layer, the time evaluated on it (`sourceIn + local`) and
/// its frame size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NestedPlan {
    pub timeline: String,
    pub time: Rational,
    pub resolution: Resolution,
}
