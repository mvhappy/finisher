//! `fin image insert` and `fin graphic insert`: still images and SVGs, and Lottie templates
//! with their slot values, on video tracks (plan 4C Task 23). Overwrite by default (Decision 18:
//! ripple is for clips); an overwrite strictly inside an item splits it first
//! (`compose::overwrite`).

use std::collections::BTreeMap;

use clap::{Args, Subcommand};
use fin_core::compose::{Composed, overwrite};
use fin_core::params::slot_spec;
use fin_core::{CoreError, InsertMode, ItemBody, ItemInsert, Rational};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::builder::rename_json;
use super::clip::merge_echo;
use super::item::{InsertModeArg, insert_mode, mode_name};
use crate::addr;
use crate::output::usage;
use crate::values;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum ImageCmd {
    Insert(ImageArgs),
}

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum GraphicCmd {
    Insert(GraphicArgs),
}

/// Place an image or SVG asset on a video track for `dur`; binds $image.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImageArgs {
    #[arg(long, value_name = "t")]
    pub track: String,
    #[arg(long, value_name = "time|after:i|end", allow_hyphen_values = true)]
    pub at: String,
    #[arg(long, value_name = "aN")]
    pub asset: String,
    #[arg(long, value_name = "time")]
    pub dur: String,
    /// "overwrite" (default) or "ripple".
    #[arg(long, value_name = "overwrite|ripple")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<InsertModeArg>,
}

/// Place a Lottie template on a video track; binds $graphic. `dur` defaults to the template's
/// length; each `slot` is `id=value` (typed by the template's slot kind).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GraphicArgs {
    #[arg(long, value_name = "t")]
    pub track: String,
    #[arg(long, value_name = "time|after:i|end", allow_hyphen_values = true)]
    pub at: String,
    #[arg(long, value_name = "aN")]
    pub template: String,
    #[arg(long, value_name = "time")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<String>,
    #[arg(long = "slot", value_name = "id=value")]
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<String>,
    /// "overwrite" (default) or "ripple".
    #[arg(long, value_name = "overwrite|ripple")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<InsertModeArg>,
}

/// One insert of `body` at `at` on `track`: a plain insert in ripple mode, an overwrite (split
/// first when inside an item) otherwise.
#[allow(clippy::too_many_arguments)]
fn insert(
    b: &mut Builder<'_>,
    track: &str,
    at: Rational,
    duration: Rational,
    body: ItemBody,
    mode: InsertMode,
    placeholder: &str,
    suffix: &str,
) -> Result<Value, CoreError> {
    let composed = match mode {
        InsertMode::Overwrite => overwrite(&b.work, track, at, duration, body, placeholder)?,
        InsertMode::Ripple => Composed {
            ops: vec![
                ItemInsert {
                    id: placeholder.into(),
                    track: track.into(),
                    at,
                    duration,
                    mode,
                    body,
                }
                .into(),
            ],
            echo: json!({}),
        },
    };
    let names = b.push_composed(composed.ops, suffix)?;
    let mut echo = composed.echo;
    rename_json(&mut echo, &names);
    Ok(echo)
}

/// The track and position an insert names (`after:` resolved through this tx's bindings).
fn place(
    b: &Builder<'_>,
    track: &str,
    at: &str,
) -> Result<(String, Rational, Rational), CoreError> {
    let track = b.id(track);
    let at = match at.strip_prefix("after:") {
        Some(item) => format!("after:{}", b.id(item)),
        None => at.to_string(),
    };
    let pos = addr::position(&b.work, &track, "at", &at)?;
    Ok((track.clone(), pos, addr::rate_of(&b.work, &track)))
}

pub fn plan_image(b: &mut Builder<'_>, a: &ImageArgs, suffix: &str) -> Result<Value, CoreError> {
    let (track, at, rate) = place(b, &a.track, &a.at)?;
    let duration = addr::time_at(rate, "dur", &a.dur)?;
    let mode = insert_mode(a.mode, InsertMode::Overwrite);
    let body = ItemBody::Image {
        asset: b.id(&a.asset),
    };
    let echo = insert(b, &track, at, duration, body, mode, "$image", suffix)?;
    let mut resolved = json!({ "track": a.track, "at": at, "asset": a.asset, "duration": duration, "mode": mode_name(mode) });
    merge_echo(&mut resolved, echo);
    Ok(resolved)
}

pub fn plan_graphic(
    b: &mut Builder<'_>,
    a: &GraphicArgs,
    suffix: &str,
) -> Result<Value, CoreError> {
    let (track, at, rate) = place(b, &a.track, &a.at)?;
    let template = b.id(&a.template);
    let asset = b.work.assets.get(&template).ok_or_else(|| {
        addr::not_found(
            "asset",
            &a.template,
            b.work.assets.keys().cloned().collect(),
        )
    })?;
    let Some(lottie) = asset.lottie.clone() else {
        return Err(CoreError::new(
            "INVALID_OP",
            format!("asset {} is not a Lottie template", a.template),
        )
        .with_details(json!({ "asset": a.template, "kind": asset.kind }))
        .with_fix("name a lottieTemplate asset (fin asset add <file.lottie>)"));
    };
    let duration = match &a.dur {
        Some(d) => addr::time_at(rate, "dur", d)?,
        None => lottie.duration,
    };
    let mut slots = BTreeMap::new();
    for entry in &a.slots {
        let Some((id, value)) = entry.split_once('=') else {
            return Err(usage(
                format!("--slot {entry:?} is not id=value"),
                json!({ "slot": entry }),
                "write each slot as --slot <id>=<value>, e.g. --slot title=Hello",
            ));
        };
        let spec = slot_spec(&lottie, &a.template, id)?;
        slots.insert(
            id.to_string(),
            values::parse(spec, &format!("slots.{id}"), value, rate)?,
        );
    }
    let mode = insert_mode(a.mode, InsertMode::Overwrite);
    let echo_slots = json!(slots);
    let body = ItemBody::Graphic {
        asset: template,
        slots,
    };
    let echo = insert(b, &track, at, duration, body, mode, "$graphic", suffix)?;
    let mut resolved = json!({
        "track": a.track, "at": at, "template": a.template, "duration": duration,
        "slots": echo_slots, "mode": mode_name(mode),
    });
    merge_echo(&mut resolved, echo);
    Ok(resolved)
}
