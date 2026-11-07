//! `eval_frame`: the layers of a timeline at one time (sub-project 4 spec §5.4, plan 4C
//! Decision 41).
//!
//! - Video and caption tracks draw, bottom first; a muted one is hidden; audio tracks never
//!   draw (and `solo` only affects audio).
//! - An item is shown for `start ≤ t < start + duration`, extended by its transitions.
//! - A nested item is a group layer evaluated at `sourceIn + local` on its timeline (a cycle,
//!   or more than [`MAX_NESTING`] levels, is `NESTED_CYCLE`).

use serde_json::json;

use crate::error::CoreError;
use crate::eval::caption::caption_plan;
use crate::eval::plan::{FramePlan, GraphicPlan, ItemKind, Layer, NestedPlan};
use crate::eval::resolve::Resolver;
use crate::eval::source::source_of;
use crate::eval::transition::active;
use crate::eval::{check_frame, timeline_of};
use crate::layout::TextMeasurer;
use crate::model::{Crop, Item, ItemBody, Project, Timeline, TrackKind, Transform};
use crate::time::Rational;

/// How deep nested timelines may go.
pub const MAX_NESTING: usize = 16;

/// The plan of `timeline` at `t`. `m` measures caption text; without one, captions are
/// unmeasured (`noMeasurer`). Errors: `NOT_FOUND` (a timeline), `NESTED_CYCLE`,
/// `TIME_OVERFLOW`, and `INTERNAL` should a value come out non-finite.
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
}

impl<'a> Ctx<'a> {
    fn layers(&mut self, tl: &'a Timeline, t: Rational) -> Result<Vec<Layer>, CoreError> {
        if self.stack.contains(&tl.id.as_str()) || self.stack.len() >= MAX_NESTING {
            return Err(nested_cycle(&self.stack, &tl.id));
        }
        self.stack.push(&tl.id);
        let mut out = Vec::new();
        for track in &tl.tracks {
            if track.kind == TrackKind::Audio || track.mute {
                continue;
            }
            for (item, transition) in active(track, t)? {
                let mut layer = self.layer(item, &track.id, t.checked_sub(item.start)?)?;
                layer.transition = transition;
                out.push(layer);
            }
        }
        self.stack.pop();
        Ok(out)
    }

    fn layer(&mut self, item: &'a Item, track: &str, local: Rational) -> Result<Layer, CoreError> {
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
                layer.transform = Transform::default();
                layer.crop = Crop::default();
            }
            ItemBody::Caption { block } => {
                let plan = caption_plan(self.project, item, block, local, self.m);
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
                let tl = timeline_of(self.project, timeline)?;
                let time = source_in.checked_add(local)?;
                layer.children = Some(self.layers(tl, time)?);
                layer.nested = Some(NestedPlan {
                    timeline: tl.id.clone(),
                    time,
                    resolution: tl.resolution,
                });
            }
        }
        Ok(layer)
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

fn nested_cycle(stack: &[&str], next: &str) -> CoreError {
    let mut path: Vec<&str> = stack.to_vec();
    path.push(next);
    let message = if stack.contains(&next) {
        format!("timeline {next} nests itself ({})", path.join(" → "))
    } else {
        format!("timelines nest more than {MAX_NESTING} deep")
    };
    CoreError::new("NESTED_CYCLE", message)
        .with_details(json!({ "timelines": path, "maxDepth": MAX_NESTING }))
        .with_fix("make a nested item point at a timeline that does not contain it")
}
