//! Value shapes of the floats an item carries (structural, plan 4C Decision 25): every number
//! must be finite (`NaN` and `±inf` have no JSON form), and `-0.0` is stored as `0.0`, since
//! the two compare equal but encode, and so hash, differently.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Interp, Item, ItemBody, ParamValue};

/// Checks every float of `item` is finite and turns each `-0.0` into `0.0`. `INVALID_RANGE`
/// with `details.path` names the first non-finite one.
pub(crate) fn normalize_item(item: &mut Item) -> Result<(), CoreError> {
    let c = &mut item.common;
    let t = &mut c.transform;
    let mut fields: Vec<(String, &mut f64)> = Vec::new();
    for (name, v) in [
        ("transform.position", &mut t.position),
        ("transform.scale", &mut t.scale),
        ("transform.anchor", &mut t.anchor),
    ] {
        let [x, y] = v;
        fields.push((format!("{name}[0]"), x));
        fields.push((format!("{name}[1]"), y));
    }
    fields.push(("transform.rotation".into(), &mut t.rotation));
    let cr = &mut c.crop;
    fields.extend([
        ("crop.left".into(), &mut cr.left),
        ("crop.right".into(), &mut cr.right),
        ("crop.top".into(), &mut cr.top),
        ("crop.bottom".into(), &mut cr.bottom),
    ]);
    fields.push(("opacity".into(), &mut c.opacity));
    if let Some(m) = &mut c.mask {
        fields.push(("mask.feather".into(), &mut m.feather));
        fields.push(("mask.choke".into(), &mut m.choke));
    }
    if let Some(a) = &mut c.audio {
        fields.push(("audio.gain".into(), &mut a.gain));
        fields.push(("audio.pan".into(), &mut a.pan));
    }
    for (path, v) in fields {
        number(&path, v)?;
    }
    for e in &mut c.effects {
        for (k, v) in &mut e.params {
            param(&format!("effects.{}.{k}", e.id), v)?;
        }
    }
    for (path, keys) in &mut c.keyframes {
        for k in keys {
            param(&format!("keyframes.{path}"), &mut k.value)?;
            if let Interp::Bezier { x1, y1, x2, y2 } = &mut k.interp {
                for v in [x1, y1, x2, y2] {
                    number(&format!("keyframes.{path}.interp"), v)?;
                }
            }
        }
    }
    body(&mut item.body)
}

/// [`normalize_item`] for an item body alone (an insert's).
pub(crate) fn body(body: &mut ItemBody) -> Result<(), CoreError> {
    match body {
        ItemBody::Graphic { slots, .. } => {
            for (k, v) in slots {
                param(&format!("slots.{k}"), v)?;
            }
        }
        ItemBody::Caption { block } => {
            for (k, v) in &mut block.overrides {
                param(&format!("overrides.{k}"), v)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// One parameter value's floats at `path`: finite, `-0.0` stored as `0.0`.
pub(crate) fn param(path: &str, v: &mut ParamValue) -> Result<(), CoreError> {
    match v {
        ParamValue::Number(x) => number(path, x),
        ParamValue::Vec2([x, y]) => {
            number(path, x)?;
            number(path, y)
        }
        ParamValue::Bool(_) | ParamValue::Text(_) | ParamValue::Asset { .. } => Ok(()),
    }
}

fn number(path: &str, v: &mut f64) -> Result<(), CoreError> {
    if !v.is_finite() {
        return Err(
            CoreError::new("INVALID_RANGE", format!("{path} is not a finite number"))
                .with_details(json!({ "path": path }))
                .with_fix("use a finite number"),
        );
    }
    if *v == 0.0 {
        *v = 0.0;
    }
    Ok(())
}
