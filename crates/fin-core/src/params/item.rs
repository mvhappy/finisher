//! Item parameters, and the specs of a graphic's slots by the kind its template declares.

use serde_json::json;

use crate::error::CoreError;
use crate::model::{Item, ItemBody, LottieInfo};
use crate::params::check::unknown;
use crate::params::{DefaultValue, ParamSpec, ParamType, Scope, effect_path, effects};

const ALL: &[&str] = &[
    "clip",
    "caption",
    "graphic",
    "image",
    "adjustment",
    "nested",
];
const SOURCED: &[&str] = &["clip", "nested"];
const GRAPHIC: &[&str] = &["graphic"];

const fn spec(
    path: &'static str,
    kinds: &'static [&'static str],
    ty: ParamType,
    animatable: bool,
    default: DefaultValue,
    unit: Option<&'static str>,
    fix: &'static str,
) -> ParamSpec {
    ParamSpec {
        path,
        scope: Scope::Item { kinds },
        ty,
        animatable,
        default,
        unit,
        fix,
    }
}

const POSITION: f64 = 100_000.0;
const CROP: ParamType = ParamType::Number { min: 0.0, max: 1.0 };
const CROP_FIX: &str = "use a crop from {min} to {max} (a fraction of the item's size)";
const FADE: ParamType = ParamType::Time { min: Some("0") };

pub(super) const SPECS: &[ParamSpec] = &[
    spec(
        "transform.position",
        ALL,
        ParamType::Vec2 {
            min: [-POSITION, -POSITION],
            max: [POSITION, POSITION],
        },
        true,
        DefaultValue::Vec2([0.0, 0.0]),
        Some("px"),
        "use a position from {min} to {max} {unit} (x,y from the frame centre)",
    ),
    spec(
        "transform.scale",
        ALL,
        ParamType::Vec2 {
            min: [-100.0, -100.0],
            max: [100.0, 100.0],
        },
        true,
        DefaultValue::Vec2([1.0, 1.0]),
        None,
        "use a scale from {min} to {max} (x,y; 1 is the original size)",
    ),
    spec(
        "transform.rotation",
        ALL,
        ParamType::Number {
            min: -3600.0,
            max: 3600.0,
        },
        true,
        DefaultValue::Number(0.0),
        Some("deg"),
        "use a rotation from {min} to {max} {unit}",
    ),
    spec(
        "transform.anchor",
        ALL,
        ParamType::Vec2 {
            min: [-10.0, -10.0],
            max: [10.0, 10.0],
        },
        true,
        DefaultValue::Vec2([0.5, 0.5]),
        None,
        "use an anchor from {min} to {max} (x,y; 0.5,0.5 is the item's centre)",
    ),
    spec(
        "crop.left",
        ALL,
        CROP,
        true,
        DefaultValue::Number(0.0),
        None,
        CROP_FIX,
    ),
    spec(
        "crop.right",
        ALL,
        CROP,
        true,
        DefaultValue::Number(0.0),
        None,
        CROP_FIX,
    ),
    spec(
        "crop.top",
        ALL,
        CROP,
        true,
        DefaultValue::Number(0.0),
        None,
        CROP_FIX,
    ),
    spec(
        "crop.bottom",
        ALL,
        CROP,
        true,
        DefaultValue::Number(0.0),
        None,
        CROP_FIX,
    ),
    spec(
        "opacity",
        ALL,
        ParamType::Number { min: 0.0, max: 1.0 },
        true,
        DefaultValue::Number(1.0),
        None,
        "use an opacity from {min} to {max}",
    ),
    spec(
        "blend",
        ALL,
        ParamType::Enum {
            values: &[
                "normal", "multiply", "screen", "overlay", "add", "darken", "lighten",
            ],
        },
        false,
        DefaultValue::Text("normal"),
        None,
        "use a blend mode: normal, multiply, screen, overlay, add, darken or lighten",
    ),
    spec(
        "audio.gain",
        SOURCED,
        ParamType::Number {
            min: -96.0,
            max: 24.0,
        },
        true,
        DefaultValue::Number(0.0),
        Some("dB"),
        "use a gain from {min} to {max} {unit}",
    ),
    spec(
        "audio.pan",
        SOURCED,
        ParamType::Number {
            min: -1.0,
            max: 1.0,
        },
        true,
        DefaultValue::Number(0.0),
        None,
        "use a pan from {min} (left) to {max} (right)",
    ),
    spec(
        "audio.fadeIn",
        SOURCED,
        FADE,
        false,
        DefaultValue::Text("0"),
        Some("s"),
        "use a fade-in of at least {min} {unit}",
    ),
    spec(
        "audio.fadeOut",
        SOURCED,
        FADE,
        false,
        DefaultValue::Text("0"),
        Some("s"),
        "use a fade-out of at least {min} {unit}",
    ),
    spec(
        "mask.feather",
        ALL,
        ParamType::Number {
            min: 0.0,
            max: 1000.0,
        },
        true,
        DefaultValue::Number(0.0),
        Some("px"),
        "use a feather from {min} to {max} {unit}",
    ),
    spec(
        "mask.choke",
        ALL,
        ParamType::Number {
            min: -1000.0,
            max: 1000.0,
        },
        true,
        DefaultValue::Number(0.0),
        Some("px"),
        "use a choke from {min} to {max} {unit}",
    ),
    spec(
        "mask.invert",
        ALL,
        ParamType::Bool,
        false,
        DefaultValue::Bool(false),
        None,
        "use true or false",
    ),
    spec(
        "speed",
        &["clip"],
        ParamType::Rational {
            min: "1/100",
            max: "100",
        },
        false,
        DefaultValue::Text("1"),
        None,
        "use a speed from {min} to {max} (n or n/d; the source must cover duration × speed)",
    ),
];

