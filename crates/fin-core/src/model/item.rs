//! Items on tracks: a kind-specific body plus the fields every kind shares.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::CoreError;
use crate::model::caption::CaptionBlock;
use crate::model::{is_default, is_false, is_one, is_one_time, is_true, is_zero, is_zero_time};
use crate::time::Rational;

/// One item on a track. On the wire the body's `kind` and fields and the common fields sit
/// next to `id`, `start` and `duration` (both are flattened).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    // Position on the timeline, in seconds.
    pub start: Rational,
    // Length on the timeline, in seconds.
    pub duration: Rational,
    #[serde(flatten)]
    pub body: ItemBody,
    #[serde(flatten)]
    pub common: Common,
}

/// What an item is, tagged by `kind`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ItemBody {
    /// A span of a media asset. Source time = `sourceIn + (t − start) × speed`.
    Clip {
        asset: String,
        // Offset into the asset, in seconds.
        source_in: Rational,
        #[serde(default = "one_time", skip_serializing_if = "is_one_time")]
        speed: Rational,
    },
    /// Timed words in a caption style. (A struct variant with the block flattened, so the
    /// generated TypeScript keeps the `kind` tag next to the block's fields.)
    Caption {
        #[serde(flatten)]
        block: CaptionBlock,
    },
    /// A Lottie template with its slot values.
    Graphic {
        asset: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        slots: BTreeMap<String, ParamValue>,
    },
    /// A still image or SVG.
    Image { asset: String },
    /// Applies its effects to everything below it.
    Adjustment {},
    /// Another timeline of the project.
    Nested {
        timeline: String,
        // Offset into the nested timeline, in seconds.
        source_in: Rational,
    },
}

fn one_time() -> Rational {
    Rational::from_int(1)
}

/// The fields every item kind has; each is omitted from the JSON at its default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Common {
    #[serde(default, skip_serializing_if = "Transform::is_default")]
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "Crop::is_default")]
    pub crop: Crop,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub opacity: f64,
    #[serde(default, skip_serializing_if = "is_default")]
    pub blend: BlendMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<Effect>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Mask")]
    pub mask: Option<Mask>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "AudioParams")]
    pub audio: Option<AudioParams>,
    /// Keyframes by registry path, each list sorted by `at`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub keyframes: BTreeMap<String, Vec<Keyframe>>,
    /// The link group (`lk<n>`) the item belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub link: Option<String>,
}

fn one() -> f64 {
    1.0
}

impl Default for Common {
    fn default() -> Self {
        Common {
            transform: Transform::default(),
            crop: Crop::default(),
            opacity: 1.0,
            blend: BlendMode::Normal,
            effects: Vec::new(),
            mask: None,
            audio: None,
            keyframes: BTreeMap::new(),
            link: None,
        }
    }
}

impl Common {
    /// Every field at its default (floats compared bit for bit, so `-0.0` is not a default).
    pub fn is_default(&self) -> bool {
        self.transform.is_default()
            && self.crop.is_default()
            && is_one(&self.opacity)
            && self.blend == BlendMode::Normal
            && self.effects.is_empty()
            && self.mask.is_none()
            && self.audio.is_none()
            && self.keyframes.is_empty()
            && self.link.is_none()
    }
}

fn same(a: [f64; 2], b: [f64; 2]) -> bool {
    a[0].to_bits() == b[0].to_bits() && a[1].to_bits() == b[1].to_bits()
}

fn unit() -> [f64; 2] {
    [1.0, 1.0]
}

fn centre() -> [f64; 2] {
    [0.5, 0.5]
}

fn origin(v: &[f64; 2]) -> bool {
    same(*v, [0.0, 0.0])
}

fn is_unit(v: &[f64; 2]) -> bool {
    same(*v, unit())
}

