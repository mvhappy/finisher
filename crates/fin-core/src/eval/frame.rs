//! `eval_frame`: the layers of a timeline at one time (sub-project 4 spec §5.4, plan 4C
//! Decision 41).
//!
//! - Video and caption tracks draw, bottom first; a muted one is hidden; audio tracks never
//!   draw (and `solo` only affects audio).
//! - An item is shown for `start ≤ t < start + duration`, extended by its transitions.
//! - A nested item is a group layer evaluated at `sourceIn + local` on its timeline (a cycle,
//!   or more than [`MAX_NESTING`] timelines deep, is `NESTED_CYCLE`; more than
//!   [`MAX_NESTED_LAYERS`] layers inside nested items in all is `NESTED_TOO_LARGE`). A nested
//!   item whose timeline is missing draws nothing (the rest of the frame still does).
//! - Each track is indexed once per evaluated timeline ([`TrackIndex`]), so a frame costs the
//!   items it meets plus one linear pass per track.

use std::collections::HashMap;
use std::rc::Rc;

use serde_json::json;

use crate::error::CoreError;
use crate::eval::caption::caption_plan;
use crate::eval::plan::{
    FramePlan, GraphicPlan, ItemKind, Layer, NestedPlan, PlanCrop, PlanTransform,
};
use crate::eval::resolve::Resolver;
use crate::eval::source::source_of;
use crate::eval::transition::TrackIndex;
use crate::eval::{check_frame, timeline_of};
use crate::layout::TextMeasurer;
use crate::model::{Item, ItemBody, Project, Resolution, Timeline, TrackKind};
use crate::time::Rational;

/// How many timelines deep evaluation may go, the evaluated (root) timeline counted: 16 allows
/// 15 levels of nested items.
pub const MAX_NESTING: usize = 16;

/// How many layers nested items may hold in all, at every level of one frame (a timeline
/// nested many times over, each holding many items, cannot make a frame unbounded).
pub const MAX_NESTED_LAYERS: usize = 10_000;

/// The plan of `timeline` at `t`. `m` measures caption text; without one, captions are
/// unmeasured (`noMeasurer`). Errors: `NOT_FOUND` (the timeline), `NESTED_CYCLE`,
/// `NESTED_TOO_LARGE`, `TIME_OVERFLOW`, and `INTERNAL` should a value come out non-finite.
pub fn eval_frame(
    project: &Project,
    timeline: &str,
    t: Rational,
    m: Option<&dyn TextMeasurer>,
) -> Result<FramePlan, CoreError> {
    let tl = timeline_of(project, timeline)?;
    let mut ctx = Ctx {
        project,
        m,
        stack: Vec::new(),
        measured: false,
        nested_layers: 0,
        index: HashMap::new(),
    };
    let layers = ctx.layers(tl, t)?;
    let plan = FramePlan {
        timeline: tl.id.clone(),
        t,
        resolution: tl.resolution,
        layers,
        text_engine: m.filter(|_| ctx.measured).map(|m| m.fingerprint()),
    };
    check_frame(&plan)?;
    Ok(plan)
}

struct Ctx<'a> {
    project: &'a Project,
    m: Option<&'a dyn TextMeasurer>,
    /// The timelines being evaluated, outermost first.
    stack: Vec<&'a str>,
    measured: bool,
    /// Layers inside nested items so far ([`MAX_NESTED_LAYERS`]).
    nested_layers: usize,
    /// Each evaluated timeline's tracks, indexed once per frame.
    index: HashMap<&'a str, Rc<Vec<TrackIndex<'a>>>>,
}

