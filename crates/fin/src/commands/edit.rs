//! The 4C edit commands as one write step (plan 4C Task 23): each group's subcommand, its
//! name, its hashed input (`argsHash`: the args as given, absent flags left out) and its plan.

use fin_core::CoreError;
use serde_json::{Value, json};

use super::Builder;
use super::asset::{self, AssetEdit};
use super::bin::{self, BinCmd};
use super::caption::{self, CaptionCmd};
use super::effect::{self, EffectCmd};
use super::fonts::{self, FontEdit};
use super::item::{self, ItemCmd};
use super::keyframe::{self, KeyframeCmd};
use super::marker::{self, MarkerCmd};
use super::media::{self, GraphicCmd, ImageCmd};
use super::param::{self, ParamCmd};
use super::style::{self, StyleCmd};
use super::timeline::{self, TimelineCmd};
use super::track::{self, TrackCmd};
use super::transition::{self, TransitionCmd};

#[derive(Debug, Clone)]
pub enum Edit {
    Image(ImageCmd),
    Graphic(GraphicCmd),
    Item(ItemCmd),
    Transition(TransitionCmd),
    Track(TrackCmd),
    Timeline(TimelineCmd),
    Marker(MarkerCmd),
    Bin(BinCmd),
    Asset(AssetEdit),
    /// Plan 4C Task 24: settings, captions, styles and font pins.
    Param(ParamCmd),
    Keyframe(KeyframeCmd),
    Effect(EffectCmd),
    Caption(CaptionCmd),
    Style(StyleCmd),
    Font(FontEdit),
}

impl Edit {
    /// The registry name, e.g. `item.move`.
    pub fn name(&self) -> &'static str {
        match self {
            Edit::Image(_) => "image.insert",
            Edit::Graphic(_) => "graphic.insert",
            Edit::Item(c) => item::name(c),
            Edit::Transition(c) => transition::name(c),
            Edit::Track(c) => track::name(c),
            Edit::Timeline(c) => timeline::name(c),
            Edit::Marker(c) => marker::name(c),
            Edit::Bin(c) => bin::name(c),
            Edit::Asset(c) => asset::edit_name(c),
            Edit::Param(c) => param::name(c),
            Edit::Keyframe(c) => keyframe::name(c),
            Edit::Effect(c) => effect::name(c),
            Edit::Caption(c) => caption::name(c),
            Edit::Style(c) => style::name(c),
            Edit::Font(c) => fonts::edit_name(c),
        }
    }

    /// The command input as given (each args struct leaves absent flags out).
    pub fn args(&self) -> Value {
        match self {
            Edit::Image(c) => json!(c),
            Edit::Graphic(c) => json!(c),
            Edit::Item(c) => json!(c),
            Edit::Transition(c) => json!(c),
            Edit::Track(c) => json!(c),
            Edit::Timeline(c) => json!(c),
            Edit::Marker(c) => json!(c),
            Edit::Bin(c) => json!(c),
            Edit::Asset(c) => json!(c),
            Edit::Param(c) => json!(c),
            Edit::Keyframe(c) => json!(c),
            Edit::Effect(c) => json!(c),
            Edit::Caption(c) => caption::args(c),
            Edit::Style(c) => json!(c),
            Edit::Font(c) => json!(c),
        }
    }

    /// Plans the step; `suffix` numbers its placeholders in a batch (empty for one command).
    pub fn plan(&self, b: &mut Builder<'_>, suffix: &str) -> Result<Value, CoreError> {
        match self {
            Edit::Image(ImageCmd::Insert(a)) => media::plan_image(b, a, suffix),
            Edit::Graphic(GraphicCmd::Insert(a)) => media::plan_graphic(b, a, suffix),
            Edit::Item(c) => item::plan(b, c, suffix),
            Edit::Transition(c) => transition::plan(b, c, suffix),
            Edit::Track(c) => track::plan(b, c, suffix),
            Edit::Timeline(c) => timeline::plan(b, c, suffix),
            Edit::Marker(c) => marker::plan(b, c, suffix),
            Edit::Bin(c) => bin::plan(b, c, suffix),
            Edit::Asset(c) => asset::plan_edit(b, c),
            Edit::Param(c) => param::plan(b, c),
            Edit::Keyframe(c) => keyframe::plan(b, c),
            Edit::Effect(c) => effect::plan(b, c, suffix),
            Edit::Caption(c) => caption::plan(b, c, suffix),
            Edit::Style(c) => style::plan(b, c, suffix),
            Edit::Font(c) => fonts::plan(b, c),
        }
    }
}
