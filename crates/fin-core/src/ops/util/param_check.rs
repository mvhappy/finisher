//! Checking one parameter value against its spec inside an op (plan 4C Task 11): shared by
//! `param.set`, graphic slots on insert, and the effect and keyframe ops.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{ParamValue, Project};
use crate::ops::Bindings;
use crate::ops::util::{asset, normalize_value};
use crate::params::{self, ParamSpec, ParamType};

/// Canonicalises `value` (finite floats, `-0.0` → `0.0`) and resolves an asset reference's
/// placeholder, then checks it at `path`: its shape and the asset it names (that it exists,
/// and its kind) are structural; its range is policy.
pub(crate) fn check_value(
    project: &Project,
    spec: &ParamSpec,
    path: &str,
    value: &mut ParamValue,
    bindings: &Bindings,
) -> Result<(), CoreError> {
    normalize_value(path, value)?;
    if let Some(a) = value.asset_mut() {
        *a = bindings.resolve(a)?;
    }
    params::check_shape(spec, path, value)?;
    if let (ParamType::AssetRef { kinds }, Some(id)) = (&spec.ty, value.asset()) {
        let a = asset(project, id)?;
        let kind = serde_json::to_value(a.kind)
            .ok()
            .and_then(|k| k.as_str().map(str::to_string))
            .unwrap_or_default();
        if !kinds.contains(&kind.as_str()) {
            return Err(CoreError::new(
                "PARAM_TYPE",
                format!(
                    "{path} takes an asset ({}), not {kind} asset {id}",
                    kinds.join(", ")
                ),
            )
            .with_details(json!({ "path": path, "expected": "assetRef", "got": kind,
                                  "asset": id, "kinds": kinds }))
            .with_fix(params::fill_fix(spec, path)));
        }
    }
    if bindings.policy() {
        params::check_range(spec, path, value)?;
    }
    Ok(())
}

/// Adds `item` to `e.details` (an error about one of an item's parameters).
pub(crate) fn about_item(mut e: CoreError, item: &str) -> CoreError {
    if let serde_json::Value::Object(d) = &mut e.details {
        d.insert("item".into(), json!(item));
    }
    e
}
