//! `fin item move|split|delete|trim|slip|slide|link|unlink`: edits of any item kind (plan 4C
//! Task 23). Modes default by kind (Decision 18): ripple for clips; overwrite (lift for a
//! delete) for every other kind. Every write echoes `resolved.mode` where the op has one.
//! A split cuts the whole link group at the time (`compose::split_linked`).

use clap::{Args, Subcommand};
use fin_core::compose::split_linked;
use fin_core::{
    CoreError, DeleteMode, Edge, InsertMode, ItemDelete, ItemLink, ItemMove, ItemSlide, ItemSlip,
    ItemTrim, ItemUnlink, Project, TrimMode,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::Builder;
use super::builder::rename_json;
use super::clip::{merge_echo, one_of};
use crate::addr;

#[derive(Debug, Clone, Subcommand, Serialize)]
#[serde(untagged)]
pub enum ItemCmd {
    Move(MoveArgs),
    Split(SplitArgs),
    Delete(DeleteArgs),
    Trim(TrimArgs),
    Slip(DeltaArgs),
    Slide(DeltaArgs),
    Link(LinkArgs),
    Unlink(UnlinkArgs),
}

/// Move an item (and its link group, each member on its own track) to `to` (a position before
/// the move: a time, "end" or "after:<item>").
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MoveArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(long, value_name = "time|after:i|end", allow_hyphen_values = true)]
    pub to: String,
    /// Target track (only the named item changes track); default its own.
    #[arg(long, value_name = "t")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    /// "ripple" (default for clips) or "overwrite" (default for other kinds).
    #[arg(long, value_name = "ripple|overwrite")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<InsertModeArg>,
}

/// Cut an item, and each linked member under the time, in two at timeline time `at`.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SplitArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(long, value_name = "time")]
    pub at: String,
}

/// Delete an item (and its link group).
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteArgs {
    #[arg(value_name = "i")]
    pub item: String,
    /// "ripple" (default for clips) or "lift" (default for other kinds).
    #[arg(long, value_name = "ripple|lift")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<DeleteModeArg>,
}

/// Move one edge of an item (and its link group) by a signed delta.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrimArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(
        long,
        value_name = "±time",
        allow_hyphen_values = true,
        required_unless_present = "end",
        conflicts_with = "end"
    )]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[arg(long, value_name = "±time", allow_hyphen_values = true)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// "ripple" (default for clips), "roll" or "overwrite" (default for other kinds).
    #[arg(long, value_name = "ripple|roll|overwrite")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<TrimModeArg>,
}

/// Slip (move the source under the item, in source seconds) or slide (move the item between
/// its neighbours, trimming them) by a signed delta.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeltaArgs {
    #[arg(value_name = "i")]
    pub item: String,
    #[arg(value_name = "±time", allow_hyphen_values = true)]
    pub delta: String,
}

/// Make two or more items (on one timeline, none linked) one link group; binds $link.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkArgs {
    #[arg(value_name = "i", num_args = 2.., required = true)]
    pub items: Vec<String>,
}

/// Dissolve a link group, named by its id (lkN) or by one of its items.
#[derive(Debug, Clone, Args, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UnlinkArgs {
    #[arg(value_name = "i|lk")]
    pub target: String,
}

/// A closed set of flag values (review #49), shared by the clap flag, the serde args and the
/// JSON schema: each takes exactly the listed spellings, and serde's error names the `field`
/// and the values.
macro_rules! closed_arg {
    ($(#[$doc:meta])* $name:ident ($field:literal) { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant),+];

            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }
        }

        impl clap::ValueEnum for $name {
            fn value_variants<'a>() -> &'a [Self] {
                Self::ALL
            }
            fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
                Some(clap::builder::PossibleValue::new(self.name()))
            }
        }

        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.serialize_str(self.name())
            }
        }

        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let given = String::deserialize(d)?;
                Self::ALL.iter().copied().find(|v| v.name() == given).ok_or_else(|| {
                    let names: Vec<&str> = Self::ALL.iter().map(|v| v.name()).collect();
                    serde::de::Error::custom(format!(
                        "{} {given:?} is not one of {}",
                        $field,
                        names.join(", ")
                    ))
                })
            }
        }

        impl schemars::JsonSchema for $name {
            fn schema_name() -> std::borrow::Cow<'static, str> {
                stringify!($name).into()
            }
            fn inline_schema() -> bool {
                true
            }
            fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
                schemars::json_schema!({ "type": "string", "enum": [$($text),+] })
            }
        }
    };
}
pub(crate) use closed_arg;

closed_arg!(
    /// An insert's or a move's `--mode`.
    InsertModeArg("mode") { Ripple = "ripple", Overwrite = "overwrite" }
);
closed_arg!(
    /// A delete's `--mode`.
    DeleteModeArg("mode") { Ripple = "ripple", Lift = "lift" }
);
closed_arg!(
    /// A trim's `--mode`.
    TrimModeArg("mode") { Ripple = "ripple", Roll = "roll", Overwrite = "overwrite" }
);

