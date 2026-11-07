//! Track, timeline, marker and style parameters, and reading and writing them on the model
//! (`track.set`, `timeline.set` and `marker.set` use these; caption style paths arrive with
//! Task 18). None is animatable. A timeline's `rate` may change only while it has no items
//! (`TIMELINE_NOT_EMPTY`, the op's check).

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Marker, ParamValue, Resolution, Timeline, Track};
use crate::params::access::{boolean, number, rational, text, time, vec2, wrong};
use crate::params::check::unknown;
use crate::params::{DefaultValue, ParamSpec, ParamType, Scope};
use crate::time::Rational;

const fn spec(
    path: &'static str,
    scope: Scope,
    ty: ParamType,
    default: DefaultValue,
    unit: Option<&'static str>,
    fix: &'static str,
) -> ParamSpec {
    ParamSpec {
        path,
        scope,
        ty,
        animatable: false,
        default,
        unit,
        fix,
    }
}

const NAME: ParamType = ParamType::Text { max_len: 64 };
const NAME_FIX: &str = "use a name of at most {max} characters";
const BOOL_FIX: &str = "use true or false";

pub(super) const TRACK: &[ParamSpec] = &[
    spec(
        "name",
        Scope::Track,
        NAME,
        DefaultValue::None,
        None,
        NAME_FIX,
    ),
    spec(
        "mute",
        Scope::Track,
        ParamType::Bool,
        DefaultValue::Bool(false),
        None,
        BOOL_FIX,
    ),
    spec(
        "solo",
        Scope::Track,
        ParamType::Bool,
        DefaultValue::Bool(false),
        None,
        BOOL_FIX,
    ),
    spec(
        "lock",
        Scope::Track,
        ParamType::Bool,
        DefaultValue::Bool(false),
        None,
        BOOL_FIX,
    ),
    spec(
        "gain",
        Scope::Track,
        ParamType::Number {
            min: -96.0,
            max: 24.0,
        },
        DefaultValue::Number(0.0),
        Some("dB"),
        "use a gain from {min} to {max} {unit}",
    ),
];

pub(super) const TIMELINE: &[ParamSpec] = &[
    spec(
        "name",
        Scope::Timeline,
        NAME,
        DefaultValue::None,
        None,
        NAME_FIX,
    ),
    spec(
        "resolution",
        Scope::Timeline,
        ParamType::Size {
            min: 16,
            max: 16384,
        },
        DefaultValue::Vec2([1080.0, 1920.0]),
        Some("px"),
        "use a width,height from {min} to {max} {unit}",
    ),
    spec(
        "rate",
        Scope::Timeline,
        ParamType::Rational {
            min: "1",
            max: "240",
        },
        DefaultValue::None,
        Some("fps"),
        "use a frame rate from {min} to {max} {unit} (n or n/d, such as 30000/1001)",
    ),
];

pub(super) const MARKER: &[ParamSpec] = &[
    spec(
        "label",
        Scope::Marker,
        ParamType::Text { max_len: 256 },
        DefaultValue::None,
        None,
        "use a label of at most {max} characters",
    ),
    spec(
        "color",
        Scope::Marker,
        ParamType::Color,
        DefaultValue::None,
        None,
        "use a colour #rrggbb or #rrggbbaa (lowercase hex)",
    ),
    spec(
        "at",
        Scope::Marker,
        ParamType::Time { min: Some("0") },
        DefaultValue::None,
        Some("s"),
        "use a time of at least {min} {unit}",
    ),
    spec(
        "duration",
        Scope::Marker,
        ParamType::Time { min: Some("0") },
        DefaultValue::None,
        Some("s"),
        "use a duration of at least {min} {unit}",
    ),
];

/// Caption style paths arrive with Task 18.
pub(super) const STYLE: &[ParamSpec] = &[];

pub(super) fn find(
    list: &'static [ParamSpec],
    scope: &str,
    path: &str,
) -> Result<&'static ParamSpec, CoreError> {
    if let Some(s) = list.iter().find(|s| s.path == path) {
        return Ok(s);
    }
    let candidates: Vec<String> = list.iter().map(|s| s.path.to_string()).collect();
    Err(unknown(
        format!("a {scope} has no parameter {path}"),
        path,
        json!({ "scope": scope }),
        &candidates,
    ))
}

fn missing(scope: &str, path: &str) -> CoreError {
    CoreError::new(
        "PARAM_UNKNOWN",
        format!("a {scope} has no parameter {path}"),
    )
    .with_details(json!({ "path": path, "scope": scope, "alternatives": [] }))
    .with_fix("fin schema param lists every path")
}

