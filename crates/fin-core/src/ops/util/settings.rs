//! Track, timeline and marker settings (`track.set`, `timeline.set`, `marker.set`, and the
//! values `track.add`, `timeline.add` and `marker.add` start with), checked against the
//! settings registry's track, timeline and marker scopes ([`crate::params`]): an unknown path
//! and a value's shape are structural (`PARAM_UNKNOWN`, `PARAM_TYPE`), its range policy
//! (`INVALID_RANGE`). The getters and setters are the registry's.

use serde_json::json;

use crate::error::CoreError;
use crate::model::ParamValue;
use crate::ops::util::normalize_value;
use crate::params::{self, ParamSpec, ParamType, ScopeOf};
use crate::time::Rational;

pub(crate) use crate::params::{set_marker, set_timeline, set_track};

/// Whose settings a path names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scope {
    Track,
    Timeline,
    Marker,
}

impl Scope {
    fn of(self) -> ScopeOf<'static> {
        match self {
            Scope::Track => ScopeOf::Track,
            Scope::Timeline => ScopeOf::Timeline,
            Scope::Marker => ScopeOf::Marker,
        }
    }
}

/// Checks `value` for `path` in `scope`: the path exists and the value has its shape
/// (structural), numbers are finite (`-0.0` becomes `0.0`), and, when `policy`, the value is in
/// range. No value (a reset) has nothing to check.
///
/// Beyond the registry's shapes, three bounds are structural because the model cannot hold
/// anything else: a rational (a frame rate) is positive, a size (a resolution) is at least one
/// pixel each way, and a time (a marker's position or duration) is at least its minimum (0).
pub(crate) fn check(
    scope: Scope,
    path: &str,
    value: Option<&mut ParamValue>,
    policy: bool,
) -> Result<(), CoreError> {
    let spec = params::find(&scope.of(), path)?;
    let Some(value) = value else {
        return Ok(());
    };
    normalize_value(path, value)?;
    params::check_shape(spec, path, value)?;
    structural_bounds(spec, path, value)?;
    if policy {
        params::check_range(spec, path, value)?;
    }
    Ok(())
}

fn structural_bounds(spec: &ParamSpec, path: &str, value: &ParamValue) -> Result<(), CoreError> {
    let bad = match (&spec.ty, value) {
        (ParamType::Rational { .. }, ParamValue::Text(s)) => {
            !s.parse::<Rational>().is_ok_and(|r| r.is_positive())
        }
        (ParamType::Size { .. }, ParamValue::Vec2(v)) => v.iter().any(|x| *x < 1.0),
        (ParamType::Time { min: Some(min) }, ParamValue::Text(s)) => {
            match (s.parse::<Rational>(), min.parse::<Rational>()) {
                (Ok(t), Ok(min)) => t < min,
                _ => false,
            }
        }
        _ => false,
    };
    if !bad {
        return Ok(());
    }
    // Always out of range, so the registry's range error names it (min, max, unit, fix).
    params::check_range(spec, path, value)?;
    Err(
        CoreError::new("INVALID_RANGE", format!("{path} is out of range"))
            .with_details(json!({ "path": path, "unit": spec.unit }))
            .with_fix(params::fill_fix(spec, path)),
    )
}