pub fn mode_name(mode: InsertMode) -> &'static str {
    match mode {
        InsertMode::Ripple => "ripple",
        InsertMode::Overwrite => "overwrite",
    }
}

/// The insert/move mode given, else `default`.
pub fn insert_mode(given: Option<InsertModeArg>, default: InsertMode) -> InsertMode {
    match given {
        Some(InsertModeArg::Overwrite) => InsertMode::Overwrite,
        Some(InsertModeArg::Ripple) => InsertMode::Ripple,
        None => default,
    }
}

/// The delete mode given, else ripple for clips and lift for other kinds.
pub fn delete_mode(given: Option<DeleteModeArg>, clip: bool) -> DeleteMode {
    match given {
        Some(DeleteModeArg::Lift) => DeleteMode::Lift,
        Some(DeleteModeArg::Ripple) => DeleteMode::Ripple,
        None if clip => DeleteMode::Ripple,
        None => DeleteMode::Lift,
    }
}

pub fn delete_name(mode: DeleteMode) -> &'static str {
    match mode {
        DeleteMode::Ripple => "ripple",
        DeleteMode::Lift => "lift",
    }
}

/// The trim mode given, else ripple for clips and overwrite for other kinds.
pub fn trim_mode(given: Option<TrimModeArg>, clip: bool) -> TrimMode {
    match given {
        Some(TrimModeArg::Roll) => TrimMode::Roll,
        Some(TrimModeArg::Overwrite) => TrimMode::Overwrite,
        Some(TrimModeArg::Ripple) => TrimMode::Ripple,
        None if clip => TrimMode::Ripple,
        None => TrimMode::Overwrite,
    }
}

pub fn trim_name(mode: TrimMode) -> &'static str {
    match mode {
        TrimMode::Ripple => "ripple",
        TrimMode::Roll => "roll",
        TrimMode::Overwrite => "overwrite",
        TrimMode::Strict => "strict",
    }
}

/// Where `edge` of `item` is now (SP2 follow-up: a trim echoes its resulting edge).
pub fn edge_at(project: &Project, item: &str, edge: Edge) -> Result<Value, CoreError> {
    let (_, _, it) = addr::item(project, item)?;
    Ok(json!(match edge {
        Edge::Start => it.start,
        Edge::End => it.end()?,
    }))
}

/// Whether `item` (an id the work copy holds, or not yet) is a clip; unknown items count as
/// clips, so the op itself reports `NOT_FOUND`.
fn is_clip(project: &Project, item: &str) -> bool {
    addr::item(project, item)
        .ok()
        .is_none_or(|(_, _, it)| it.kind_name() == "clip")
}

fn rate_of_item(project: &Project, item: &str) -> fin_core::Rational {
    addr::item(project, item).map_or(project.rate, |(tl, _, _)| tl.rate)
}

pub fn name(cmd: &ItemCmd) -> &'static str {
    match cmd {
        ItemCmd::Move(_) => "item.move",
        ItemCmd::Split(_) => "item.split",
        ItemCmd::Delete(_) => "item.delete",
        ItemCmd::Trim(_) => "item.trim",
        ItemCmd::Slip(_) => "item.slip",
        ItemCmd::Slide(_) => "item.slide",
        ItemCmd::Link(_) => "item.link",
        ItemCmd::Unlink(_) => "item.unlink",
    }
}

pub fn plan(b: &mut Builder<'_>, cmd: &ItemCmd, suffix: &str) -> Result<Value, CoreError> {
    match cmd {
        ItemCmd::Move(a) => plan_move(b, a),
        ItemCmd::Split(a) => plan_split(b, a, suffix),
        ItemCmd::Delete(a) => {
            let item = b.id(&a.item);
            let mode = delete_mode(a.mode, is_clip(&b.work, &item));
            let members = members(&b.work, &item);
            b.push(
                ItemDelete {
                    item: a.item.clone(),
                    mode,
                }
                .into(),
            )?;
            Ok(json!({ "item": a.item, "mode": delete_name(mode), "items": members }))
        }
        ItemCmd::Trim(a) => plan_trim(b, a),
        ItemCmd::Slip(a) => {
            let item = b.id(&a.item);
            let delta = addr::delta_at(rate_of_item(&b.work, &item), "delta", &a.delta)?;
            b.push(
                ItemSlip {
                    item: a.item.clone(),
                    delta,
                }
                .into(),
            )?;
            let source_in = addr::item(&b.work, &item)
                .ok()
                .and_then(|(_, _, it)| match &it.body {
                    fin_core::ItemBody::Clip { source_in, .. }
                    | fin_core::ItemBody::Graphic { source_in, .. }
                    | fin_core::ItemBody::Nested { source_in, .. } => Some(*source_in),
                    _ => None,
                });
            Ok(json!({ "item": a.item, "delta": delta, "sourceIn": source_in }))
        }
        ItemCmd::Slide(a) => {
            let item = b.id(&a.item);
            let delta = addr::delta_at(rate_of_item(&b.work, &item), "delta", &a.delta)?;
            b.push(
                ItemSlide {
                    item: a.item.clone(),
                    delta,
                    sides: None,
                }
                .into(),
            )?;
            Ok(
                json!({ "item": a.item, "delta": delta, "start": edge_at(&b.work, &item, Edge::Start)? }),
            )
        }
        ItemCmd::Link(a) => {
            let id = if suffix.is_empty() {
                "$link".to_string()
            } else {
                format!("$link{suffix}")
            };
            b.push(
                ItemLink {
                    items: a.items.clone(),
                    id: id.clone(),
                }
                .into(),
            )?;
            Ok(json!({ "items": a.items, "link": id }))
        }
        ItemCmd::Unlink(a) => plan_unlink(b, a),
    }
}