impl<'a> Ctx<'a> {
    fn tracks(&mut self, tl: &'a Timeline) -> Result<Rc<Vec<TrackIndex<'a>>>, CoreError> {
        if let Some(i) = self.index.get(tl.id.as_str()) {
            return Ok(Rc::clone(i));
        }
        let built: Vec<TrackIndex<'a>> = tl
            .tracks
            .iter()
            .filter(|t| t.kind != TrackKind::Audio && !t.mute)
            .map(TrackIndex::new)
            .collect::<Result<_, _>>()?;
        let built = Rc::new(built);
        self.index.insert(&tl.id, Rc::clone(&built));
        Ok(built)
    }

    fn layers(&mut self, tl: &'a Timeline, t: Rational) -> Result<Vec<Layer>, CoreError> {
        if self.stack.contains(&tl.id.as_str()) || self.stack.len() >= MAX_NESTING {
            return Err(nested_cycle(&self.stack, &tl.id));
        }
        let nested = !self.stack.is_empty();
        self.stack.push(&tl.id);
        let mut out = Vec::new();
        for index in self.tracks(tl)?.iter() {
            for (item, transition) in index.active(t)? {
                if nested {
                    self.nested_layers += 1;
                    if self.nested_layers > MAX_NESTED_LAYERS {
                        return Err(nested_too_large(&self.stack));
                    }
                }
                let local = t.checked_sub(item.start)?;
                let Some(mut layer) = self.layer(item, &index.track.id, local, tl.resolution)?
                else {
                    continue;
                };
                layer.transition = transition;
                out.push(layer);
            }
        }
        self.stack.pop();
        Ok(out)
    }

    /// The layer of `item` at `local` on a timeline of `resolution`; `None` for a nested item
    /// whose timeline is missing.
    fn layer(
        &mut self,
        item: &'a Item,
        track: &str,
        local: Rational,
        resolution: Resolution,
    ) -> Result<Option<Layer>, CoreError> {
        let r = Resolver { item, local };
        let mut layer = Layer {
            item: item.id.clone(),
            kind: kind_of(&item.body),
            track: track.to_string(),
            local,
            source: source_of(item, local)?,
            transform: r.transform(),
            crop: r.crop(),
            opacity: r.opacity(),
            blend: item.common.blend,
            effects: r.effects(),
            mask: r.mask(),
            transition: None,
            caption: None,
            graphic: None,
            nested: None,
            children: None,
        };
        match &item.body {
            ItemBody::Clip { .. } | ItemBody::Image { .. } => {}
            ItemBody::Adjustment {} => {
                layer.transform = PlanTransform::default();
                layer.crop = PlanCrop::default();
            }
            ItemBody::Caption { block } => {
                let plan = caption_plan(self.project, item, block, local, resolution, self.m);
                self.measured |= plan.layout.is_some();
                layer.caption = Some(plan);
            }
            ItemBody::Graphic { asset, slots } => {
                let template = self
                    .project
                    .assets
                    .get(asset)
                    .and_then(|a| a.lottie.as_ref());
                layer.graphic = Some(GraphicPlan {
                    asset: asset.clone(),
                    time: local,
                    slots: r.slots(slots, template),
                });
            }
            ItemBody::Nested {
                timeline,
                source_in,
            } => {
                let Ok(tl) = timeline_of(self.project, timeline) else {
                    return Ok(None);
                };
                let time = source_in.checked_add(local)?;
                layer.children = Some(self.layers(tl, time)?);
                layer.nested = Some(NestedPlan {
                    timeline: tl.id.clone(),
                    time,
                    resolution: tl.resolution,
                });
            }
        }
        Ok(Some(layer))
    }
}

pub(crate) fn kind_of(body: &ItemBody) -> ItemKind {
    match body {
        ItemBody::Clip { .. } => ItemKind::Clip,
        ItemBody::Caption { .. } => ItemKind::Caption,
        ItemBody::Graphic { .. } => ItemKind::Graphic,
        ItemBody::Image { .. } => ItemKind::Image,
        ItemBody::Adjustment {} => ItemKind::Adjustment,
        ItemBody::Nested { .. } => ItemKind::Nested,
    }
}

/// `NESTED_TOO_LARGE`: nested items hold more than [`MAX_NESTED_LAYERS`] layers in all.
pub(crate) fn nested_too_large(stack: &[&str]) -> CoreError {
    CoreError::new(
        "NESTED_TOO_LARGE",
        format!("nested items hold more than {MAX_NESTED_LAYERS} layers at once"),
    )
    .with_details(json!({ "timelines": stack, "max": MAX_NESTED_LAYERS }))
    .with_fix("nest fewer items, or a timeline with fewer items at this time")
}

pub(crate) fn nested_cycle(stack: &[&str], next: &str) -> CoreError {
    let mut path: Vec<&str> = stack.to_vec();
    path.push(next);
    let message = if stack.contains(&next) {
        format!("timeline {next} nests itself ({})", path.join(" → "))
    } else {
        format!("timelines nest more than {MAX_NESTING} deep (the root counted)")
    };
    CoreError::new("NESTED_CYCLE", message)
        .with_details(json!({ "timelines": path, "maxDepth": MAX_NESTING }))
        .with_fix("make a nested item point at a timeline that does not contain it")
}