fn is_centre(v: &[f64; 2]) -> bool {
    same(*v, centre())
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Transform {
    /// Pixels from the frame centre.
    #[serde(default, skip_serializing_if = "origin")]
    pub position: [f64; 2],
    #[serde(default = "unit", skip_serializing_if = "is_unit")]
    pub scale: [f64; 2],
    /// Degrees.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub rotation: f64,
    /// Item-normalised.
    #[serde(default = "centre", skip_serializing_if = "is_centre")]
    pub anchor: [f64; 2],
}

impl Default for Transform {
    fn default() -> Self {
        Transform {
            position: [0.0, 0.0],
            scale: unit(),
            rotation: 0.0,
            anchor: centre(),
        }
    }
}

impl Transform {
    pub fn is_default(&self) -> bool {
        origin(&self.position)
            && is_unit(&self.scale)
            && is_zero(&self.rotation)
            && is_centre(&self.anchor)
    }
}

/// Fractions of the item's size cut from each side.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Crop {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub left: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub right: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub top: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub bottom: f64,
}

impl Crop {
    pub fn is_default(&self) -> bool {
        is_zero(&self.left) && is_zero(&self.right) && is_zero(&self.top) && is_zero(&self.bottom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Add,
    Darken,
    Lighten,
}

/// One effect of an item (`fx<n>`, addressed as `(item, effect)`); `params` is sparse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Effect {
    pub id: String,
    pub kind: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, ParamValue>,
}

fn yes() -> bool {
    true
}

impl Effect {
    /// Every id the effect carries, mutably: its own and the assets its params name.
    pub fn refs_mut(&mut self) -> Vec<&mut String> {
        let mut out = vec![&mut self.id];
        out.extend(self.params.values_mut().filter_map(ParamValue::asset_mut));
        out
    }

    /// The ids [`Effect::refs_mut`] lists.
    pub fn ids(&self) -> Vec<&str> {
        let mut out = vec![self.id.as_str()];
        out.extend(self.params.values().filter_map(ParamValue::asset));
        out
    }
}

/// A matte (an artifact reference) that masks the item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Mask {
    pub matte: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub feather: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub choke: f64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub invert: bool,
}

/// The item's own audio: gain in dB, pan in −1…1, fades in seconds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AudioParams {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub gain: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pan: f64,
    #[serde(default, skip_serializing_if = "is_zero_time")]
    pub fade_in: Rational,
    #[serde(default, skip_serializing_if = "is_zero_time")]
    pub fade_out: Rational,
}

/// A keyframe; `at` is relative to the item's start (plan 4C Decision 13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Keyframe {
    pub at: Rational,
    pub value: ParamValue,
    pub interp: Interp,
}

/// How a keyframe's value moves to the next one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Interp {
    Hold,
    Linear,
    /// CSS `cubic-bezier(x1, y1, x2, y2)`.
    Bezier {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
    },
}

/// A parameter value: finite numbers round-trip bit for bit through stored JSON (serde_json's
/// `float_roundtrip`). Times, rationals, colours (`#rrggbb[aa]`) and enum values are text; an
/// asset reference is `{"asset": id}`, so every id a value carries is found without the
/// registry (plan 4C Decision 24).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ParamValue {
    Bool(bool),
    Number(f64),
    Vec2([f64; 2]),
    Text(String),
    Asset { asset: String },
}

impl ParamValue {
    /// The asset id the value names, mutably.
    pub fn asset_mut(&mut self) -> Option<&mut String> {
        match self {
            ParamValue::Asset { asset } => Some(asset),
            _ => None,
        }
    }

    /// The asset id the value names.
    pub fn asset(&self) -> Option<&str> {
        match self {
            ParamValue::Asset { asset } => Some(asset),
            _ => None,
        }
    }
}

impl Item {
    /// An item of `body` with default common fields.
    pub fn new(id: impl Into<String>, start: Rational, duration: Rational, body: ItemBody) -> Item {
        Item {
            id: id.into(),
            start,
            duration,
            body,
            common: Common::default(),
        }
    }