/// The members of `item`'s link group (the item alone when unlinked or unknown).
fn members(project: &Project, item: &str) -> Vec<String> {
    addr::item(project, item).map_or_else(
        |_| vec![item.to_string()],
        |(_, _, it)| addr::link_members(project, it),
    )
}

fn plan_move(b: &mut Builder<'_>, a: &MoveArgs) -> Result<Value, CoreError> {
    let item = b.id(&a.item);
    let (_, track, it) = addr::item(&b.work, &item)?;
    let clip = it.kind_name() == "clip";
    let target_ref = a.track.clone().unwrap_or_else(|| track.id.clone());
    let target = b.id(&target_ref);
    let to = match a.to.strip_prefix("after:") {
        Some(other) => format!("after:{}", b.id(other)),
        None => a.to.clone(),
    };
    let at = addr::position(&b.work, &target, "to", &to)?;
    let default = if clip {
        InsertMode::Ripple
    } else {
        InsertMode::Overwrite
    };
    let mode = insert_mode(a.mode, default);
    let members = members(&b.work, &item);
    let track_name = a.track.clone().unwrap_or(target_ref);
    b.push(
        ItemMove {
            item: a.item.clone(),
            track: track_name.clone(),
            at,
            mode,
        }
        .into(),
    )?;
    Ok(
        json!({ "item": a.item, "track": track_name, "at": at, "mode": mode_name(mode), "items": members }),
    )
}

fn plan_split(b: &mut Builder<'_>, a: &SplitArgs, suffix: &str) -> Result<Value, CoreError> {
    let item = b.id(&a.item);
    let at = addr::time_at(rate_of_item(&b.work, &item), "at", &a.at)?;
    let composed = split_linked(&b.work, &item, at)?;
    let names = b.push_composed(composed.ops, suffix)?;
    let mut echo = composed.echo;
    rename_json(&mut echo, &names);
    let mut resolved = json!({ "item": a.item, "at": at });
    merge_echo(&mut resolved, echo);
    Ok(resolved)
}

fn plan_trim(b: &mut Builder<'_>, a: &TrimArgs) -> Result<Value, CoreError> {
    let (is_start, input) = one_of("item.trim", ("start", &a.start), ("end", &a.end))?;
    let (edge, field) = if is_start {
        (Edge::Start, "start")
    } else {
        (Edge::End, "end")
    };
    let item = b.id(&a.item);
    let delta = addr::delta_at(rate_of_item(&b.work, &item), field, input)?;
    let mode = trim_mode(a.mode, is_clip(&b.work, &item));
    let members = members(&b.work, &item);
    b.push(
        ItemTrim {
            item: a.item.clone(),
            edge,
            delta,
            mode,
            alone: false,
        }
        .into(),
    )?;
    Ok(json!({
        "item": a.item,
        "edge": edge,
        "delta": delta,
        "mode": trim_name(mode),
        "edgeAt": edge_at(&b.work, &item, edge)?,
        "items": members,
    }))
}

fn plan_unlink(b: &mut Builder<'_>, a: &UnlinkArgs) -> Result<Value, CoreError> {
    let target = b.id(&a.target);
    let tracks = || b.work.timelines.iter().flat_map(|tl| tl.tracks.iter());
    let in_group = |g: &str| -> Vec<String> {
        tracks()
            .flat_map(|t| t.items.iter())
            .filter(|i| i.common.link.as_deref() == Some(g))
            .map(|i| i.id.clone())
            .collect()
    };
    let (group, items) = if !in_group(&target).is_empty() {
        (a.target.clone(), in_group(&target))
    } else {
        let (_, _, it) = addr::item(&b.work, &target).map_err(|e| {
            e.with_fix("name a link group (lkN) or an item in one; `fin view` shows ⟷ lkN")
        })?;
        let Some(link) = it.common.link.clone() else {
            return Err(
                CoreError::new("INVALID_OP", format!("item {} is not linked", a.target))
                    .with_details(json!({ "item": a.target }))
                    .with_fix(
                        "name a linked item or its group (lkN); link items with fin item link",
                    ),
            );
        };
        let items = in_group(&link);
        (link, items)
    };
    b.push(
        ItemUnlink {
            group: group.clone(),
            items: None,
        }
        .into(),
    )?;
    Ok(json!({ "group": group, "items": items }))
}