/// `name` has no default to reset to.
fn required(path: &str) -> CoreError {
    CoreError::new("INVALID_OP", format!("{path} cannot be reset"))
        .with_details(json!({ "path": path }))
        .with_fix(format!("give {path} a value"))
}

fn required_text(path: &str, v: Option<&ParamValue>) -> Result<String, CoreError> {
    text(path, v)?.ok_or_else(|| required(path))
}

/// The value of `path` of `track`.
pub fn get_track(track: &Track, path: &str) -> Result<Option<ParamValue>, CoreError> {
    Ok(Some(match path {
        "name" => ParamValue::Text(track.name.clone()),
        "mute" => ParamValue::Bool(track.mute),
        "solo" => ParamValue::Bool(track.solo),
        "lock" => ParamValue::Bool(track.lock),
        "gain" => ParamValue::Number(track.gain),
        _ => return Err(missing("track", path)),
    }))
}

/// Sets `path` of `track` (`None` resets), returning the old value.
pub fn set_track(
    track: &mut Track,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_track(track, path)?;
    let v = value.as_ref();
    match path {
        "name" => track.name = required_text(path, v)?,
        "mute" => track.mute = boolean(path, v, false)?,
        "solo" => track.solo = boolean(path, v, false)?,
        "lock" => track.lock = boolean(path, v, false)?,
        "gain" => track.gain = number(path, v, 0.0)?,
        _ => return Err(missing("track", path)),
    }
    Ok(old)
}

/// The value of `path` of `timeline`.
pub fn get_timeline(timeline: &Timeline, path: &str) -> Result<Option<ParamValue>, CoreError> {
    let r = timeline.resolution;
    Ok(Some(match path {
        "name" => ParamValue::Text(timeline.name.clone()),
        "resolution" => ParamValue::Vec2([f64::from(r.width), f64::from(r.height)]),
        "rate" => time(timeline.rate),
        _ => return Err(missing("timeline", path)),
    }))
}

/// Sets `path` of `timeline` (`None` resets the resolution to 1080×1920), returning the old
/// value.
pub fn set_timeline(
    timeline: &mut Timeline,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_timeline(timeline, path)?;
    let v = value.as_ref();
    match path {
        "name" => timeline.name = required_text(path, v)?,
        "resolution" => {
            let [w, h] = vec2(path, v, [1080.0, 1920.0])?;
            let px = |x: f64| -> Result<u32, CoreError> {
                if x >= 0.0 && x <= f64::from(u32::MAX) && x == x.floor() {
                    // In range and whole: the conversion is exact.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    Ok(x as u32)
                } else {
                    Err(wrong(path, "size", &ParamValue::Vec2([w, h])))
                }
            };
            timeline.resolution = Resolution {
                width: px(w)?,
                height: px(h)?,
            };
        }
        "rate" => {
            let rate = rational(path, v, Rational::zero())?;
            if v.is_none() {
                return Err(required(path));
            }
            timeline.rate = rate;
        }
        _ => return Err(missing("timeline", path)),
    }
    Ok(old)
}

/// The value of `path` of `marker` (`None`: no colour or duration).
pub fn get_marker(marker: &Marker, path: &str) -> Result<Option<ParamValue>, CoreError> {
    Ok(match path {
        "label" => Some(ParamValue::Text(marker.label.clone())),
        "color" => marker.color.clone().map(ParamValue::Text),
        "at" => Some(time(marker.at)),
        "duration" => marker.duration.map(time),
        _ => return Err(missing("marker", path)),
    })
}

/// Sets `path` of `marker` (`None` clears a colour or duration), returning the old value.
pub fn set_marker(
    marker: &mut Marker,
    path: &str,
    value: Option<ParamValue>,
) -> Result<Option<ParamValue>, CoreError> {
    let old = get_marker(marker, path)?;
    let v = value.as_ref();
    match path {
        "label" => marker.label = required_text(path, v)?,
        "color" => marker.color = text(path, v)?,
        "at" => {
            if v.is_none() {
                return Err(required(path));
            }
            marker.at = rational(path, v, Rational::zero())?;
        }
        "duration" => {
            marker.duration = match v {
                None => None,
                Some(_) => Some(rational(path, v, Rational::zero())?),
            }
        }
        _ => return Err(missing("marker", path)),
    }
    Ok(old)
}
