//! The 4C edit commands' registry entries (plan 4C Task 23), listed after `clip.*` in
//! [`crate::registry::commands`].

use crate::commands::{asset, bin, item, marker, media, timeline, track, transition};
use crate::registry::{Command, one_of, schema_of};

const fn write(
    name: &'static str,
    group: &'static str,
    args: &'static str,
    summary: &'static str,
    schema: fn() -> serde_json::Value,
) -> Command {
    Command {
        name,
        group,
        args,
        summary,
        write: true,
        schema,
    }
}

pub const EDIT_COMMANDS: &[Command] = &[
    write(
        "image.insert",
        "image",
        "--track <t> --at <time|after:i|end> --asset <aN> --dur <time> [--mode overwrite|ripple]",
        "place an image/SVG (overwrite by default); binds $image",
        || schema_of::<media::ImageArgs>("image.insert"),
    ),
    write(
        "graphic.insert",
        "graphic",
        "--track <t> --at <time|after:i|end> --template <aN> [--dur <time>] [--slot id=value]... [--mode]",
        "place a Lottie template with slot values; binds $graphic",
        || schema_of::<media::GraphicArgs>("graphic.insert"),
    ),
    write(
        "item.move",
        "item",
        "<i> --to <time|after:i|end> [--track <t>] [--mode ripple|overwrite]",
        "move an item and its link group (ripple for clips, else overwrite)",
        || schema_of::<item::MoveArgs>("item.move"),
    ),
    write(
        "item.split",
        "item",
        "<i> --at <time>",
        "cut an item (and linked members under the time) in two; binds $split",
        || schema_of::<item::SplitArgs>("item.split"),
    ),
    write(
        "item.delete",
        "item",
        "<i> [--mode ripple|lift]",
        "delete an item and its link group (ripple for clips, else lift)",
        || schema_of::<item::DeleteArgs>("item.delete"),
    ),
    write(
        "item.trim",
        "item",
        "<i> (--start <±time>|--end <±time>) [--mode ripple|roll|overwrite]",
        "move one edge; echoes edgeAt (ripple for clips, else overwrite)",
        || one_of(schema_of::<item::TrimArgs>("item.trim"), "start", "end"),
    ),
    write(
        "item.slip",
        "item",
        "<i> <±time>",
        "move the source under a clip by a signed delta (source seconds)",
        || schema_of::<item::DeltaArgs>("item.slip"),
    ),
    write(
        "item.slide",
        "item",
        "<i> <±time>",
        "move an item between its neighbours, trimming them",
        || schema_of::<item::DeltaArgs>("item.slide"),
    ),
    write(
        "item.link",
        "item",
        "<i> <i>...",
        "make items one link group; binds $link",
        || schema_of::<item::LinkArgs>("item.link"),
    ),
    write(
        "item.unlink",
        "item",
        "<i|lkN>",
        "dissolve a link group",
        || schema_of::<item::UnlinkArgs>("item.unlink"),
    ),
    write(
        "transition.set",
        "transition",
        "--between <a> <b> --dur <time> [--kind dissolve|crossfade] [--curve linear|equalPower] [--no-linked]",
        "set the transition at a cut (and its link partners' cuts); binds $transition",
        || schema_of::<transition::SetArgs>("transition.set"),
    ),
    write(
        "transition.remove",
        "transition",
        "<trN>",
        "take a transition off",
        || schema_of::<transition::RemoveArgs>("transition.remove"),
    ),
    write(
        "track.add",
        "track",
        "--kind video|audio|caption [--index <n>] [--name <s>]",
        "add a track to --timeline; binds $track",
        || schema_of::<track::AddArgs>("track.add"),
    ),
    write(
        "track.remove",
        "track",
        "<t> [--with-items]",
        "remove a track (TRACK_NOT_EMPTY without --with-items)",
        || schema_of::<track::RemoveArgs>("track.remove"),
    ),
    write(
        "track.move",
        "track",
        "<t> --index <n>",
        "move a track in compositing order (bottom first)",
        || schema_of::<track::MoveArgs>("track.move"),
    ),
    write(
        "track.set",
        "track",
        "<t> <name|mute|solo|lock|gain> (<value>|--reset)",
        "set a track setting",
        || one_of(schema_of::<track::SetArgs>("track.set"), "value", "reset"),
    ),
    write(
        "timeline.add",
        "timeline",
        "--name <s> [--rate <fps>] [--resolution <WxH>]",
        "add a timeline; binds $timeline",
        || schema_of::<timeline::AddArgs>("timeline.add"),
    ),
    write(
        "timeline.set",
        "timeline",
        "<tl> <name|resolution|rate> (<value>|--reset)",
        "set a timeline setting",
        || {
            one_of(
                schema_of::<track::SetArgs>("timeline.set"),
                "value",
                "reset",
            )
        },
    ),
    write(
        "timeline.remove",
        "timeline",
        "<tl>",
        "remove a timeline with everything on it",
        || schema_of::<timeline::RemoveArgs>("timeline.remove"),
    ),
    write(
        "marker.add",
        "marker",
        "--at <time> --label <s> [--dur <time>] [--color <#hex>]",
        "add a marker to --timeline (markers never ripple); binds $marker",
        || schema_of::<marker::AddArgs>("marker.add"),
    ),
    write(
        "marker.set",
        "marker",
        "<mN> <label|color|at|duration> (<value>|--reset)",
        "set a marker field",
        || one_of(schema_of::<track::SetArgs>("marker.set"), "value", "reset"),
    ),
    write("marker.remove", "marker", "<mN>", "remove a marker", || {
        schema_of::<marker::RemoveArgs>("marker.remove")
    }),
    write(
        "bin.add",
        "bin",
        "--name <s> [--parent <bN>]",
        "add a bin; binds $bin",
        || schema_of::<bin::AddArgs>("bin.add"),
    ),
    write(
        "bin.move",
        "bin",
        "<bN> [--parent <bN>]",
        "move a bin (top level without --parent)",
        || schema_of::<bin::MoveArgs>("bin.move"),
    ),
    write("bin.remove", "bin", "<bN>", "remove an empty bin", || {
        schema_of::<bin::RemoveArgs>("bin.remove")
    }),
    write(
        "asset.move",
        "asset",
        "<aN> [--bin <bN>]",
        "file an asset in a bin (top level without --bin)",
        || schema_of::<asset::MoveArgs>("asset.move"),
    ),
    write(
        "asset.remove",
        "asset",
        "<aN>",
        "remove an unused asset (ASSET_IN_USE lists its users)",
        || schema_of::<asset::RemoveArgs>("asset.remove"),
    ),
    write(
        "asset.reprobe",
        "asset",
        "<aN> [--deep]",
        "probe a media asset's unchanged file again and record its display size, rotation and pixel aspect (server only)",
        || schema_of::<asset::ReprobeArgs>("asset.reprobe"),
    ),
];
