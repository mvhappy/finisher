//! The scoped reads' and `fin log`'s registry entries (review batch L), listed after the
//! settings commands in [`crate::registry::commands`]; their rows are here too.

use crate::commands::{inspect, log};
use crate::registry::{Command, schema_of};
use crate::registry_rpcs::{Row, RpcUse, row, rpc};

const fn read(
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
        write: false,
        schema,
    }
}

pub const INSPECT_COMMANDS: &[Command] = &[
    read(
        "track.ls",
        "track",
        "[--offset <n>] [--limit <n>]",
        "tracks of --timeline (default all): id, name, kind, index, items, end, flags",
        || schema_of::<inspect::TrackLsArgs>("track.ls"),
    ),
    read(
        "asset.ls",
        "asset",
        "[--bin <bN>] [--offset <n>] [--limit <n>]",
        "assets: id, kind, path, bin, duration",
        || schema_of::<inspect::AssetLsArgs>("asset.ls"),
    ),
    read(
        "bin.ls",
        "bin",
        "[--offset <n>] [--limit <n>]",
        "bins as a tree: id, name, parent, path, bins and assets held",
        || schema_of::<inspect::BinLsArgs>("bin.ls"),
    ),
    read(
        "effect.ls",
        "effect",
        "[<i>] [--offset <n>] [--limit <n>]",
        "stored effects, disabled too: item, id, kind, enabled, index, params",
        || schema_of::<inspect::EffectLsArgs>("effect.ls"),
    ),
    read(
        "log",
        "history",
        "[<txId>] [--before <rev>] [--limit <n>]",
        "the branch's records, newest first (paged); one record's ops and changes",
        || schema_of::<log::LogArgs>("log"),
    ),
];

const PB: &[&str] = &["projectId", "branch"];

/// The rows: the reads take a snapshot (`snapshot.get`, a helper, as for `view`); `log` sends
/// `history.log` (`tx.get` for one record, a helper).
pub const INSPECT_ROWS: &[Row] = &[
    row!("track.ls", rpc!(), PB),
    row!("asset.ls", rpc!(), PB),
    row!("bin.ls", rpc!(), PB),
    row!("effect.ls", rpc!(), PB),
    row!("log", rpc!("history.log"), PB),
];
