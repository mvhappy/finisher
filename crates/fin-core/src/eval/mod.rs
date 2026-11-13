//! Evaluation (sub-project 4 spec §5.4): what the compositor draws at a time and what the mixer
//! plays over a range, as plans. Pure and deterministic: rational times, and `f64` limited to
//! `+ − × ÷ sqrt`, comparisons, `abs`, `min`/`max`/`clamp`, `floor` and conversions from
//! integers (plan 4C Global Constraints, gated by Task 17), so plans hash the same on every
//! platform and on wasm32. A plan never holds NaN or ∞ (`INTERNAL` if it would).

mod audio;
mod audio_effects;
mod caption;
mod frame;
mod interp;
mod plan;
mod resolve;
mod segments;
mod source;
mod transition;

pub use audio::{AudioItem, AudioPlan, GainSegment, PanSegment, Shape, ShapeKind, eval_audio};
pub use audio_effects::{AudioEffectPlan, ParamSegment};
pub use frame::{MAX_NESTED_LAYERS, MAX_NESTING, eval_frame};
pub use interp::{PRESETS, ValueType, bezier_y, colour_hex, interpolate, parse_colour};
pub use plan::{
    CaptionPlan, EffectPlan, FramePlan, GraphicPlan, ItemKind, Layer, MaskPlan, NestedPlan,
    PlanCrop, PlanTransform, PlanValue, Role, SourceRef, TransitionState, Unmeasured,
    UnmeasuredReason, WordPlan,
};

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Interp, Project, Timeline};

pub(crate) fn timeline_of<'a>(project: &'a Project, id: &str) -> Result<&'a Timeline, CoreError> {
    project
        .timelines
        .iter()
        .find(|t| t.id == id)
        .ok_or_else(|| {
            CoreError::new("NOT_FOUND", format!("timeline {id} does not exist"))
                .with_details(json!({ "id": id, "kind": "timeline" }))
                .with_fix("use the id of an existing timeline")
        })
}

/// `INTERNAL` naming the first non-finite value (`path` is a JSON-pointer-like location).
fn non_finite(path: &str) -> CoreError {
    CoreError::new(
        "INTERNAL",
        format!("evaluation produced a non-finite value at {path}"),
    )
    .with_details(json!({ "path": path }))
    .with_fix("report this project: its stored values overflow evaluation")
}

fn finite(path: &str, xs: &[f64]) -> Result<(), CoreError> {
    match xs.iter().all(|x| x.is_finite()) {
        true => Ok(()),
        false => Err(non_finite(path)),
    }
}

fn check_value(path: &str, v: &PlanValue) -> Result<(), CoreError> {
    match v {
        PlanValue::Number(n) => finite(path, &[*n]),
        PlanValue::Vec2(v) => finite(path, v),
        PlanValue::Color(c) => finite(path, c),
        PlanValue::Bool(_) | PlanValue::Text(_) | PlanValue::Asset { .. } => Ok(()),
    }
}

fn check_layers(at: &str, layers: &[Layer]) -> Result<(), CoreError> {
    for (i, l) in layers.iter().enumerate() {
        let p = format!("{at}/{i}");
        let t = &l.transform;
        finite(
            &format!("{p}/transform"),
            &[t.position[0], t.position[1], t.scale[0]],
        )?;
        finite(
            &format!("{p}/transform"),
            &[t.scale[1], t.rotation, t.anchor[0], t.anchor[1]],
        )?;
        let c = &l.crop;
        finite(&format!("{p}/crop"), &[c.left, c.right, c.top, c.bottom])?;
        finite(&format!("{p}/opacity"), &[l.opacity])?;
        for (j, e) in l.effects.iter().enumerate() {
            for (k, v) in &e.params {
                check_value(&format!("{p}/effects/{j}/params/{k}"), v)?;
            }
        }
        if let Some(m) = &l.mask {
            finite(&format!("{p}/mask"), &[m.feather, m.choke])?;
        }
        if let Some(tr) = &l.transition {
            finite(&format!("{p}/transition/progress"), &[tr.progress])?;
        }
        if let Some(g) = &l.graphic {
            for (k, v) in &g.slots {
                check_value(&format!("{p}/graphic/slots/{k}"), v)?;
            }
        }
        if let Some(children) = &l.children {
            check_layers(&format!("{p}/children"), children)?;
        }
    }
    Ok(())
}

pub(crate) fn check_frame(plan: &FramePlan) -> Result<(), CoreError> {
    check_layers("/layers", &plan.layers)
}

/// A segment's ends and progress, and its bézier control points (the mixer eases with them).
fn check_segment(path: &str, v: [f64; 2], u: [f64; 2], interp: &Interp) -> Result<(), CoreError> {
    finite(path, &[v[0], v[1], u[0], u[1]])?;
    match *interp {
        Interp::Bezier { x1, y1, x2, y2 } => finite(path, &[x1, y1, x2, y2]),
        Interp::Hold | Interp::Linear => Ok(()),
    }
}

pub(crate) fn check_audio(plan: &AudioPlan) -> Result<(), CoreError> {
    for (i, item) in plan.items.iter().enumerate() {
        for (j, s) in item.gain.iter().enumerate() {
            let p = format!("/items/{i}/gain/{j}");
            check_segment(&p, s.db, s.u, &s.interp)?;
            for sh in &s.shapes {
                finite(&p, &sh.p)?;
            }
        }
        for (j, s) in item.pan.iter().enumerate() {
            check_segment(&format!("/items/{i}/pan/{j}"), s.pan, s.u, &s.interp)?;
        }
        for (j, e) in item.effects.iter().enumerate() {
            let p = format!("/items/{i}/effects/{j}");
            for (name, segs) in &e.numbers {
                for (k, s) in segs.iter().enumerate() {
                    check_segment(&format!("{p}/numbers/{name}/{k}"), s.v, s.u, &s.interp)?;
                }
            }
            for (name, v) in &e.values {
                check_value(&format!("{p}/values/{name}"), v)?;
            }
        }
    }
    Ok(())
}