    /// A clip item at speed 1 with default common fields.
    pub fn clip(
        id: impl Into<String>,
        asset: impl Into<String>,
        start: Rational,
        source_in: Rational,
        duration: Rational,
    ) -> Item {
        let body = ItemBody::Clip {
            asset: asset.into(),
            source_in,
            speed: one_time(),
        };
        Item::new(id, start, duration, body)
    }

    /// Every id the item carries (plan 4C Decision 24), mutably: its own, what its body
    /// names (an asset, a nested timeline, a caption style, the assets its slots name), its
    /// effects' (with the assets their params name) and its link group.
    pub fn refs_mut(&mut self) -> Vec<&mut String> {
        let mut out = vec![&mut self.id];
        out.extend(self.body.refs_mut());
        for e in &mut self.common.effects {
            out.extend(e.refs_mut());
        }
        out.extend(self.common.link.as_mut());
        out
    }

    /// The ids [`Item::refs_mut`] lists.
    pub fn ids(&self) -> Vec<&str> {
        let mut out = vec![self.id.as_str()];
        out.extend(self.body.ids());
        for e in &self.common.effects {
            out.extend(e.ids());
        }
        out.extend(self.common.link.as_deref());
        out
    }

    /// The end on the timeline (`start + duration`).
    pub fn end(&self) -> Result<Rational, CoreError> {
        self.start.checked_add(self.duration)
    }

    /// The asset the item shows, for clips, graphics and images.
    pub fn asset(&self) -> Option<&str> {
        self.body.asset()
    }

    /// The source offset, for clips and nested items.
    pub fn source_in(&self) -> Option<Rational> {
        match &self.body {
            ItemBody::Clip { source_in, .. } | ItemBody::Nested { source_in, .. } => {
                Some(*source_in)
            }
            _ => None,
        }
    }

    /// The wire `kind` (`"clip"`, `"caption"`, ...).
    pub fn kind_name(&self) -> &'static str {
        self.body.kind_name()
    }
}

impl ItemBody {
    /// The asset the body shows, for clips, graphics and images.
    pub fn asset(&self) -> Option<&str> {
        match self {
            ItemBody::Clip { asset, .. }
            | ItemBody::Graphic { asset, .. }
            | ItemBody::Image { asset } => Some(asset),
            _ => None,
        }
    }

    /// The wire `kind` (`"clip"`, `"caption"`, ...).
    pub fn kind_name(&self) -> &'static str {
        match self {
            ItemBody::Clip { .. } => "clip",
            ItemBody::Caption { .. } => "caption",
            ItemBody::Graphic { .. } => "graphic",
            ItemBody::Image { .. } => "image",
            ItemBody::Adjustment {} => "adjustment",
            ItemBody::Nested { .. } => "nested",
        }
    }

    /// Every id the body names (an asset, a nested timeline, a caption style, the assets a
    /// graphic's slots name), mutably.
    pub fn refs_mut(&mut self) -> Vec<&mut String> {
        match self {
            ItemBody::Graphic { asset, slots } => {
                let mut out = vec![asset];
                out.extend(slots.values_mut().filter_map(ParamValue::asset_mut));
                out
            }
            ItemBody::Clip { asset, .. } | ItemBody::Image { asset } => vec![asset],
            ItemBody::Nested { timeline, .. } => vec![timeline],
            ItemBody::Caption { block } => vec![&mut block.style],
            ItemBody::Adjustment {} => Vec::new(),
        }
    }

    /// The ids [`ItemBody::refs_mut`] lists.
    pub fn ids(&self) -> Vec<&str> {
        match self {
            ItemBody::Graphic { asset, slots } => {
                let mut out = vec![asset.as_str()];
                out.extend(slots.values().filter_map(ParamValue::asset));
                out
            }
            ItemBody::Clip { asset, .. } | ItemBody::Image { asset } => vec![asset],
            ItemBody::Nested { timeline, .. } => vec![timeline],
            ItemBody::Caption { block } => vec![&block.style],
            ItemBody::Adjustment {} => Vec::new(),
        }
    }
}