/// A graphic's slots, one spec per slot kind a template declares (`text`, `color`, `image`,
/// `number`), in that order; each is listed at the path `slots.<id>`.
pub(super) const SLOTS: &[ParamSpec] = &[
    spec(
        "slots.<id>",
        GRAPHIC,
        ParamType::Text { max_len: 1000 },
        false,
        DefaultValue::None,
        None,
        "use text of at most {max} characters",
    ),
    spec(
        "slots.<id>",
        GRAPHIC,
        ParamType::Color,
        true,
        DefaultValue::None,
        None,
        "use a colour #rrggbb or #rrggbbaa (lowercase hex)",
    ),
    spec(
        "slots.<id>",
        GRAPHIC,
        ParamType::AssetRef {
            kinds: &["image", "svg"],
        },
        false,
        DefaultValue::None,
        None,
        "use an image or svg asset",
    ),
    spec(
        "slots.<id>",
        GRAPHIC,
        ParamType::Number {
            min: -1_000_000.0,
            max: 1_000_000.0,
        },
        true,
        DefaultValue::None,
        None,
        "use a number from {min} to {max}",
    ),
];

/// The spec of slot `slot` of `template` (asset `asset`): `SLOT_UNKNOWN` when the template has
/// no such slot, or declares a kind with no parameter type.
pub fn slot_spec(
    template: &LottieInfo,
    asset: &str,
    slot: &str,
) -> Result<&'static ParamSpec, CoreError> {
    let ids: Vec<&str> = template.slots.iter().map(|s| s.id.as_str()).collect();
    let Some(s) = template.slots.iter().find(|s| s.id == slot) else {
        return Err(CoreError::new(
            "SLOT_UNKNOWN",
            format!("template {asset} has no slot {slot}"),
        )
        .with_details(json!({ "asset": asset, "slot": slot, "slots": ids }))
        .with_fix(if ids.is_empty() {
            "this template has no slots".to_string()
        } else {
            format!("use one of the template's slots: {}", ids.join(", "))
        }));
    };
    let i = match s.kind.as_str() {
        "text" => 0,
        "color" => 1,
        "image" => 2,
        "number" => 3,
        other => {
            return Err(CoreError::new(
                "SLOT_UNKNOWN",
                format!("slot {slot} of template {asset} has kind {other}, which cannot be set"),
            )
            .with_details(json!({ "asset": asset, "slot": slot, "kind": other, "slots": ids }))
            .with_fix("set a text, color, image or number slot"));
        }
    };
    Ok(&SLOTS[i])
}

/// The spec `path` names on `item` (see the module docs of [`crate::params`]).
pub(super) fn find(
    item: &Item,
    template: Option<&LottieInfo>,
    path: &str,
) -> Result<&'static ParamSpec, CoreError> {
    let kind = item.kind_name();
    if let Some((fx, param)) = effect_path(path) {
        let Some(e) = item.common.effects.iter().find(|e| e.id == fx) else {
            let ids: Vec<&str> = item.common.effects.iter().map(|e| e.id.as_str()).collect();
            let candidates: Vec<String> =
                ids.iter().map(|f| format!("effects.{f}.{param}")).collect();
            return Err(unknown(
                format!("item {} has no effect {fx}", item.id),
                path,
                json!({ "item": item.id, "effects": ids }),
                &candidates,
            ));
        };
        return effects::find(&e.kind, param, |p| format!("effects.{fx}.{p}"));
    }
    if let Some(slot) = path.strip_prefix("slots.")
        && let ItemBody::Graphic { asset, .. } = &item.body
    {
        let empty = LottieInfo {
            width: 0,
            height: 0,
            rate: crate::time::Rational::from_int(1),
            duration: crate::time::Rational::zero(),
            slots: Vec::new(),
            fonts: Vec::new(),
        };
        return slot_spec(template.unwrap_or(&empty), asset, slot);
    }
    if path.starts_with("mask.") && item.common.mask.is_none() {
        return Err(
            CoreError::new("PARAM_UNKNOWN", format!("item {} has no mask", item.id))
                .with_details(json!({ "path": path, "item": item.id, "alternatives": [] }))
                .with_fix("mask edits arrive in sub-project 6"),
        );
    }
    let applies =
        |s: &&ParamSpec| matches!(s.scope, Scope::Item { kinds } if kinds.contains(&kind));
    if let Some(s) = SPECS.iter().filter(applies).find(|s| s.path == path) {
        return Ok(s);
    }
    let mut candidates: Vec<String> = SPECS
        .iter()
        .filter(applies)
        .filter(|s| item.common.mask.is_some() || !s.path.starts_with("mask."))
        .map(|s| s.path.to_string())
        .collect();
    if let ItemBody::Graphic { .. } = item.body
        && let Some(t) = template
    {
        candidates.extend(t.slots.iter().map(|s| format!("slots.{}", s.id)));
    }
    for e in &item.common.effects {
        candidates.extend(effects::params_of(&e.kind).map(|p| format!("effects.{}.{p}", e.id)));
    }
    Err(unknown(
        format!("a {kind} item has no parameter {path}"),
        path,
        json!({ "item": item.id, "kind": kind }),
        &candidates,
    ))
}
